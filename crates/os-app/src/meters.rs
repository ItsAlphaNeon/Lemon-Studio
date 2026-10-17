// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Mixer peak meters: ballistics (rise, fall, peak hold), the level scale law and the colour zones. Every number lives
//! in one table (`PARAMS`; values tuned by eye against FL by the owner, 2026-10-10). "Alternate meter scale" maps a
//! level before the law (`alternate_db`).
//!
// spec: ui/mixer.md §9 (meters)

use crate::monitor::Param;

const fn p(name: &'static str, default: f32, min: f32, max: f32, decimals: usize, unit: &'static str) -> Param {
    Param { name, default, min, max, decimals, unit }
}

/// Indices into `Settings::v`.
pub mod k {
    pub const FALL: usize = 0;
    pub const FALL_MODE: usize = 1;
    pub const FALL_TIME: usize = 2;
    pub const RISE: usize = 3;
    pub const HOLD_ON: usize = 4;
    pub const HOLD_MS: usize = 5;
    pub const HOLD_FALL: usize = 6;
    pub const YELLOW_DB: usize = 7;
    pub const RED_DB: usize = 8;
    pub const FLOOR_DB: usize = 9;
    pub const GRADIENT: usize = 10;
    pub const EDGE: usize = 11;
    pub const CAP: usize = 12;
}

pub const PARAMS: [Param; 13] = [
    p("Fall speed", 51.0, 1.0, 600.0, 0, "dB/s"),
    p("Fall: dB/s (0) / exp (1)", 0.0, 0.0, 1.0, 0, ""),
    p("Fall time (exp)", 300.0, 5.0, 5000.0, 0, "ms"),
    p("Rise (1 = instant)", 1.0, 0.02, 1.0, 2, ""),
    p("Peak hold line", 1.0, 0.0, 1.0, 0, "on/off"),
    p("Peak hold time", 1500.0, 0.0, 10000.0, 0, "ms"),
    p("Peak hold fall", 24.0, 0.0, 600.0, 0, "dB/s"),
    p("Yellow from", -6.0, -40.0, 6.0, 1, "dB"),
    p("Red from", 0.0, -12.0, 6.0, 1, "dB"),
    p("Meter floor", -63.0, -120.0, -46.0, 0, "dB"),
    p("Colour gradient", 1.0, 0.0, 2.0, 2, "x"),
    p("Edge shade", 0.67, 0.0, 1.0, 2, "x"),
    p("Dark cap", 1.0, 0.0, 1.0, 0, "on/off"),
];

#[derive(Clone, PartialEq)]
pub struct Settings {
    pub v: [f32; PARAMS.len()],
}

impl Default for Settings {
    fn default() -> Self {
        let mut s = Settings { v: [0.0; PARAMS.len()] };
        (0..PARAMS.len()).for_each(|i| s.set(i, PARAMS[i].default));
        s
    }
}

impl Settings {
    pub fn set(&mut self, i: usize, x: f32) {
        let p = &PARAMS[i];
        let step = 10f32.powi(-(p.decimals as i32));
        self.v[i] = ((x.clamp(p.min, p.max) / step).round() * step).clamp(p.min, p.max);
    }
}

/// Below this a meter is empty.
pub const SILENT: f32 = -200.0;

/// One track's meter state, in dB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meter {
    pub db: [f32; 2],
    /// Peak-hold line level and how long it has been held (seconds).
    pub hold: [f32; 2],
    pub held: [f32; 2],
}

impl Default for Meter {
    fn default() -> Self {
        Meter { db: [SILENT; 2], hold: [SILENT; 2], held: [0.0; 2] }
    }
}

impl Meter {
    pub fn active(&self) -> bool {
        self.db.iter().chain(&self.hold).any(|&d| d > SILENT)
    }
}

pub fn to_db(v: f32) -> f32 {
    if v > 1e-10 { 20.0 * v.log10() } else { SILENT }
}

/// Advances one meter by `dt` seconds with the new block peaks `peak` (linear). True while it still moves.
pub fn step(m: &mut Meter, peak: [f32; 2], dt: f32, c: &Settings) -> bool {
    let floor = c.v[k::FLOOR_DB];
    for i in 0..2 {
        let p = to_db(peak[i]);
        let d = &mut m.db[i];
        if p >= *d {
            // Rise: 1 = jump to the peak, less = that fraction of the way per 1/60 s.
            let r = c.v[k::RISE];
            *d = if r >= 1.0 || *d <= SILENT { p } else { *d + (p - *d) * (1.0 - (1.0 - r).powf(dt * 60.0)) };
        } else {
            *d = if c.v[k::FALL_MODE] >= 0.5 {
                // Exponential in amplitude: the level falls to 1/e in the fall time.
                *d - 8.685_89 * dt / (c.v[k::FALL_TIME] / 1000.0).max(1e-3)
            } else {
                *d - c.v[k::FALL] * dt
            }
            .max(p);
        }
        if *d < floor {
            *d = SILENT;
        }
        // Peak hold: a new maximum restarts the hold time; afterwards the line falls.
        let level = m.db[i];
        if level >= m.hold[i] {
            m.hold[i] = level;
            m.held[i] = 0.0;
        } else {
            m.held[i] += dt;
            if m.held[i] * 1000.0 > c.v[k::HOLD_MS] {
                m.hold[i] = (m.hold[i] - c.v[k::HOLD_FALL] * dt).max(level);
            }
        }
        if m.hold[i] < floor {
            m.hold[i] = SILENT;
        }
    }
    m.active()
}

/// The level scale (label positions on the "C" column at the measured 1080p mixer, label centres; spec:
/// ui/main-window.md mixer scale) as (dB, y). Top of the column y 130, bottom y 924.
const LAW: [(f32, f32); 17] = [
    (3.0, 181.0), (0.0, 241.0), (-3.0, 301.0), (-6.0, 360.0), (-9.0, 419.0), (-12.0, 479.0), (-15.0, 535.0), (-18.0, 588.0),
    (-21.0, 639.0), (-24.0, 686.0), (-27.0, 728.0), (-30.0, 766.0), (-33.0, 799.0), (-36.0, 826.0), (-39.0, 848.0),
    (-42.0, 866.0), (-45.0, 880.0),
];
pub const LAW_TOP: f32 = 130.0;
pub const LAW_BOTTOM: f32 = 924.0;

/// The alternate scale ("Alternate meter scale", General ▸ Miscellaneous; spec: ui/settings-choices.md
/// "Miscellaneous: seven more checks, Default template and Startup project"): each alternate mark (dB) and the level
/// the default scale draws at its height, measured on both scales' tick rows.
pub const ALTERNATE: [(f32, f32); 11] = [
    (0.0, 0.0), (-3.0, -6.6), (-6.0, -12.6), (-9.0, -17.9), (-12.0, -22.8), (-15.0, -27.1), (-18.0, -31.1), (-21.0, -34.9),
    (-24.0, -38.4), (-27.0, -41.6), (-30.0, -45.0),
];

/// The level the default law draws level `db` as on the alternate scale (spec: ui/settings-choices.md
/// "Miscellaneous: seven more checks, Default template and Startup project"): above 0 dB the level as it is, between
/// the marks of `ALTERNATE` linearly, and from −30 dB down to `floor` linearly onto −45 dB down to `floor`. 0 dB
/// stays where it is; the top 30 dB take the room the top 45 dB take on the default scale.
pub fn alternate_db(db: f32, floor: f32) -> f32 {
    if db >= 0.0 || db <= floor {
        return db;
    }
    for w in ALTERNATE.windows(2) {
        let ((a0, d0), (a1, d1)) = (w[0], w[1]);
        if db <= a0 && db >= a1 {
            return d0 + (d1 - d0) * (a0 - db) / (a0 - a1);
        }
    }
    let (a, d) = ALTERNATE[ALTERNATE.len() - 1];
    d + (floor - d) * (a - db) / (a - floor).max(1e-3)
}

/// Reference y (C column, 1080p) of level `db`: the measured marks, 20 px/dB above +3 dB, a straight run from −45 dB
/// down to the floor at the column bottom. With `alt` ("Alternate meter scale") the level is mapped first
/// (`alternate_db`).
pub fn law_y(db: f32, floor: f32, alt: bool) -> f32 {
    let db = if alt { alternate_db(db, floor) } else { db };
    if db <= floor {
        return LAW_BOTTOM;
    }
    if db >= LAW[0].0 {
        return (LAW[0].1 - (db - LAW[0].0) * 20.0).max(LAW_TOP);
    }
    for w in LAW.windows(2) {
        let ((d0, y0), (d1, y1)) = (w[0], w[1]);
        if db <= d0 && db >= d1 {
            return y0 + (y1 - y0) * (d0 - db) / (d0 - d1);
        }
    }
    let (d, y) = LAW[LAW.len() - 1];
    y + (LAW_BOTTOM - y) * (d - db) / (d - floor).max(1e-3)
}

/// Height fraction of level `db` (0 = empty, 1 = full) on any meter that uses the scale (`alt`: the alternate scale).
pub fn frac(db: f32, floor: f32, alt: bool) -> f32 {
    ((LAW_BOTTOM - law_y(db, floor, alt)) / (LAW_BOTTOM - LAW_TOP)).clamp(0.0, 1.0)
}

/// Level at height fraction `f` (the scale's hover read-out; `alt`: the alternate scale).
pub fn db_at(f: f32, floor: f32, alt: bool) -> f32 {
    let (mut lo, mut hi) = (floor, 6.0f32);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if frac(mid, floor, alt) < f { lo = mid } else { hi = mid }
    }
    (lo + hi) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn law_and_ballistics() {
        let c = Settings::default();
        let f = |d| frac(d, c.v[k::FLOOR_DB], false);
        assert!(f(0.0) > f(-6.0) && f(-6.0) > f(-45.0) && f(-45.0) > 0.0);
        assert_eq!(f(-100.0), 0.0);
        assert!((db_at(f(-12.0), c.v[k::FLOOR_DB], false) + 12.0).abs() < 0.01);
        // Jumps up at once, falls at the set speed, holds the peak line.
        let mut m = Meter::default();
        step(&mut m, [1.0, 0.5], 0.016, &c);
        assert!((m.db[0]).abs() < 1e-4 && (m.db[1] + 6.02).abs() < 0.01);
        step(&mut m, [0.0, 0.0], 0.5, &c);
        assert!((m.db[0] + 25.5).abs() < 0.01, "51 dB/s for 0.5 s: {}", m.db[0]);
        assert_eq!(m.hold[0], 0.0, "hold line stays for its hold time");
        for _ in 0..200 {
            step(&mut m, [0.0, 0.0], 0.05, &c);
        }
        assert!(!m.active(), "everything falls back to empty");
    }

    // spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and Startup project"
    /// "Alternate meter scale": a level is mapped through the measured table, then drawn with the default law. 0 dB
    /// stays put, −30 dB sits where −45 dB does by default, levels above 0 dB and at the floor are unchanged, and the
    /// hover read-out inverts the mapping.
    #[test]
    fn alternate_scale() {
        let floor = Settings::default().v[k::FLOOR_DB];
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        for &(mark, default) in &ALTERNATE {
            assert!(near(alternate_db(mark, floor), default), "{mark} dB -> {default} dB");
            assert!(near(frac(mark, floor, true), frac(default, floor, false)), "{mark} dB at the height of {default} dB");
        }
        assert!(near(alternate_db(-4.5, floor), -9.6), "between marks: linear");
        assert!(near(alternate_db(2.0, floor), 2.0) && near(frac(2.0, floor, true), frac(2.0, floor, false)), "above 0 dB");
        assert!(near(frac(0.0, floor, true), frac(0.0, floor, false)), "0 dB stays at the same height");
        assert!(near(alternate_db(floor, floor), floor), "the floor stays the floor");
        assert!(near(alternate_db((floor - 30.0) / 2.0, floor), (floor - 45.0) / 2.0), "-30 .. floor onto -45 .. floor");
        assert_eq!(frac(-100.0, floor, true), 0.0);
        // Below 0 dB a level sits lower than on the default scale, and the read-out gives the level back.
        assert!(frac(-12.0, floor, true) < frac(-12.0, floor, false));
        for db in [-1.0, -12.0, -29.0, -40.0] {
            assert!((db_at(frac(db, floor, true), floor, true) - db).abs() < 0.01, "{db} dB read back");
        }
    }
}
