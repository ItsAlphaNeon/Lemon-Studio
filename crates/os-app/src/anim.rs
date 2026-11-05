// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! General ▸ Display ▸ Animations: the one level every animation of the interface reads, and the four motions they
//! are built from.
//!
//! - **The level** (`Level`): Off, Subtle, Polished, Playful, low to high. Each animation is either *feedback* (it
//!   tells the user what they just did: shown at every level, at once at Off) or *decoration* (hidden below its own
//!   lowest level). The level is global and applies at once: painting reads it every frame.
//! - **Time, not frames.** Every motion is a function of the time since it began (`Tween`) or is stepped by the real
//!   time since the last frame (`follow`, `Spring`, `Pendulum`), so Animation refresh rate changes only how many
//!   frames are seen, never how long anything takes (`smooth::frame_cap`).
//! - **Idle when still.** Each animation says whether it still moves; the shell ticks at animation rate only while
//!   one does (`App::animating`).
// spec: ui/animations.md

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// General ▸ Display ▸ Animations, low to high (spec: ui/animations.md §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// No decoration; feedback shows at once.
    Off = 0,
    /// Short fades.
    Subtle = 1,
    /// Every motion, critically damped: nothing overshoots.
    Polished = 2,
    /// Every motion with springs that overshoot and sway (the default).
    Playful = 3,
}

impl Level {
    /// The combo's order, as FL lists its levels: highest first.
    pub const MENU: [Level; 4] = [Level::Playful, Level::Polished, Level::Subtle, Level::Off];

    pub fn name(self) -> &'static str {
        match self {
            Level::Off => "Off",
            Level::Subtle => "Subtle",
            Level::Polished => "Polished",
            Level::Playful => "Playful",
        }
    }

    pub fn from_name(s: &str) -> Option<Level> {
        Level::MENU.into_iter().find(|l| l.name().eq_ignore_ascii_case(s.trim()))
    }

    fn from_u8(v: u8) -> Level {
        match v {
            0 => Level::Off,
            1 => Level::Subtle,
            2 => Level::Polished,
            _ => Level::Playful,
        }
    }

    /// How long a timed animation lasts at this level, as a share of its full length: Off snaps to the end, Subtle
    /// takes 60 %.
    pub fn time_scale(self) -> f64 {
        match self {
            Level::Off => 0.0,
            Level::Subtle => 0.6,
            Level::Polished | Level::Playful => 1.0,
        }
    }

    /// A timed animation of `full` at this level.
    pub fn dur(self, full: Duration) -> Duration {
        full.mul_f64(self.time_scale())
    }

    /// The damping of springs that may overshoot: Playful sways, Polished settles without overshoot.
    pub fn zeta(self, playful: f32) -> f32 {
        if self == Level::Playful { playful } else { 1.0 }
    }
}

#[cfg(not(test))]
static LEVEL: AtomicU8 = AtomicU8::new(Level::Playful as u8);

// Tests run in parallel threads: each has its own level, as each has its own settings (`settings_file`).
#[cfg(test)]
thread_local! {
    static LEVEL: AtomicU8 = const { AtomicU8::new(Level::Playful as u8) };
}

/// The level now.
pub fn level() -> Level {
    #[cfg(not(test))]
    let v = LEVEL.load(Ordering::Relaxed);
    #[cfg(test)]
    let v = LEVEL.with(|l| l.load(Ordering::Relaxed));
    Level::from_u8(v)
}

/// Sets the level; every animation reads it from its next frame on.
pub fn set_level(l: Level) {
    #[cfg(not(test))]
    LEVEL.store(l as u8, Ordering::Relaxed);
    #[cfg(test)]
    LEVEL.with(|v| v.store(l as u8, Ordering::Relaxed));
}

/// The level kept in `animations.txt` in the user folder, Playful when there is none (FL's default is its highest
/// level). The file once held "Playful" or "Off" only; both still read. Tests always run at Playful unless they set
/// another level.
pub fn load() -> Level {
    if cfg!(test) {
        return Level::Playful;
    }
    os_paths::user_file("animations.txt")
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| Level::from_name(&t))
        .unwrap_or(Level::Playful)
}

/// Keeps the level for the next start.
pub fn save(l: Level) {
    if cfg!(test) {
        return;
    }
    if let Some(dir) = os_paths::user_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("animations.txt"), l.name());
    }
}

/// Whether the mixer's cables swing at `l` (decoration, like the drag tag: from Polished up). Below, they hang still in
/// their resting shapes (`crate::cable`).
pub fn cables_swing(l: Level) -> bool {
    l >= Level::Polished
}

// Tests see motion only on a clock they run themselves (`set_clock`, `advance`); without one, every motion is at its
// end at once, so a frame painted twice is the same frame.
#[cfg(test)]
thread_local! {
    static CLOCK: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// The time the interface's motions read.
pub fn now() -> Instant {
    #[cfg(test)]
    if let Some(t) = CLOCK.with(|c| c.get()) {
        return t;
    }
    Instant::now()
}

/// Whether motions jump to their ends: at Off (feedback still shows, at once), and in tests that run no clock.
pub fn instant() -> bool {
    still() || level() == Level::Off
}

/// Tests that run no clock: nothing moves and nothing flashes, so painting the same state twice paints the same frame.
pub fn still() -> bool {
    #[cfg(test)]
    return CLOCK.with(|c| c.get()).is_none();
    #[cfg(not(test))]
    false
}

/// Tests: starts this thread's clock at the present (`true`) or stops it (`false`: motions end at once again).
#[cfg(test)]
pub fn set_clock(on: bool) {
    CLOCK.with(|c| c.set(on.then(Instant::now)));
}

/// Tests: runs this thread's clock from `t` (back on after `set_clock(false)`, where it was).
#[cfg(test)]
pub fn set_clock_at(t: Instant) {
    CLOCK.with(|c| c.set(Some(t)));
}

/// Tests: moves this thread's clock on by `ms`.
#[cfg(test)]
pub fn advance(ms: u64) {
    CLOCK.with(|c| c.set(Some(c.get().unwrap_or_else(Instant::now) + Duration::from_millis(ms))));
}

/// Ease-out cubic: fast at first, gentle at the end. `u` from 0 to 1.
pub fn ease_out(u: f64) -> f64 {
    let v = 1.0 - u.clamp(0.0, 1.0);
    1.0 - v * v * v
}

/// A timed move from 0 to 1: begun at `at`, `dur` long.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tween {
    pub at: Instant,
    pub dur: Duration,
}

impl Tween {
    pub fn new(at: Instant, dur: Duration) -> Tween {
        Tween { at, dur }
    }

    /// How far it is at `now`, 0 to 1 (1 at once when it has no length).
    pub fn progress(&self, now: Instant) -> f64 {
        if self.dur.is_zero() {
            return 1.0;
        }
        (now.saturating_duration_since(self.at).as_secs_f64() / self.dur.as_secs_f64()).clamp(0.0, 1.0)
    }

    /// `progress` through the ease-out curve.
    pub fn eased(&self, now: Instant) -> f64 {
        ease_out(self.progress(now))
    }

    pub fn done(&self, now: Instant) -> bool {
        self.progress(now) >= 1.0
    }
}

/// One step of an exponential follow: `x` moves toward `target` by `1 - e^(-dt / tau)` of the way. At `tau` 0 it is
/// there at once.
pub fn follow(x: f64, target: f64, dt: f64, tau: f64) -> f64 {
    if tau <= 0.0 { target } else { x + (target - x) * (1.0 - (-dt / tau).exp()) }
}

/// A damped spring pulling `x` toward `target`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub x: f64,
    pub v: f64,
    pub target: f64,
}

impl Spring {
    pub fn at(x: f64) -> Spring {
        Spring { x, v: 0.0, target: x }
    }

    /// Moves on by `dt` seconds at natural frequency `hz` and damping ratio `zeta` (1 = critically damped). Returns
    /// whether it has come to rest (then it sits exactly on the target). Stepped in substeps of at most 4 ms, so a
    /// slow frame can't make it unstable.
    pub fn step(&mut self, dt: f64, hz: f64, zeta: f64, rest: f64) -> bool {
        let w = std::f64::consts::TAU * hz;
        let n = (dt / 0.004).ceil().max(1.0) as usize;
        let h = dt / n as f64;
        for _ in 0..n {
            let a = -w * w * (self.x - self.target) - 2.0 * zeta * w * self.v;
            self.v += a * h;
            self.x += self.v * h;
        }
        let still = (self.x - self.target).abs() < rest && self.v.abs() < rest * w;
        if still {
            self.x = self.target;
            self.v = 0.0;
        }
        still
    }

    #[cfg(test)]
    pub fn moving(&self) -> bool {
        self.x != self.target || self.v != 0.0
    }
}

/// The drag tag: a card hanging from a pivot that follows the mouse, swinging like a damped pendulum driven by the
/// mouse's sideways motion (spec: ui/animations.md §4.1). `theta` is the card's angle from hanging straight down,
/// positive with its bottom to the right.
#[derive(Debug, Clone, PartialEq)]
pub struct Pendulum {
    pub theta: f64,
    pub omega: f64,
    /// The mouse's sideways velocity and acceleration, low-passed (px/s, px/s²).
    vx: f64,
    ax: f64,
    last_x: Option<f64>,
    /// Swings a second, damping ratio, pivot to centre (px), how far a steady move leans it (1/s), and how much a
    /// change of speed kicks it (a share of the mouse's acceleration).
    pub hz: f64,
    pub zeta: f64,
    pub len: f64,
    pub lean: f64,
    pub kick: f64,
}

/// The tag never swings further than this from straight down.
pub const MAX_SWING: f64 = 40.0 * std::f64::consts::PI / 180.0;

impl Pendulum {
    /// A still tag at `level`. Playful: 1.6 swings a second, damping 0.2 and a small kick from the mouse's changes of
    /// speed, so a stop swings it past straight down and it sways out. Polished: critically damped and no kick, so it
    /// leans while the mouse moves and comes back without swinging past.
    pub fn new(level: Level) -> Pendulum {
        let playful = level == Level::Playful;
        Pendulum {
            theta: 0.0,
            omega: 0.0,
            vx: 0.0,
            ax: 0.0,
            last_x: None,
            hz: 1.6,
            zeta: f64::from(level.zeta(0.2)),
            len: 34.0,
            lean: 1.25,
            kick: if playful { 0.05 } else { 0.0 },
        }
    }

    /// The mouse is at `x` after `dt` seconds.
    pub fn update(&mut self, x: f64, dt: f64) {
        if dt <= 0.0 {
            self.last_x.get_or_insert(x);
            return;
        }
        // A long stall (a slow frame, the window in the background) would kick the tag: step at most 50 ms.
        let dt = dt.min(0.05);
        let raw = self.last_x.map_or(0.0, |l| (x - l) / dt);
        self.last_x = Some(x);
        // 30 ms low-pass on the velocity and on the acceleration taken from it.
        let k = 1.0 - (-dt / 0.03).exp();
        let v = self.vx + (raw - self.vx) * k;
        self.ax += ((v - self.vx) / dt - self.ax) * k;
        self.vx = v;
        let w = std::f64::consts::TAU * self.hz;
        let n = (dt / 0.004).ceil().max(1.0) as usize;
        let h = dt / n as f64;
        for _ in 0..n {
            let drive = (self.kick * self.ax + self.lean * self.vx) / self.len;
            let acc = -w * w * self.theta.sin() - 2.0 * self.zeta * w * self.omega - drive * self.theta.cos();
            self.omega += acc * h;
            self.theta += self.omega * h;
        }
        if self.theta.abs() > MAX_SWING {
            self.theta = MAX_SWING.copysign(self.theta);
            self.omega *= -0.3;
        }
    }

    /// Whether it still moves (the mouse has stopped and the tag hangs still: no more frames are needed).
    pub fn moving(&self) -> bool {
        self.theta.abs() > 0.005 || self.omega.abs() > 0.02 || self.vx.abs() > 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_order_and_names() {
        assert!(Level::Off < Level::Subtle && Level::Subtle < Level::Polished && Level::Polished < Level::Playful);
        for l in Level::MENU {
            assert_eq!(Level::from_name(l.name()), Some(l));
        }
        // The old file's two values.
        assert_eq!((Level::from_name("Playful\n"), Level::from_name("Off")), (Some(Level::Playful), Some(Level::Off)));
        assert_eq!(Level::from_name("Entertain me"), None);
        assert_eq!(Level::Off.dur(Duration::from_millis(120)), Duration::ZERO);
        assert_eq!(Level::Subtle.dur(Duration::from_millis(100)), Duration::from_millis(60));
        assert_eq!(Level::Playful.dur(Duration::from_millis(120)), Duration::from_millis(120));
    }

    #[test]
    fn tween_eases_out_by_time() {
        let t0 = Instant::now();
        let t = Tween::new(t0, Duration::from_millis(120));
        assert_eq!(t.progress(t0), 0.0);
        assert!((t.eased(t0 + Duration::from_millis(40)) - (1.0 - (2.0f64 / 3.0).powi(3))).abs() < 1e-9);
        assert!(t.eased(t0 + Duration::from_millis(40)) > 0.6, "ease-out: most of the way in the first third");
        assert!(t.done(t0 + Duration::from_millis(120)) && !t.done(t0 + Duration::from_millis(119)));
        assert!(Tween::new(t0, Duration::ZERO).done(t0), "no length: at the end at once");
    }

    /// The same follow over the same time ends in the same place at any frame rate.
    #[test]
    fn follow_is_frame_rate_independent() {
        let run = |dt: f64| {
            let mut x = 0.0;
            let mut t = 0.0;
            while t < 0.1 - 1e-9 {
                x = follow(x, 1.0, dt, 0.06);
                t += dt;
            }
            x
        };
        let (a, b, c) = (run(1.0 / 180.0), run(1.0 / 60.0), run(1.0 / 40.0));
        assert!((a - b).abs() < 1e-9 && (a - c).abs() < 1e-9, "{a} {b} {c}");
        assert_eq!(follow(0.0, 5.0, 0.01, 0.0), 5.0);
    }

    /// Critically damped: no overshoot. Damping 0.12: overshoots, then settles.
    #[test]
    fn spring_damping() {
        let mut s = Spring::at(0.0);
        s.target = 100.0;
        let mut max: f64 = 0.0;
        for _ in 0..200 {
            s.step(1.0 / 60.0, 8.0, 1.0, 0.05);
            max = max.max(s.x);
        }
        assert!(max <= 100.0 + 1e-6 && s.x == 100.0, "critically damped: {max}");
        let mut s = Spring::at(0.0);
        s.target = 100.0;
        let mut max: f64 = 0.0;
        let mut settled = None;
        for i in 0..600 {
            if s.step(1.0 / 60.0, 2.0, 0.12, 0.05) && settled.is_none() {
                settled = Some(i);
            }
            max = max.max(s.x);
        }
        assert!(max > 130.0, "overshoots: {max}");
        assert!(settled.is_some() && !s.moving());
    }

    /// Mouse right at a steady 1000 px/s: the tag leans left (its bottom trails) by about 20 degrees; stopped, it swings
    /// past straight down and sways out in a few swings at Playful, and comes back without crossing at Polished.
    #[test]
    fn pendulum_leans_then_sways() {
        let dt = 1.0 / 120.0;
        let run = |level: Level| {
            let mut p = Pendulum::new(level);
            let mut x = 0.0;
            p.update(x, 0.0);
            for _ in 0..60 {
                x += 1000.0 * dt;
                p.update(x, dt);
            }
            let lean = p.theta.to_degrees();
            // Visible swings: past straight down by more than 2 degrees.
            let mut crossings = 0;
            let mut sign = p.theta.signum();
            let mut max_after: f64 = 0.0;
            let mut still_at = None;
            for i in 0..600 {
                p.update(x, dt);
                if p.theta.signum() != sign && p.theta.abs() > 2f64.to_radians() {
                    crossings += 1;
                    sign = p.theta.signum();
                }
                if p.theta > 0.0 {
                    max_after = max_after.max(p.theta.to_degrees());
                }
                if still_at.is_none() && !p.moving() {
                    still_at = Some(i as f64 * dt);
                }
            }
            (lean, crossings, max_after, still_at)
        };
        let (lean, crossings, past, still) = run(Level::Playful);
        assert!((-30.0..=-15.0).contains(&lean), "leans left: {lean}");
        assert!((2..=8).contains(&crossings), "sways: {crossings} crossings");
        assert!(past > 5.0, "swings past straight down: {past}");
        assert!(still.is_some_and(|t| t < 3.0), "comes to rest: {still:?}");
        let (lean, crossings, past, _) = run(Level::Polished);
        assert!(lean < -10.0, "leans: {lean}");
        assert!(crossings == 0 && past < 2.0, "no sway: {crossings}, {past}");
    }

    #[test]
    fn pendulum_is_clamped_and_survives_stalls() {
        let mut p = Pendulum::new(Level::Playful);
        p.update(0.0, 0.0);
        p.update(5000.0, 0.008);
        p.update(10000.0, 0.008);
        p.update(10000.0, 3.0);
        assert!(p.theta.abs() <= MAX_SWING + 1e-9 && p.theta.is_finite());
    }
}
