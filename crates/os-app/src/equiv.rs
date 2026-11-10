// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The equivalence harness (specs: roadmap/control-plane.md part 2, "Method"). It drives the app without a window the
//! way a user does (clicks, double clicks, right clicks, drags, the wheel and keys, on what the frame shows) and after
//! every step writes down a fingerprint of what the step may change: the project, the project state the interface
//! holds, what `.lemon` would store, the undo history, whether the engine holds the project as it is, and whether the
//! step left something to save. A trace (a project, its inputs and the fingerprints) recorded on one build is replayed on another: the
//! first step whose fingerprint differs names the input that behaves differently.
//!
//! At the end of each trace it also checks what needs no other build: undo back to the start gives the opened project,
//! redo gives the last one back, the engine holds what the project says, and a saved copy opens as the same project.
//!
//! Each recording and replay runs in a process of its own with a home folder of its own under `OS_EQUIV_DIR` (the
//! app saves projects, backups and settings there, and no trace sees another's). Run outside the sandbox and on the
//! hidden display (`hidden-tests`):
//! ```text
//! OS_EQUIV_DIR=<traces> OS_EQUIV_CORPUS=<folder of .flp> \
//!   cargo test -p os-app --bin lemonstudio equiv_record -- --ignored --nocapture
//! ```
//! `equiv_show` runs in the test's own process: give it `HOME=<OS_EQUIV_DIR>/home` and no `DISPLAY` (`equiv_shrink`
//! runs each candidate in a process of its own, as a recording does). Both work in the folder the trace's recording
//! used, so `OS_EQUIV_DIR` must be the recording's folder: a save writes its path.
//! `equiv_replay` replays every trace in `OS_EQUIV_DIR`; `equiv_twice` records and replays on the same build (what
//! isn't deterministic yet). `equiv_coverage` adds up what the recordings in `OS_EQUIV_DIR` reached: each kind of
//! control, how many frames showed it, how often a step aimed at it, and whether it ever changed something. `OS_EQUIV_SEEDS` (2) traces per project, `OS_EQUIV_STEPS` (300) steps each,
//! `OS_EQUIV_ONLY=<name part>` limits the projects, `OS_EQUIV_SEED_FROM` (1) the first seed, `OS_EQUIV_FIRST=<n>` takes the first n of the corpus.

use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use winit::keyboard::{Key, NamedKey};

use crate::app::{Action, App};

fn var<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// FNV-1a, 64 bits: the same on every build and toolchain, unlike the standard library's hasher.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

impl fmt::Write for Fnv {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.bytes(s.as_bytes());
        Ok(())
    }
}

fn hash(v: &impl fmt::Debug) -> u64 {
    let mut h = Fnv::new();
    let _ = write!(h, "{v:?}");
    h.0
}

/// Project state the interface holds until save (specs: control-plane.md part 2, "Where main is"), as one value: a
/// field per line, so a difference names its field.
fn side(app: &App) -> String {
    let (s, r) = (&app.state, &app.state.rack);
    // "Unsorted" at the end of the group list reads as None (last) or as its place there: the same thing.
    let unsorted = r.unsorted_at.filter(|&u| u < r.groups.len());
    let mut out = String::new();
    let mut f = |name: &str, v: &dyn fmt::Debug| {
        let _ = writeln!(out, "{name}: {v:#?}");
    };
    f("tempo", &s.tempo.to_bits());
    f("pattern", &s.pattern);
    f("song_mode", &s.song_mode);
    f("filter", &r.filter);
    f("groups", &r.groups);
    f("zipped", &r.zipped);
    f("locked", &r.locked);
    f("loop_mode", &r.loop_mode);
    f("loop_len", &r.loop_len);
    f("loop_unit", &r.loop_unit);
    f("loop_type", &r.loop_type);
    f("unsorted_at", &unsorted);
    out
}

/// What a step may change, each part hashed. Parts are named so a difference says where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fp(Vec<(&'static str, u64)>);

impl Fp {
    fn of(app: &App) -> Fp {
        let s = &app.state;
        let side = hash(&side(app));
        let history = hash(&(s.history.entries(), s.history.at()));
        Fp(vec![
            ("project", hash(&s.project)),
            ("side", side),
            ("lemon", hash(&crate::lemon_state::saved_form(s, &app.plugins))),
            ("history", history),
            // Whether the engine holds the project as it is: 0 nothing sent yet, 1 in step, 2 behind. Not the copy
            // itself: what it was last handed while playing starts or pauses follows the audio thread's timing.
            ("engine", match app.transport.sent_for_tests() {
                None => 0,
                Some(p) if p == crate::rack::play_project(s) => 1,
                Some(_) => 2,
            }),
            ("unsaved", u64::from(app.unsaved())),
        ])
    }

    fn parse(t: &str) -> Option<Fp> {
        let mut v = Vec::new();
        for part in t.split_whitespace() {
            let (k, h) = part.split_once('=')?;
            let k = ["project", "side", "lemon", "history", "engine", "unsaved"].into_iter().find(|n| *n == k)?;
            v.push((k, u64::from_str_radix(h, 16).ok()?));
        }
        Some(Fp(v))
    }

    /// The names of the parts that differ from `o`.
    fn differs(&self, o: &Fp) -> Vec<&'static str> {
        self.0.iter().zip(&o.0).filter(|(a, b)| a != b).map(|(a, _)| a.0).collect()
    }
}

impl fmt::Display for Fp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (k, h)) in self.0.iter().enumerate() {
            write!(f, "{}{k}={h:x}", if i > 0 { " " } else { "" })?;
        }
        Ok(())
    }
}

/// One input, as a trace writes it.
#[derive(Debug, Clone, PartialEq)]
enum Input {
    Move(f32, f32),
    Down,
    Up,
    Right,
    Wheel(f32, bool, bool),
    Key(String, bool, bool),
    Mods(bool, bool, bool),
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = |v: bool| u8::from(v);
        match self {
            Input::Move(x, y) => write!(f, "move {x} {y}"),
            Input::Down => write!(f, "down"),
            Input::Up => write!(f, "up"),
            Input::Right => write!(f, "right"),
            Input::Wheel(dy, shift, ctrl) => write!(f, "wheel {dy} {} {}", b(*shift), b(*ctrl)),
            Input::Key(k, ctrl, alt) => write!(f, "key {k} {} {}", b(*ctrl), b(*alt)),
            Input::Mods(shift, ctrl, alt) => write!(f, "mods {} {} {}", b(*shift), b(*ctrl), b(*alt)),
        }
    }
}

impl Input {
    fn parse(t: &str) -> Option<Input> {
        let w: Vec<&str> = t.split_whitespace().collect();
        let n = |i: usize| w.get(i).and_then(|v| v.parse::<f32>().ok());
        let b = |i: usize| w.get(i).map(|v| *v == "1");
        Some(match *w.first()? {
            "move" => Input::Move(n(1)?, n(2)?),
            "down" => Input::Down,
            "up" => Input::Up,
            "right" => Input::Right,
            "wheel" => Input::Wheel(n(1)?, b(2)?, b(3)?),
            "key" => Input::Key(w.get(1)?.to_string(), b(2)?, b(3)?),
            "mods" => Input::Mods(b(1)?, b(2)?, b(3)?),
            _ => return None,
        })
    }

    fn apply(&self, app: &mut App) {
        match self {
            Input::Move(x, y) => {
                app.mouse_move(*x, *y);
                // `OS_EQUIV_SHOW=1`: what each move lands on, to read a shrunk trace.
                if std::env::var("OS_EQUIV_SHOW").is_ok() {
                    let on = app.hits.iter().rev().find(|h| h.rect.contains(*x, *y)).map(|h| format!("{:?} ({})", h.action, h.hint));
                    eprintln!("  at {x} {y}: {}", on.unwrap_or_else(|| "nothing".into()));
                }
            }
            Input::Down => app.mouse_down(),
            Input::Up => app.mouse_up(),
            Input::Right => app.mouse_down_right(),
            Input::Wheel(dy, shift, ctrl) => app.wheel(0.0, *dy, *shift, *ctrl),
            Input::Key(k, ctrl, alt) => {
                if let Some(key) = key_of(k) {
                    app.key(&key, *ctrl, *alt);
                }
            }
            Input::Mods(shift, ctrl, alt) => {
                app.shift = *shift;
                app.ctrl = *ctrl;
                app.alt = *alt;
            }
        }
    }
}

/// The keys the generator presses: letters, digits and the named keys the shortcuts use.
const NAMED: [(&str, NamedKey); 18] = [
    ("Delete", NamedKey::Delete),
    ("Backspace", NamedKey::Backspace),
    ("Escape", NamedKey::Escape),
    ("Enter", NamedKey::Enter),
    ("Tab", NamedKey::Tab),
    ("Space", NamedKey::Space),
    ("Up", NamedKey::ArrowUp),
    ("Down", NamedKey::ArrowDown),
    ("Left", NamedKey::ArrowLeft),
    ("Right", NamedKey::ArrowRight),
    ("Home", NamedKey::Home),
    ("End", NamedKey::End),
    ("PageUp", NamedKey::PageUp),
    ("PageDown", NamedKey::PageDown),
    ("Insert", NamedKey::Insert),
    ("F6", NamedKey::F6),
    ("F7", NamedKey::F7),
    ("F9", NamedKey::F9),
];
const CHARS: &str = "abcdefghijklmnopqrstuvwxyz0123456789+-";

fn key_of(name: &str) -> Option<Key> {
    if let Some((_, k)) = NAMED.iter().find(|(n, _)| *n == name) {
        return Some(Key::Named(*k));
    }
    name.strip_prefix("c:").map(|c| Key::Character(c.into()))
}

/// xorshift64*: a seed makes the same trace on every build.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn signed(&mut self, max: f32) -> f32 {
        ((self.next() % 2001) as f32 / 1000.0 - 1.0) * max
    }
}

/// The frame the app paints into: one size for every build.
const FRAME: (u32, u32) = (1920, 1080);

/// The app, the frame it paints and the clock its motions read.
struct Rig {
    app: App,
    canvas: os_ui::Canvas,
    /// Steps run so far, for `OS_EQUIV_DUMP`.
    step: usize,
    /// How often the generator aimed at each kind of control: it favours the kinds it has used least.
    used: std::collections::BTreeMap<String, u32>,
    /// The kinds of control whose step changed the project, the interface's part of it or Lemon Studio's own state:
    /// these weigh more, so the controls that edit are tried in more ways than those that only show something.
    edits: std::collections::BTreeSet<String>,
    /// The kind the last generated step aimed at.
    aimed: Option<String>,
    /// The first step after which two things shared an id, or one had none (`ids_fault`).
    id_fault: Option<String>,
    /// How many frames showed each kind of control: with `used` and `edits`, what a recording reports as its
    /// coverage (`<trace>.kinds`).
    seen: std::collections::BTreeMap<String, u32>,
}

/// A control's kind for the generator's novelty: its action's variant, and the variant inside it.
fn kind(a: &Action) -> String {
    let d = format!("{a:?}");
    let mut parts = d.split(['(', ')', ' ', '{', ',']).filter(|p| p.chars().next().is_some_and(char::is_uppercase));
    match (parts.next(), parts.next()) {
        (Some(a), Some(b)) => format!("{a}.{b}"),
        (Some(a), None) => a.to_string(),
        _ => d,
    }
}

/// Where the clock starts: the same instant for every rig of this run, so motions and holds see the same times.
fn epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

impl Rig {
    /// Opens a copy of `project` in `work` (the app may save over what it opened).
    fn open(project: &Path, work: &Path) -> Rig {
        // The app writes settings, backups and recent files under the home folder: never the user's own.
        let home = std::env::var("HOME").unwrap_or_default();
        assert!(Path::new(&home).starts_with(traces_dir()) || std::env::var("OS_EQUIV_HOME_OK").is_ok(), "HOME must be a folder under OS_EQUIV_DIR, not {home}");
        crate::transport::headless();
        crate::anim::set_clock_at(epoch());
        let _ = std::fs::create_dir_all(work);
        let copy = work.join(project.file_name().expect("a file"));
        std::fs::copy(project, &copy).expect("copy the project");
        let mut app = App::new(None);
        app.plugins = crate::plugins::Plugins::default();
        app.plugins.available = crate::plugins::builtin();
        app.plugins.effects = crate::plugins::builtin_effects();
        app.open(copy);
        let mut rig = Rig { app, canvas: os_ui::Canvas::new(FRAME.0, FRAME.1, 1.0).expect("a canvas"), step: 0, used: Default::default(), edits: Default::default(), aimed: None, seen: Default::default(), id_fault: None };
        rig.settle();
        // Mixer effects are hosted a few ticks after loading.
        for _ in 0..20 {
            rig.tick();
        }
        // The engine is handed the project as opened, as playing it would: from here an edit that doesn't reach the
        // engine leaves it behind, where with nothing sent yet it would look the same as one that does.
        rig.app.transport.update(&crate::rack::play_project(&rig.app.state));
        rig.paint();
        rig
    }

    /// One frame of the app's loop: 10 ms on the motions' clock.
    fn tick(&mut self) {
        crate::anim::advance(10);
        self.app.tick();
    }

    /// Until loading, hosting and recording are done: loading has no clock of ours (its budget is the machine's), so
    /// it is waited for, not counted.
    fn settle(&mut self) {
        let t0 = Instant::now();
        while self.app.loading() && t0.elapsed() < Duration::from_secs(180) {
            self.app.tick();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn paint(&mut self) {
        self.app.invalidate();
        self.app.paint(&mut self.canvas, None);
    }

    /// After a step: a few frames, playback stopped (its position would follow the machine's clock), requests to the
    /// window dropped (there is none), loading waited for, and a full frame painted so the hits are current.
    fn after_step(&mut self, generation: u64) {
        for _ in 0..4 {
            self.tick();
        }
        if self.app.transport.playing() {
            self.app.act(Action::Stop);
            self.tick();
        }
        // A step that started or stopped playback: with no device the audio thread may stop it on its own before the
        // Stop above, which leaves the engine's copy otherwise. Either way it is stopped and handed the project as it
        // is now, so the fingerprint doesn't follow that race.
        if self.app.transport.generation_for_tests() != generation {
            self.app.transport.stop();
            self.app.transport.update(&crate::rack::play_project(&self.app.state));
        }
        self.app.want_close = false;
        self.settle();
        self.tick();
        self.paint();
    }

    fn run(&mut self, inputs: &[Input]) -> Fp {
        let generation = self.app.transport.generation_for_tests();
        for i in inputs {
            i.apply(&mut self.app);
        }
        self.after_step(generation);
        // `OS_EQUIV_DUMP=<step>`: the project and the interface's part of it after that step, numbered per run, to
        // compare two runs that differ there.
        if std::env::var("OS_EQUIV_DUMP").ok().and_then(|v| v.parse().ok()) == Some(self.step) {
            let dir = traces_dir().join("dumps");
            let _ = std::fs::create_dir_all(&dir);
            let n = std::fs::read_dir(&dir).map_or(0, |d| d.count());
            let _ = std::fs::write(dir.join(format!("{n}.txt")), format!("{:#?}\n{}", self.app.state.project, side(&self.app)));
        }
        if self.id_fault.is_none() {
            self.id_fault = ids_fault(&self.app.state.project).map(|f| format!("step {}: {f}", self.step));
        }
        self.step += 1;
        let fp = Fp::of(&self.app);
        // Each step's "unsaved" says whether that step made a change to save, not whether any step before it did.
        self.app.mark_saved();
        fp
    }

    /// Where the generator may aim: the centres of the hits the last frame registered, on the frame, with each one's
    /// kind and whether it holds a value (a knob, fader, slider or anything automatable: the tempo, a value box).
    fn targets(&self) -> Vec<(f32, f32, String, bool)> {
        let (w, h) = (FRAME.0 as f32, FRAME.1 as f32);
        self.app
            .hits
            .iter()
            .filter(|h| h.rect.w >= 1.0 && h.rect.h >= 1.0)
            // Only what a click there reaches: a control under a dialog, a menu or another window is covered (a
            // click lands on what covers it), and aiming at it would leave the dialog's own buttons rarely tried.
            .filter(|h| self.app.top_hit(h.rect.x + h.rect.w / 2.0, h.rect.y + h.rect.h / 2.0).is_some_and(|t| t.action == h.action))
            .map(|h| {
                let valued = crate::knobs::Control::of(&h.action).is_some() || self.app.automation_target(&h.action).is_some();
                (h.rect.x + h.rect.w / 2.0, h.rect.y + h.rect.h / 2.0, kind(&h.action), valued)
            })
            .filter(|&(x, y, ..)| (0.0..w).contains(&x) && (0.0..h).contains(&y))
            .collect()
    }

    /// The next step's inputs, aimed at what the frame shows.
    fn generate(&mut self, rng: &mut Rng) -> Vec<Input> {
        let targets = self.targets();
        for k in targets.iter().map(|t| &t.2).collect::<std::collections::BTreeSet<_>>() {
            *self.seen.entry(k.clone()).or_default() += 1;
        }
        // Weighted by novelty: a kind of control aimed at n times weighs 1/(n+1), so the many step buttons or grid
        // cells of one kind don't crowd out the few knobs, boxes and menus. A kind that has edited weighs four times
        // as much: its drags, wheels and modifiers are where edits go wrong. The Settings window's controls weigh a
        // quarter: they change preferences, not the project.
        // A kind never aimed at yet weighs 64 times more, per control: a dialog's or a name box's buttons, shown for a
        // few frames, are tried while they are there, not crowded out by the hundreds of step buttons and grid cells.
        let weight = |k: &str, _valued: bool| {
            let edit = if self.edits.contains(k) { 16 } else { 4 };
            let edit = if k.starts_with("Settings") { edit / 4 } else { edit };
            let used = u64::from(self.used.get(k).copied().unwrap_or(0));
            let fresh = if used == 0 { 64 } else { 1 };
            fresh * edit * 1_000_000 / (1 + used)
        };
        let total: u64 = targets.iter().map(|t| weight(&t.2, t.3)).sum();
        if total == 0 {
            self.aimed = None;
            return vec![Input::Key("Escape".into(), false, false)];
        }
        let mut at = rng.next() % total;
        let Some((x, y, k, valued)) = targets.iter().find(|t| {
            let w = weight(&t.2, t.3);
            if at < w {
                return true;
            }
            at -= w;
            false
        }).cloned() else {
            self.aimed = None;
            return vec![Input::Key("Escape".into(), false, false)];
        };
        *self.used.entry(k.clone()).or_default() += 1;
        self.aimed = Some(k);
        let (shift, ctrl, alt) = (rng.chance(12), rng.chance(12), rng.chance(5));
        let mods = Input::Mods(shift, ctrl, alt);
        let none = Input::Mods(false, false, false);
        // A control holding a value is mostly dragged or wheeled: a click alone rarely changes it.
        let roll = if valued && rng.chance(70) { 58 + rng.below(27) } else { rng.below(100) };
        match roll {
            0..40 => vec![mods, Input::Move(x, y), Input::Down, Input::Up, none],
            40..48 => vec![Input::Move(x, y), Input::Down, Input::Up, Input::Down, Input::Up],
            48..58 => vec![Input::Move(x, y), Input::Right, Input::Up],
            58..73 => {
                let mut v = vec![mods, Input::Move(x, y), Input::Down];
                let (mut px, mut py) = (x, y);
                for _ in 0..1 + rng.below(5) {
                    px += rng.signed(80.0);
                    py += rng.signed(80.0);
                    v.push(Input::Move(px.round(), py.round()));
                }
                v.extend([Input::Up, none]);
                v
            }
            73..85 => vec![Input::Move(x, y), Input::Wheel(if rng.chance(50) { 1.0 } else { -1.0 }, rng.chance(20), rng.chance(30))],
            _ => {
                let k = if rng.chance(40) {
                    NAMED[rng.below(NAMED.len())].0.to_string()
                } else {
                    let c = CHARS.as_bytes()[rng.below(CHARS.len())] as char;
                    format!("c:{c}")
                };
                vec![Input::Key(k, rng.chance(25), rng.chance(8))]
            }
        }
    }

    /// With no steps in the history (just opened, or a project opened or made by a step), undo starts from here.
    fn undo_start(&self, start: &mut os_model::Project) {
        if self.app.state.history.entries().len() <= 1 {
            start.clone_from(&self.app.state.project);
        }
    }

    /// The checks that need no other build, at the end of a trace: each a name when it fails.
    fn invariants(&mut self, opened: &os_model::Project, work: &Path) -> Vec<String> {
        let mut bad = Vec::new();
        // The checks run as a user would get to them: with no dialog open. A piano roll tool's preview goes as its ×
        // does (no undo step was made for it), a message box or form as Escape closes it.
        crate::piano_dlg::cancel(&mut self.app.state);
        for _ in 0..8 {
            if !self.app.modal_open() {
                break;
            }
            self.run(&[Input::Key("Escape".into(), false, false)]);
        }
        // And stopped: a take still recording (a Touch controller note with Start on input on) would keep changing the
        // project, by the clock of a device that is not there, while the checks undo, redo and save.
        if self.app.transport.playing() || self.app.state.rec.recording() {
            self.app.act(crate::app::Action::Stop);
            self.run(&[]);
        }
        // The engine holds what the project says (an edit that never reached it shows here): one finding per top-level
        // field that differs, named by its first difference, so what is accepted is decided per field and the names
        // don't depend on the accepted list.
        if let Some(sent) = self.app.transport.sent_for_tests() {
            let play = crate::rack::play_project(&self.app.state);
            if sent != play {
                let diff = differences(&sent, &play, false);
                if diff.is_empty() {
                    bad.push("engine-behind".into());
                }
                bad.extend(engine_findings(&diff));
            }
        }
        if let Some(f) = &self.id_fault {
            bad.push(format!("ids({f})"));
        }
        let last = self.app.state.project.clone();
        let last_side = side(&self.app);
        // Undo back to the start, then redo to where the trace ended.
        let mut undone = 0;
        while crate::history::undo(&mut self.app.state).is_some() && undone < 10_000 {
            undone += 1;
            self.tick();
        }
        self.after_step(self.app.transport.generation_for_tests());
        if let Some(d) = difference(&canonical(opened), &canonical(&self.app.state.project), false) {
            bad.push(format!("undo-to-start({undone}:{d})"));
        }
        // Redo as many steps as were done at the end: steps the trace itself undid stay undone.
        for _ in 0..undone {
            if crate::history::redo(&mut self.app.state).is_none() {
                break;
            }
            self.tick();
        }
        self.after_step(self.app.transport.generation_for_tests());
        if let Some(d) = difference(&canonical(&last), &canonical(&self.app.state.project), false) {
            bad.push(format!("redo-to-end({d})"));
        }
        let now_side = side(&self.app);
        if now_side != last_side {
            let (a, b) = (leaves(&last_side), leaves(&now_side));
            let d = a.iter().zip(&b).find(|(x, y)| x != y).map_or("length".into(), |(x, y)| format!("{}:{}->{}", x.0, short(&x.1), short(&y.1)));
            bad.push(format!("redo-to-end(side:{d})"));
        }
        // A saved copy opens as the same project, and the interface's part of it (tempo, current pattern, rack state)
        // as the interface held it.
        let copy = work.join("roundtrip.flp");
        // The project as the save wrote it: with what the interface holds written in (the tempo, the pattern shown,
        // the rack's loops and groups, the playlist's view) and the plugins' states; the interface's part is compared
        // on its own below.
        let mut written = self.app.state.project.clone();
        match self.app.save_copy_with(copy.clone(), &mut |p| written = p.clone()) {
            Some(Ok(())) => {
                let mut again = open_app(&copy);
                let back = again.state.project.clone();
                if let Some(d) = difference(&canonical(&written), &canonical(&back), true) {
                    bad.push(format!("roundtrip({d})"));
                }
                let (a, b) = (side(&self.app), side(&again));
                if a != b {
                    let (a, b) = (leaves(&a), leaves(&b));
                    let d = a.iter().zip(&b).find(|(x, y)| x != y).map_or("length".into(), |(x, y)| format!("{}:{}->{}", x.0, short(&x.1), short(&y.1)));
                    bad.push(format!("roundtrip(side:{d})"));
                }
                // Saving what was opened again writes the same project: nothing drifts from save to save.
                let second = work.join("roundtrip2.flp");
                match again.save_copy_with(second.clone(), &mut |_| {}) {
                    Some(Ok(())) => {
                        let model = |p: &Path| std::fs::read(p).ok().and_then(|b| os_flp::FlpFile::parse(&b).ok()).and_then(|f| os_flp::project::load(&f).ok());
                        match (model(&copy), model(&second)) {
                            (Some(x), Some(y)) => {
                                if let Some(d) = difference(&x, &y, false) {
                                    bad.push(format!("resave({d})"));
                                }
                            }
                            _ => bad.push("resave(unreadable)".into()),
                        }
                    }
                    other => bad.push(format!("resave-failed({other:?})")),
                }
            }
            other => bad.push(format!("save-failed({other:?})")),
        }
        // One word each: a trace keeps them on one line.
        bad.into_iter().map(|b| b.split_whitespace().collect::<Vec<_>>().join("_")).collect()
    }
}

/// A project as the checks compare it: what its lists' order and empty patterns say is left out. The file holds the
/// clips in position order and the patterns by number, so their order in the lists means nothing; a pattern with
/// nothing in it (no notes, name, colour, length, loops or markers) has nothing to write and reads back as absent;
/// a send level at unity is the same as none.
fn canonical(p: &os_model::Project) -> os_model::Project {
    let mut p = p.clone();
    for a in &mut p.arrangements {
        // A clip's position, kind, item, length, track, mute bit and id are written from its fields on every save
        // (os-flp encode_clip, encode_clips): the stored record's copy of them is left as it was read, so a clip made
        // here holds zeros. Its offsets (bytes 24..32) are its window, written from it when it changed (an edit
        // that cuts or trims a clip changes only the window); the window is compared.
        for c in &mut a.clips {
            c.raw[..14].fill(0);
            c.raw[24..32].fill(0);
            c.raw[19] &= !0x20;
            c.raw[32..36].fill(0);
        }
        a.clips.sort_by_cached_key(|c| format!("{:012}:{:06}:{c:?}", c.position, c.track));
    }
    // A pattern with nothing in it, loop lengths and loop mode alone included, is not stored, and one without notes
    // keeps no loop lengths: FL keeps none for a pattern without notes (flp/events/patterns.md "Loop lengths").
    for q in &mut p.patterns {
        if q.notes.is_empty() {
            (q.loops, q.loop_off) = (Vec::new(), None);
        }
    }
    p.patterns.retain(|q| {
        !(q.notes.is_empty() && q.controls.is_empty() && q.name.is_none() && q.color.is_none() && q.length.is_none())
            || !q.markers.is_empty()
    });
    p.patterns.sort_by_key(|q| q.number);
    // A mixer track's origin is bookkeeping (os-model: tracks that differ only there are equal).
    for m in &mut p.mixer {
        m.origin = None;
    }
    // A route with no level record plays at unity (0.8 on the fader scale), as one with a record at 0.8 does.
    for m in &mut p.mixer {
        m.send_levels.retain(|&(_, l)| (l - 0.8).abs() > 1e-9);
        // A save writes them by destination (os-flp mixer records), whatever order they were set in.
        m.send_levels.sort_by_key(|&(d, _)| d);
    }
    p
}

/// Whether every channel, pattern, mixer track, slot plugin, arrangement and clip has an id and no two share one (the
/// session gives them out from one counter, `control::ids`): what is wrong, if anything.
fn ids_fault(p: &os_model::Project) -> Option<String> {
    let mut all: Vec<(&str, os_model::Id)> = Vec::new();
    all.extend(p.channels.iter().map(|c| ("channel", c.id)));
    all.extend(p.patterns.iter().map(|q| ("pattern", q.id)));
    all.extend(p.mixer.iter().map(|m| ("track", m.id)));
    all.extend(p.mixer.iter().flat_map(|m| m.slots.iter().flatten()).map(|x| ("slot", x.id)));
    all.extend(p.arrangements.iter().map(|a| ("arrangement", a.id)));
    all.extend(p.arrangements.iter().flat_map(|a| &a.clips).map(|c| ("clip", c.id)));
    if let Some((kind, _)) = all.iter().find(|(_, id)| id.is_none()) {
        return Some(format!("a {kind} without an id"));
    }
    let mut seen = std::collections::HashMap::new();
    for (kind, id) in &all {
        if let Some(first) = seen.insert(id.0, *kind) {
            return Some(format!("a {first} and a {kind} share id {}", id.0));
        }
    }
    None
}

/// A second app with `path` opened, as the rig opens (builtin plugins only), loading waited for.
fn open_app(path: &Path) -> App {
    let mut a = App::new(None);
    a.plugins = crate::plugins::Plugins::default();
    a.plugins.available = crate::plugins::builtin();
    a.plugins.effects = crate::plugins::builtin_effects();
    a.open(path.to_path_buf());
    let t0 = Instant::now();
    while a.loading() && t0.elapsed() < Duration::from_secs(180) {
        a.tick();
        std::thread::sleep(Duration::from_millis(2));
    }
    a
}

/// The leaves of a value's pretty debug form, each with its path (`channels[3].volume`).
fn leaves(text: &str) -> Vec<(String, String)> {
    // A frame: its path, and the next item's index when it is a list.
    let mut stack: Vec<(String, Option<usize>)> = vec![(String::new(), None)];
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim().trim_end_matches(',');
        if l.starts_with(['}', ']', ')']) {
            stack.pop();
            if let Some((_, Some(i))) = stack.last_mut() {
                *i += 1;
            }
            continue;
        }
        let (parent, index) = stack.last().cloned().unwrap_or_default();
        let (key, rest) = match index {
            Some(i) => (format!("{parent}[{i}]"), l),
            None => match l.split_once(": ") {
                Some((k, r)) if !k.contains(' ') || k.starts_with('"') => (if parent.is_empty() { k.to_string() } else { format!("{parent}.{k}") }, r),
                _ => (parent.clone(), l),
            },
        };
        if rest.ends_with(['{', '[', '(']) {
            // Lists and tuples number their items.
            stack.push((key, (!rest.ends_with('{')).then_some(0)));
        } else {
            out.push((key, rest.to_string()));
            if let Some((_, Some(i))) = stack.last_mut() {
                *i += 1;
            }
        }
    }
    out
}

/// Where two values differ: the first differing path with both sides, and how many paths differ, or None. Ids are
/// left out (they are not saved yet and are handed out again after an undo: specs control-plane.md part 2, step 12);
/// `rounding` takes numbers within 1e-4 as equal (what the file format keeps of them).
fn difference(a: &impl fmt::Debug, b: &impl fmt::Debug, rounding: bool) -> Option<String> {
    let diff = differences(a, b, rounding);
    diff.first().map(|d| format!("{d}(+{})", diff.len() - 1))
}

/// Every leaf where `a` and `b` differ, as `path:a->b`.
/// `engine-behind` findings from the differences between what the engine holds and what the project says: one per
/// top-level field (the path up to its first `.` or `[`), in the order found, named by its first difference and the
/// count of the others.
fn engine_findings(diff: &[String]) -> Vec<String> {
    let top = |d: &str| d[..d.find(['.', '[', ':']).unwrap_or(d.len())].to_string();
    let mut groups: Vec<(String, &String, usize)> = Vec::new();
    for d in diff {
        match groups.iter_mut().find(|(t, _, _)| *t == top(d)) {
            Some(g) => g.2 += 1,
            None => groups.push((top(d), d, 0)),
        }
    }
    groups.into_iter().map(|(_, d, n)| format!("engine-behind({d}(+{n}))")).collect()
}

fn differences(a: &impl fmt::Debug, b: &impl fmt::Debug, rounding: bool) -> Vec<String> {
    let kept = |l: Vec<(String, String)>| -> Vec<(String, String)> { l.into_iter().filter(|(k, _)| !(k.ends_with(".id") || k.contains(".id["))).collect() };
    let (a, b) = (kept(leaves(&format!("{a:#?}"))), kept(leaves(&format!("{b:#?}"))));
    let same = |x: &str, y: &str| {
        x == y || (rounding && matches!((x.parse::<f64>(), y.parse::<f64>()), (Ok(p), Ok(q)) if (p - q).abs() <= 1e-4))
    };
    let bm: std::collections::BTreeMap<&str, &str> = b.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let am: std::collections::BTreeMap<&str, &str> = a.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut diff: Vec<String> = a
        .iter()
        .filter(|(k, v)| !bm.get(k.as_str()).is_some_and(|w| same(v, w)))
        .map(|(k, v)| format!("{k}:{}->{}", short(v), bm.get(k.as_str()).map_or("absent".into(), |w| short(w))))
        .collect();
    diff.extend(b.iter().filter(|(k, _)| !am.contains_key(k.as_str())).map(|(k, v)| format!("{k}:absent->{}", short(v))));
    diff
}

fn short(v: &str) -> String {
    v.chars().filter(|c| !c.is_whitespace()).take(40).collect()
}

/// A trace: the project it opens, the seed that made it, and each step's inputs with the fingerprint after it.
struct Trace {
    project: String,
    seed: u64,
    opened: Fp,
    steps: Vec<(Vec<Input>, Fp)>,
    invariants: Vec<String>,
}

impl Trace {
    fn write(&self) -> String {
        let mut t = format!("project {}\nseed {}\nopened {}\n", self.project, self.seed, self.opened);
        for (inputs, fp) in &self.steps {
            let line: Vec<String> = inputs.iter().map(ToString::to_string).collect();
            let _ = writeln!(t, "{}\t{fp}", line.join("; "));
        }
        let _ = writeln!(t, "invariants {}", if self.invariants.is_empty() { "ok".into() } else { self.invariants.join(" ") });
        t
    }

    fn read(t: &str) -> Option<Trace> {
        let mut lines = t.lines();
        let project = lines.next()?.strip_prefix("project ")?.to_string();
        let seed = lines.next()?.strip_prefix("seed ")?.parse().ok()?;
        let opened = Fp::parse(lines.next()?.strip_prefix("opened ")?)?;
        let mut steps = Vec::new();
        let mut invariants = Vec::new();
        for l in lines {
            if let Some(rest) = l.strip_prefix("invariants ") {
                invariants = rest.split_whitespace().filter(|w| *w != "ok").map(String::from).collect();
                continue;
            }
            let (inputs, fp) = l.split_once('\t')?;
            let inputs = inputs.split("; ").map(Input::parse).collect::<Option<Vec<_>>>()?;
            steps.push((inputs, Fp::parse(fp)?));
        }
        Some(Trace { project, seed, opened, steps, invariants })
    }
}

/// The projects of the corpus, sorted, as paths relative to it.
fn corpus() -> (PathBuf, Vec<String>) {
    let root = PathBuf::from(std::env::var("OS_EQUIV_CORPUS").expect("OS_EQUIV_CORPUS: a folder of projects"));
    let mut v = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("flp")) {
                v.push(p.strip_prefix(&root).expect("inside").to_string_lossy().into_owned());
            }
        }
    }
    v.sort();
    if let Ok(only) = std::env::var("OS_EQUIV_ONLY") {
        v.retain(|p| p.contains(&only));
    }
    v.truncate(var("OS_EQUIV_FIRST", usize::MAX));
    (root, v)
}

fn traces_dir() -> PathBuf {
    PathBuf::from(std::env::var("OS_EQUIV_DIR").expect("OS_EQUIV_DIR: where traces live"))
}

/// A scratch folder for one trace's copies (under the traces' folder, emptied first).
fn work_dir(name: &str) -> PathBuf {
    let d = traces_dir().join("work").join(name);
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn trace_name(project: &str, seed: u64) -> String {
    let stem: String = project.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    format!("{stem}-{seed}")
}

fn record(root: &Path, project: &str, seed: u64, steps: usize) -> Trace {
    let work = work_dir(&trace_name(project, seed));
    let mut rig = Rig::open(&root.join(project), &work);
    let opened = Fp::of(&rig.app);
    let mut opened_project = rig.app.state.project.clone();
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut out = Vec::new();
    let mut before = opened.clone();
    for _ in 0..steps {
        let inputs = rig.generate(&mut rng);
        let fp = rig.run(&inputs);
        rig.undo_start(&mut opened_project);
        if fp.differs(&before).iter().any(|p| ["project", "side", "lemon"].contains(p)) {
            if let Some(k) = rig.aimed.clone() {
                rig.edits.insert(k);
            }
        }
        before = fp.clone();
        out.push((inputs, fp));
    }
    let invariants = rig.invariants(&opened_project, &work);
    let kinds: String = rig.seen.iter().map(|(k, n)| format!("{k}\t{n}\t{}\t{}\n", rig.used.get(k).copied().unwrap_or(0), u8::from(rig.edits.contains(k)))).collect();
    let _ = std::fs::write(traces_dir().join(format!("{}.kinds", trace_name(project, seed))), kinds);
    Trace { project: project.into(), seed, opened, steps: out, invariants }
}

/// Replays `t`: None when every fingerprint matches, else a report of the first difference.
fn replay(root: &Path, t: &Trace) -> Option<String> {
    // The folder the recording used: paths the project stores (a render, a save as) must be the same.
    let work = work_dir(&trace_name(&t.project, t.seed));
    let mut rig = Rig::open(&root.join(&t.project), &work);
    let mut opened_project = rig.app.state.project.clone();
    let fp = Fp::of(&rig.app);
    if fp != t.opened {
        return Some(format!("opening differs in {:?}", fp.differs(&t.opened)));
    }
    for (i, (inputs, want)) in t.steps.iter().enumerate() {
        let got = rig.run(inputs);
        rig.undo_start(&mut opened_project);
        if &got != want {
            let line: Vec<String> = inputs.iter().map(ToString::to_string).collect();
            return Some(format!("step {i} ({}) differs in {:?}", line.join("; "), got.differs(want)));
        }
    }
    let inv = rig.invariants(&opened_project, &work);
    (inv != t.invariants).then(|| format!("invariants: recorded {:?}, now {:?}", t.invariants, inv))
}

/// What the invariants report on purpose (`equiv_accepted.txt`): (normalized prefix, why).
fn accepted() -> Vec<(&'static str, &'static str)> {
    include_str!("equiv_accepted.txt").lines().filter(|l| !l.starts_with('#') && !l.is_empty()).filter_map(|l| l.split_once('\t')).collect()
}

/// A finding as the accepted list names it: list indices as `[]`, the undo count after the kind left out.
fn normalized(finding: &str) -> String {
    let mut out = String::new();
    let mut chars = finding.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '[' && chars.peek().is_some_and(char::is_ascii_digit) {
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
        }
        if c == '(' && out.starts_with("undo-to-start(") && out.len() == "undo-to-start(".len() {
            let rest: String = chars.clone().collect();
            if let Some((n, _)) = rest.split_once(':').filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit())) {
                for _ in 0..=n.len() {
                    chars.next();
                }
            }
        }
    }
    out
}

/// The findings of `inv` not on the accepted list.
fn unaccepted(inv: &[String]) -> Vec<&String> {
    let list = accepted();
    inv.iter().filter(|i| !list.iter().any(|(p, _)| normalized(i).starts_with(p))).collect()
}

#[test]
fn engine_findings_are_one_per_field_and_matched_one_by_one() {
    let diff: Vec<String> =
        ["work_time[0].spent:0.5->0.0", "info.url:None->absent", "info.genre:x->y", "tempo:120->121"].map(String::from).into();
    let found = engine_findings(&diff);
    assert_eq!(found, ["engine-behind(work_time[0].spent:0.5->0.0(+0))", "engine-behind(info.url:None->absent(+1))", "engine-behind(tempo:120->121(+0))"]);
    let list = ["engine-behind(info.", "engine-behind(work_time"];
    let open: Vec<&String> = found.iter().filter(|f| !list.iter().any(|a| normalized(f).starts_with(a))).collect();
    assert_eq!(open, [&found[2]], "with the info and the work time accepted, the tempo is still found");
}

#[test]
fn findings_are_matched_against_the_accepted_list() {
    assert_eq!(normalized("undo-to-start(25:mixer[6].enabled:true->false(+1))"), "undo-to-start(mixer[].enabled:true->false(+1))");
    assert_eq!(normalized("roundtrip(mixer[0].slots[4][0].name:x)"), "roundtrip(mixer[].slots[][].name:x)");
    let inv = ["undo-to-start(3:mixer[2].enabled:true->false(+0))".to_string(), "roundtrip(mixer[1].eq[0][1]:1->2(+0))".to_string()];
    assert_eq!(unaccepted(&inv), [&inv[1]]);
}

#[test]
#[ignore = "the equivalence harness: records traces (see the module's doc)"]
fn equiv_record() {
    let (root, projects) = corpus();
    let dir = traces_dir();
    std::fs::create_dir_all(&dir).expect("traces folder");
    let (seeds, steps) = (var("OS_EQUIV_SEEDS", 2u64), var("OS_EQUIV_STEPS", 300usize));
    let _ = root;
    for p in &projects {
        for seed in var("OS_EQUIV_SEED_FROM", 1u64)..=seeds {
            let t0 = Instant::now();
            let r = isolated(&trace_name(p, seed), &format!("record\t{p}\t{seed}\t{steps}"));
            eprintln!("{p} seed {seed}: {r} in {:.1} s", t0.elapsed().as_secs_f64());
        }
    }
}

#[test]
#[ignore = "the equivalence harness: replays recorded traces (see the module's doc)"]
fn equiv_replay() {
    let (root, _) = corpus();
    let mut names: Vec<PathBuf> = std::fs::read_dir(traces_dir()).expect("traces").flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "trace")).collect();
    names.sort();
    let mut failed = 0;
    let _ = root;
    for n in &names {
        let name = n.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match isolated(&name, &format!("replay\t{}", n.display())).as_str() {
            "same" => eprintln!("same: {}", n.display()),
            r => {
                failed += 1;
                eprintln!("DIFFERENT: {}: {r}", n.display());
            }
        }
    }
    assert_eq!(failed, 0, "{failed} of {} traces differ", names.len());
}

#[test]
#[ignore = "the equivalence harness: what is not deterministic on one build (see the module's doc)"]
fn equiv_twice() {
    let (root, projects) = corpus();
    eprintln!("corpus {}: {} projects", root.display(), projects.len());
    let (seeds, steps) = (var("OS_EQUIV_SEEDS", 2u64), var("OS_EQUIV_STEPS", 300usize));
    let mut failed = 0;
    for p in &projects {
        for seed in var("OS_EQUIV_SEED_FROM", 1u64)..=seeds {
            let name = trace_name(p, seed);
            let recorded = isolated(&name, &format!("record\t{p}\t{seed}\t{steps}"));
            let path = traces_dir().join(format!("{name}.trace"));
            let t = Trace::read(&std::fs::read_to_string(&path).expect("recorded")).expect("a trace");
            assert_eq!(Trace::read(&t.write()).map(|a| a.write()), Some(t.write()), "the trace format round-trips");
            match isolated(&name, &format!("replay\t{}", path.display())).as_str() {
                "same" => eprintln!("deterministic: {p} seed {seed}, {recorded}"),
                r => {
                    failed += 1;
                    eprintln!("NOT DETERMINISTIC: {p} seed {seed}: {r}");
                }
            }
        }
    }
    assert_eq!(failed, 0);
}

/// Runs one recording, replay or shrink candidate (`job`: "record\t<project>\t<seed>\t<steps>", "replay\t<trace>" or
/// "check\t<project>\t<invariant>\t<file of steps>") in a process of
/// its own with a home folder of its own, so no trace sees what another left behind (settings, clipboards, the
/// process-wide switches): what it reports.
fn isolated(name: &str, job: &str) -> String {
    let home = traces_dir().join("homes").join(name);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::create_dir_all(&home);
    let bin = no_programs(&home);
    let out = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args(["equiv::equiv_one", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env("OS_EQUIV_ONE", job)
        .env("HOME", &home)
        // No audio device at all: a null device keeps no time, so its stream would spin a core for the whole trace.
        .env("ALSA_CONFIG_PATH", home.join("no-alsa.conf"))
        // Generated clicks reach Open folder, help links, the bell and plugin rebuilds: nothing the app starts may
        // reach the user's desktop (a file manager opened through D-Bus on 2026-11-08) or run a build.
        .env("PATH", &bin)
        .env("XDG_RUNTIME_DIR", home.join("run"))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        // No display either: whether a plugin window opens (and what the plugin's state then holds) must not depend on
        // whether a hidden X server happened to be running.
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("CARGO")
        .output();
    if let Ok(started) = std::fs::read_to_string(home.join("started")) {
        for l in started.lines() {
            eprintln!("STARTED {name}: {l}");
        }
    }
    let _ = std::fs::remove_dir_all(&home);
    let Ok(out) = out else { return "did not start".into() };
    let err = String::from_utf8_lossy(&out.stderr);
    err.lines().find_map(|l| l.strip_prefix("EQUIV-RESULT ")).map_or_else(
        || format!("no result ({})", err.lines().filter(|l| l.contains("panicked") || l.contains("error")).take(2).collect::<Vec<_>>().join(" / ")),
        String::from,
    )
}

/// The only programs a trace's process can start: stand-ins that note the call in `<home>/started` and do nothing.
/// Anything else the app tries to start is not found. What it returns is the trace's whole `PATH`.
fn no_programs(home: &std::path::Path) -> std::path::PathBuf {
    let bin = home.join("bin");
    let _ = std::fs::create_dir_all(&bin);
    let _ = std::fs::create_dir_all(home.join("run"));
    let log = home.join("started");
    for p in ["xdg-open", "open", "explorer", "pw-play", "cargo", "gio", "notify-send"] {
        let f = bin.join(p);
        let _ = std::fs::write(&f, format!("#!/bin/sh\necho \"{p} $*\" >> '{}'\n", log.display()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755));
        }
    }
    bin
}

/// The child of `isolated`: one recording (written to the traces folder), one replay or one shrink candidate.
#[test]
#[ignore = "the equivalence harness: one job of equiv_record or equiv_replay, in a process of its own"]
fn equiv_one() {
    let Ok(job) = std::env::var("OS_EQUIV_ONE") else { return };
    let (root, _) = corpus();
    let parts: Vec<&str> = job.split('\t').collect();
    let result = match parts.as_slice() {
        ["record", project, seed, steps] => {
            let (seed, steps) = (seed.parse().expect("seed"), steps.parse().expect("steps"));
            let t = record(&root, project, seed, steps);
            std::fs::write(traces_dir().join(format!("{}.trace", trace_name(project, seed))), t.write()).expect("write trace");
            let new = unaccepted(&t.invariants);
            format!("{} steps, findings {new:?} ({} accepted)", t.steps.len(), t.invariants.len() - new.len())
        }
        ["check", project, seed, kind, path] => {
            let steps: Option<Vec<Vec<Input>>> =
                std::fs::read_to_string(path).expect("read").lines().map(|l| l.split("; ").map(Input::parse).collect()).collect();
            fails_here(&root, project, seed.parse().expect("a seed"), &steps.expect("a candidate"), kind).map_or_else(|| "holds".into(), |f| format!("fails: {f}"))
        }
        ["replay", path] => {
            let t = Trace::read(&std::fs::read_to_string(path).expect("read")).expect("a trace");
            replay(&root, &t).unwrap_or_else(|| "same".into())
        }
        _ => format!("unknown job {job:?}"),
    };
    eprintln!("EQUIV-RESULT {}", result.replace('\n', " "));
}

/// Whether running `steps` on `project` breaks an invariant whose normalized name (`normalized`) starts with `kind`
/// and is not on the accepted list, in a process of its own as
/// a recording runs (settings and process-wide switches one candidate changes must not reach the next).
fn fails(_root: &Path, project: &str, seed: u64, steps: &[Vec<Input>], kind: &str) -> Option<String> {
    let file = traces_dir().join("shrink-candidate.txt");
    let text: String = steps.iter().map(|s| s.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ") + "\n").collect();
    std::fs::write(&file, text).expect("write the candidate");
    let r = isolated("shrink", &format!("check\t{project}\t{seed}\t{kind}\t{}", file.display()));
    r.strip_prefix("fails: ").map(String::from)
}

/// `fails` in this process, in the folder the trace's recording used (a save writes its path, so another folder can
/// change what is saved).
fn fails_here(root: &Path, project: &str, seed: u64, steps: &[Vec<Input>], kind: &str) -> Option<String> {
    let work = work_dir(&trace_name(project, seed));
    let mut rig = Rig::open(&root.join(project), &work);
    let mut start = rig.app.state.project.clone();
    for s in steps {
        rig.run(s);
        rig.undo_start(&mut start);
    }
    let list = accepted();
    rig.invariants(&start, &work).into_iter().find(|i| {
        let n = normalized(i);
        n.starts_with(kind) && !list.iter().any(|(a, _)| n.starts_with(a))
    })
}

/// Cuts `OS_EQUIV_TRACE` down to the steps that still give a finding starting with `OS_EQUIV_KIND`, accepted ones
/// left aside (delta debugging: drop
/// chunks of steps, halving their size, while it still fails), and prints what is left as a trace.
#[test]
#[ignore = "the equivalence harness: shrinks a trace to what breaks an invariant (see the module's doc)"]
fn equiv_shrink() {
    let (root, _) = corpus();
    let path = std::env::var("OS_EQUIV_TRACE").expect("OS_EQUIV_TRACE: a trace file");
    let kind = std::env::var("OS_EQUIV_KIND")
        .expect("OS_EQUIV_KIND: the start of a finding as the accepted list writes it, e.g. undo-to-start or roundtrip(arrangements[].clips");
    let t = Trace::read(&std::fs::read_to_string(&path).expect("read")).expect("a trace");
    let mut steps: Vec<Vec<Input>> = t.steps.into_iter().map(|(i, _)| i).collect();
    // The shortest failing prefix first: the steps after it play no part.
    let (mut lo, mut hi) = (0, steps.len());
    let Some(mut why) = fails(&root, &t.project, t.seed, &steps, &kind) else { panic!("the whole trace does not break {kind}") };
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        match fails(&root, &t.project, t.seed, &steps[..mid], &kind) {
            Some(w) => {
                hi = mid;
                why = w;
            }
            None => lo = mid,
        }
    }
    steps.truncate(hi);
    let mut chunk = steps.len().div_ceil(2);
    while chunk >= 1 {
        let mut i = 0;
        while i < steps.len() && steps.len() > 1 {
            let mut less = steps.clone();
            less.drain(i..(i + chunk).min(steps.len()));
            match fails(&root, &t.project, t.seed, &less, &kind) {
                Some(w) => {
                    steps = less;
                    why = w;
                }
                None => i += chunk,
            }
        }
        if chunk == 1 {
            break;
        }
        chunk = chunk.div_ceil(2);
    }
    eprintln!("{} steps of {} break it: {why}", steps.len(), t.project);
    for s in &steps {
        let line: Vec<String> = s.iter().map(ToString::to_string).collect();
        eprintln!("  {}", line.join("; "));
    }
}

/// Runs `OS_EQUIV_TRACE`'s steps and says after each what it landed on (`OS_EQUIV_SHOW`), the newest undo step, the
/// status message and the project's parts that changed, to read a shrunk trace.
#[test]
#[ignore = "the equivalence harness: shows what a trace's steps do (see the module's doc)"]
fn equiv_show() {
    let (root, _) = corpus();
    let path = std::env::var("OS_EQUIV_TRACE").expect("OS_EQUIV_TRACE: a trace file");
    let t = Trace::read(&std::fs::read_to_string(&path).expect("read")).expect("a trace");
    let work = work_dir(&trace_name(&t.project, t.seed));
    let mut rig = Rig::open(&root.join(&t.project), &work);
    let mut start = rig.app.state.project.clone();
    for (i, (inputs, _)) in t.steps.iter().enumerate() {
        let line: Vec<String> = inputs.iter().map(ToString::to_string).collect();
        eprintln!("step {i}: {}", line.join("; "));
        let before = rig.app.state.project.clone();
        rig.run(inputs);
        rig.undo_start(&mut start);
        let s = &rig.app.state;
        eprintln!("  undo: {:?} ({} steps), message: {:?}", s.history.newest_done(), s.history.done(), s.message);
        if let Some(d) = difference(&before, &s.project, false) {
            eprintln!("  changed: {d}");
        }
    }
    // OS_EQUIV_SHOW=checks: then the end checks, as a recording runs them.
    if std::env::var("OS_EQUIV_SHOW").as_deref() == Ok("checks") {
        for f in rig.invariants(&start, &work) {
            eprintln!("check: {f}");
        }
    }
    // OS_EQUIV_SHOW=save: then a saved copy, opened again, against the project (as the roundtrip check compares them,
    // every difference).
    if std::env::var("OS_EQUIV_SHOW").as_deref() == Ok("save") {
        let copy = work.join("roundtrip.flp");
        let mut written = rig.app.state.project.clone();
        match rig.app.save_copy_with(copy.clone(), &mut |p| written = p.clone()) {
            Some(Ok(())) => {
                let again = open_app(&copy);
                for d in differences(&canonical(&written), &canonical(&again.state.project), true).iter().take(60) {
                    eprintln!("saved and opened: {d}");
                }
                let (a, b) = (leaves(&side(&rig.app)), leaves(&side(&again)));
                for (x, y) in a.iter().zip(&b).filter(|(x, y)| x != y).take(20) {
                    eprintln!("saved and opened, interface: {}:{}->{}", x.0, short(&x.1), short(&y.1));
                }
            }
            other => eprintln!("save failed: {other:?}"),
        }
    }
}

/// What the recordings in `OS_EQUIV_DIR` reached (their `.kinds` files): the kinds of control shown but never aimed
/// at, aimed at but never changing the project, the interface's part of it or Lemon Studio's state, and those that did.
#[test]
#[ignore = "the equivalence harness: what the recordings reached (see the module's doc)"]
fn equiv_coverage() {
    let mut all: std::collections::BTreeMap<String, (u64, u64, u64, u64)> = Default::default();
    let mut traces = 0;
    for e in std::fs::read_dir(traces_dir()).expect("traces folder").flatten() {
        if e.path().extension().is_none_or(|x| x != "kinds") {
            continue;
        }
        traces += 1;
        for l in std::fs::read_to_string(e.path()).unwrap_or_default().lines() {
            let f: Vec<&str> = l.split('\t').collect();
            let [k, seen, used, edited] = f[..] else { continue };
            let a = all.entry(k.to_string()).or_default();
            a.0 += seen.parse::<u64>().unwrap_or(0);
            a.1 += used.parse::<u64>().unwrap_or(0);
            a.2 += u64::from(edited == "1");
            a.3 += 1;
        }
    }
    eprintln!("{traces} recordings, {} kinds of control shown", all.len());
    for (title, pick) in [
        ("shown, never aimed at", &(|a: &(u64, u64, u64, u64)| a.1 == 0) as &dyn Fn(&(u64, u64, u64, u64)) -> bool),
        ("aimed at, never changed anything", &|a| a.1 > 0 && a.2 == 0),
        ("changed something", &|a| a.2 > 0),
    ] {
        let rows: Vec<_> = all.iter().filter(|(_, a)| pick(a)).collect();
        eprintln!("\n{title}: {}", rows.len());
        for (k, (seen, used, edited, traces)) in rows {
            eprintln!("  {k}: shown in {seen} frames of {traces} recordings, aimed at {used} times, changed in {edited}");
        }
    }
}
