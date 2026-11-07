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
//! Run with a home folder of its own (the app saves projects, backups and settings), outside the sandbox and on the
//! hidden display (`hidden-tests`):
//! ```text
//! HOME=<scratch home> OS_EQUIV_DIR=<traces> OS_EQUIV_CORPUS=<folder of .flp> \
//!   cargo test -p os-app --bin lemonstudio equiv_record -- --ignored --nocapture
//! ```
//! `equiv_replay` replays every trace in `OS_EQUIV_DIR`; `equiv_twice` records and replays on the same build (what
//! isn't deterministic yet). `OS_EQUIV_SEEDS` (2) traces per project, `OS_EQUIV_STEPS` (150) steps each,
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

/// Project state the interface holds until save (specs: control-plane.md part 2, "Where main is"), as one value.
fn side(app: &App) -> String {
    let (s, r) = (&app.state, &app.state.rack);
    let v = (s.tempo.to_bits(), s.pattern, s.song_mode, &r.filter, &r.groups, &r.zipped, &r.locked, r.loop_mode, &r.loop_len, &r.loop_unit, r.loop_type, r.unsorted_at);
    format!("{v:#?}")
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
}

/// A control's kind for the generator's novelty: its action's variant, and the variant inside it.
fn kind(a: &Action) -> String {
    let d = format!("{a:?}");
    let mut parts = d.split(['(', ' ', '{', ',']).filter(|p| p.chars().next().is_some_and(char::is_uppercase));
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
        let mut rig = Rig { app, canvas: os_ui::Canvas::new(FRAME.0, FRAME.1, 1.0).expect("a canvas"), step: 0, used: std::collections::BTreeMap::new() };
        rig.settle();
        // Mixer effects are hosted a few ticks after loading.
        for _ in 0..20 {
            rig.tick();
        }
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
        self.step += 1;
        let fp = Fp::of(&self.app);
        // Each step's "unsaved" says whether that step made a change to save, not whether any step before it did.
        self.app.mark_saved();
        fp
    }

    /// Where the generator may aim: the centres of the hits the last frame registered, on the frame.
    fn targets(&self) -> Vec<(f32, f32, String)> {
        let (w, h) = (FRAME.0 as f32, FRAME.1 as f32);
        self.app
            .hits
            .iter()
            .filter(|h| h.rect.w >= 1.0 && h.rect.h >= 1.0)
            .map(|h| (h.rect.x + h.rect.w / 2.0, h.rect.y + h.rect.h / 2.0, kind(&h.action)))
            .filter(|&(x, y, _)| (0.0..w).contains(&x) && (0.0..h).contains(&y))
            .collect()
    }

    /// The next step's inputs, aimed at what the frame shows.
    fn generate(&mut self, rng: &mut Rng) -> Vec<Input> {
        let targets = self.targets();
        // Weighted by novelty: a kind of control aimed at n times weighs 1/(n+1), so the many step buttons or grid
        // cells of one kind don't crowd out the few knobs, boxes and menus.
        let weight = |k: &str| 1_000_000 / (1 + u64::from(self.used.get(k).copied().unwrap_or(0)));
        let total: u64 = targets.iter().map(|t| weight(&t.2)).sum();
        if total == 0 {
            return vec![Input::Key("Escape".into(), false, false)];
        }
        let mut at = rng.next() % total;
        let Some((x, y, k)) = targets.iter().find(|t| {
            let w = weight(&t.2);
            if at < w {
                return true;
            }
            at -= w;
            false
        }).cloned() else {
            return vec![Input::Key("Escape".into(), false, false)];
        };
        *self.used.entry(k).or_default() += 1;
        let (shift, ctrl, alt) = (rng.chance(12), rng.chance(12), rng.chance(5));
        let mods = Input::Mods(shift, ctrl, alt);
        let none = Input::Mods(false, false, false);
        let roll = rng.below(100);
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
        // The engine holds what the project says (an edit that never reached it shows here).
        if let Some(sent) = self.app.transport.sent_for_tests() {
            if sent != crate::rack::play_project(&self.app.state) {
                bad.push("engine-behind".into());
            }
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
        if let Some(d) = difference(opened, &self.app.state.project, false) {
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
        if let Some(d) = difference(&last, &self.app.state.project, false) {
            bad.push(format!("redo-to-end({d})"));
        }
        let now_side = side(&self.app);
        if now_side != last_side {
            let (a, b) = (leaves(&last_side), leaves(&now_side));
            let d = a.iter().zip(&b).find(|(x, y)| x != y).map_or("length".into(), |(x, y)| format!("{}:{}->{}", x.0, short(&x.1), short(&y.1)));
            bad.push(format!("redo-to-end(side:{d})"));
        }
        // A saved copy opens as the same project: the project as save writes it (the tempo, song mode and rack state
        // the interface holds go in at save) against the one opened again, and the interface's part of it too.
        let copy = work.join("roundtrip.flp");
        match self.app.save_copy_with(copy.clone(), &mut |_| {}) {
            Some(Ok(())) => {
                let mut again = open_app(&copy);
                let mut written = self.app.state.project.clone();
                let s = &self.app.state;
                written.tempo = Some(s.tempo);
                written.song_mode = Some(s.song_mode);
                if let Some(d) = difference(&written, &again.state.project, true) {
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
    diff.first().map(|d| format!("{d}(+{})", diff.len() - 1))
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
    for _ in 0..steps {
        let inputs = rig.generate(&mut rng);
        let fp = rig.run(&inputs);
        rig.undo_start(&mut opened_project);
        out.push((inputs, fp));
    }
    let invariants = rig.invariants(&opened_project, &work);
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
fn findings_are_matched_against_the_accepted_list() {
    assert_eq!(normalized("undo-to-start(25:mixer[6].enabled:true->false(+1))"), "undo-to-start(mixer[].enabled:true->false(+1))");
    assert_eq!(normalized("roundtrip(mixer[0].slots[4][0].name:x)"), "roundtrip(mixer[].slots[][].name:x)");
    let inv = ["undo-to-start(3:mixer[2].enabled:true->false(+0))".to_string(), "resave(tempo[0]:1->2(+0))".to_string()];
    assert_eq!(unaccepted(&inv), [&inv[1]]);
}

#[test]
#[ignore = "the equivalence harness: records traces (see the module's doc)"]
fn equiv_record() {
    let (root, projects) = corpus();
    let dir = traces_dir();
    std::fs::create_dir_all(&dir).expect("traces folder");
    let (seeds, steps) = (var("OS_EQUIV_SEEDS", 2u64), var("OS_EQUIV_STEPS", 150usize));
    for p in &projects {
        for seed in var("OS_EQUIV_SEED_FROM", 1u64)..=seeds {
            let t0 = Instant::now();
            let t = record(&root, p, seed, steps);
            std::fs::write(dir.join(format!("{}.trace", trace_name(p, seed))), t.write()).expect("write trace");
            let new = unaccepted(&t.invariants);
            eprintln!("{p} seed {seed}: {} steps in {:.1} s, findings {new:?} ({} accepted)", t.steps.len(), t0.elapsed().as_secs_f64(), t.invariants.len() - new.len());
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
    for n in &names {
        let t = Trace::read(&std::fs::read_to_string(n).expect("read")).expect("a trace");
        match replay(&root, &t) {
            None => eprintln!("same: {}", n.display()),
            Some(r) => {
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
    let (seeds, steps) = (var("OS_EQUIV_SEEDS", 2u64), var("OS_EQUIV_STEPS", 150usize));
    let mut failed = 0;
    for p in &projects {
        for seed in var("OS_EQUIV_SEED_FROM", 1u64)..=seeds {
            let t = record(&root, p, seed, steps);
            let again = Trace::read(&t.write()).expect("reads back");
            assert_eq!(again.write(), t.write(), "the trace format round-trips");
            if let Some(r) = replay(&root, &again) {
                failed += 1;
                eprintln!("NOT DETERMINISTIC: {p} seed {seed}: {r}");
            } else {
                eprintln!("deterministic: {p} seed {seed}, invariants {:?}", t.invariants);
            }
        }
    }
    assert_eq!(failed, 0);
}

/// Whether running `steps` on `project` breaks an invariant whose name starts with `kind`.
fn fails(root: &Path, project: &str, steps: &[Vec<Input>], kind: &str) -> Option<String> {
    let work = work_dir("shrink");
    let mut rig = Rig::open(&root.join(project), &work);
    let mut start = rig.app.state.project.clone();
    for s in steps {
        rig.run(s);
        rig.undo_start(&mut start);
    }
    rig.invariants(&start, &work).into_iter().find(|i| i.starts_with(kind))
}

/// Cuts `OS_EQUIV_TRACE` down to the steps that still break invariant `OS_EQUIV_KIND` (delta debugging: drop
/// chunks of steps, halving their size, while it still fails), and prints what is left as a trace.
#[test]
#[ignore = "the equivalence harness: shrinks a trace to what breaks an invariant (see the module's doc)"]
fn equiv_shrink() {
    let (root, _) = corpus();
    let path = std::env::var("OS_EQUIV_TRACE").expect("OS_EQUIV_TRACE: a trace file");
    let kind = std::env::var("OS_EQUIV_KIND").expect("OS_EQUIV_KIND: the invariant's name, e.g. undo-to-start");
    let t = Trace::read(&std::fs::read_to_string(&path).expect("read")).expect("a trace");
    let mut steps: Vec<Vec<Input>> = t.steps.into_iter().map(|(i, _)| i).collect();
    // The shortest failing prefix first: the steps after it play no part.
    let (mut lo, mut hi) = (0, steps.len());
    let Some(mut why) = fails(&root, &t.project, &steps, &kind) else { panic!("the whole trace does not break {kind}") };
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        match fails(&root, &t.project, &steps[..mid], &kind) {
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
            match fails(&root, &t.project, &less, &kind) {
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
    let mut rig = Rig::open(&root.join(&t.project), &work_dir("show"));
    for (i, (inputs, _)) in t.steps.iter().enumerate() {
        let line: Vec<String> = inputs.iter().map(ToString::to_string).collect();
        eprintln!("step {i}: {}", line.join("; "));
        let before = rig.app.state.project.clone();
        rig.run(inputs);
        let s = &rig.app.state;
        eprintln!("  undo: {:?} ({} steps), message: {:?}", s.history.newest_done(), s.history.done(), s.message);
        if let Some(d) = difference(&before, &s.project, false) {
            eprintln!("  changed: {d}");
        }
    }
}
