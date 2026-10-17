// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Plugin performance monitor's readings: every hosted instance's processing time per second, its peak and total
//! since the measurements were reset, and an instrument's voices. The window is `views/perf.rs`.
// spec: ui/plugin-performance.md

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use os_host::Meter;

/// The list's columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Plugin,
    Type,
    Share,
    Time,
    Peak,
    Total,
    Voices,
}

impl Col {
    pub fn title(self) -> &'static str {
        match self {
            Col::Plugin => "Plugin",
            Col::Type => "Type",
            Col::Share => "%",
            Col::Time => "Time",
            Col::Peak => "Peak",
            Col::Total => "Total",
            Col::Voices => "Voices",
        }
    }

    /// Numbers sort largest first, text A to Z (§2).
    fn numeric(self) -> bool {
        !matches!(self, Col::Plugin | Col::Type)
    }
}

/// What the window's controls do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PerfAct {
    Sort(Col),
    /// A row of the shown list (a double click opens the plugin's window).
    Row(usize),
    /// The list area (wheel scrolling).
    List,
    ResetOnTransport,
    Freeze,
    Find,
}

/// One hosted instance as the app lists it.
pub struct Source {
    /// The `Plugins::hosted` key.
    pub key: usize,
    pub name: String,
    /// The preset it last loaded or saved.
    pub patch: Option<String>,
    /// Its channel, or "Insert N, slot M".
    pub place: String,
    pub synth: bool,
    pub stopped: bool,
    pub meter: Option<Arc<Meter>>,
}

/// A row of the list. Times in microseconds.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: usize,
    pub name: String,
    pub patch: Option<String>,
    pub place: String,
    pub synth: bool,
    pub stopped: bool,
    /// Processing time per second over the last reading.
    pub time: f64,
    pub peak: f64,
    pub total: f64,
    /// An instrument's voices.
    pub voices: Option<usize>,
}

impl Row {
    /// Share of real time, percent.
    pub fn share(&self) -> f64 {
        self.time / 1e6 * 100.0
    }

    /// The Plugin column's name part: "name - patch".
    pub fn title(&self) -> String {
        match &self.patch {
            Some(p) => format!("{} - {p}", self.name),
            None => self.name.clone(),
        }
    }

    /// The text Find searches: name, patch and place.
    fn text(&self) -> String {
        format!("{} {}", self.title(), self.place).to_lowercase()
    }

    fn value(&self, c: Col) -> f64 {
        match c {
            Col::Share | Col::Time => self.time,
            Col::Peak => self.peak,
            Col::Total => self.total,
            Col::Voices => self.voices.map_or(-1.0, |v| v as f64),
            Col::Plugin | Col::Type => 0.0,
        }
    }
}

/// A plugin's processing time: busy nanoseconds at the last reading and at the reset, the last Time and the peak.
#[derive(Default)]
struct Stat {
    last: u64,
    base: u64,
    time: f64,
    peak: f64,
}

pub struct Perf {
    /// Top-left corner (logical px); placed in the middle of the workspace when it first opens.
    pub pos: (f32, f32),
    pub placed: bool,
    pub reset_on_transport: bool,
    pub freeze: bool,
    pub find: String,
    /// The Find box takes the typing.
    pub finding: bool,
    pub sort: Col,
    pub desc: bool,
    /// First row shown.
    pub scroll: usize,
    /// The list as last read (all instances, sorted; Find filters it when shown).
    pub rows: Vec<Row>,
    /// The last click on a row (its key, when), for double clicks.
    pub last_click: Option<(usize, Instant)>,
    stats: BTreeMap<usize, Stat>,
    at: Option<Instant>,
}

impl Default for Perf {
    fn default() -> Self {
        Perf {
            pos: (0.0, 0.0),
            placed: false,
            reset_on_transport: false,
            freeze: false,
            find: String::new(),
            finding: false,
            sort: Col::Time,
            desc: true,
            scroll: 0,
            rows: Vec::new(),
            last_click: None,
            stats: BTreeMap::new(),
            at: None,
        }
    }
}

impl Perf {
    /// Takes a reading of `sources` (in rack and mixer order). Returns whether the list changed.
    pub fn read(&mut self, now: Instant, sources: Vec<Source>) -> bool {
        let secs = self.at.map_or(0.0, |t| now.duration_since(t).as_secs_f64());
        self.at = Some(now);
        self.stats.retain(|k, _| sources.iter().any(|s| s.key == *k));
        let mut rows = Vec::with_capacity(sources.len());
        for s in sources {
            let busy = s.meter.as_ref().map_or(0, |m| m.busy_ns());
            let st = self.stats.entry(s.key).or_insert_with(|| Stat { last: busy, base: busy, ..Stat::default() });
            st.time = if secs > 0.0 { busy.saturating_sub(st.last) as f64 / 1000.0 / secs } else { 0.0 };
            st.last = busy;
            st.peak = st.peak.max(st.time);
            rows.push(Row {
                key: s.key,
                name: s.name,
                patch: s.patch,
                place: s.place,
                synth: s.synth,
                stopped: s.stopped,
                time: st.time,
                peak: st.peak,
                total: busy.saturating_sub(st.base) as f64 / 1000.0,
                voices: s.synth.then(|| s.meter.as_ref().map_or(0, |m| m.voices())),
            });
        }
        // Frozen: the measurements go on, the list stays as it is (once there is one).
        if self.freeze && !self.rows.is_empty() {
            return false;
        }
        self.sort_rows(&mut rows);
        let changed = rows != self.rows;
        self.rows = rows;
        changed
    }

    /// Clears every measurement (the window was closed).
    pub fn reset(&mut self) {
        self.stats.clear();
        self.rows.clear();
        self.at = None;
        self.scroll = 0;
        self.last_click = None;
    }

    /// Clears the peaks (Reset on transport).
    pub fn reset_peaks(&mut self) {
        for s in self.stats.values_mut() {
            s.peak = 0.0;
        }
        for r in &mut self.rows {
            r.peak = 0.0;
        }
    }

    /// A click on a column header: sort by it, or reverse.
    pub fn sort_by(&mut self, c: Col) {
        if self.sort == c {
            self.desc = !self.desc;
        } else {
            self.sort = c;
            self.desc = c.numeric();
        }
        let mut rows = std::mem::take(&mut self.rows);
        self.sort_rows(&mut rows);
        self.rows = rows;
    }

    fn sort_rows(&self, rows: &mut [Row]) {
        let c = self.sort;
        // Stable: equal rows keep the rack / mixer order.
        rows.sort_by(|a, b| {
            let o = match c {
                Col::Plugin => a.title().to_lowercase().cmp(&b.title().to_lowercase()),
                Col::Type => b.synth.cmp(&a.synth),
                _ => a.value(c).total_cmp(&b.value(c)),
            };
            if self.desc { o.reverse() } else { o }
        });
    }

    /// The rows Find leaves, in order.
    pub fn shown(&self) -> Vec<&Row> {
        let f = self.find.to_lowercase();
        self.rows.iter().filter(|r| f.is_empty() || r.text().contains(&f)).collect()
    }
}

/// A time in microseconds as the list shows it: "8 412 µs", from 100 000 µs on "0.12 s".
pub fn fmt_us(us: f64) -> String {
    if us >= 100_000.0 {
        return format!("{:.2} s", us / 1e6);
    }
    let n = us.max(0.0).round() as u64;
    let d = n.to_string();
    let mut out = String::new();
    for (i, ch) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out + " µs"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn src(key: usize, name: &str, m: &Arc<Meter>, synth: bool) -> Source {
        Source { key, name: name.into(), patch: None, place: format!("Channel {key}"), synth, stopped: false, meter: Some(m.clone()) }
    }

    #[test]
    fn readings_peaks_sorting_and_find() {
        let (a, b) = (Arc::new(Meter::default()), Arc::new(Meter::default()));
        let t0 = Instant::now();
        let mut p = Perf::default();
        let list = |a: &Arc<Meter>, b: &Arc<Meter>| vec![src(0, "Lemon Sampler", a, true), src(1 << 24, "Lemon Parametric EQ", b, false)];
        p.read(t0, list(&a, &b));
        assert!(p.rows.iter().all(|r| r.time == 0.0), "no time over the first reading");
        // 5 ms and 1 ms of processing over half a second: 10 000 and 2 000 µs per second.
        a.add(Duration::from_millis(5));
        b.add(Duration::from_millis(1));
        a.set_voices(3);
        assert!(p.read(t0 + Duration::from_millis(500), list(&a, &b)));
        assert_eq!(p.rows[0].name, "Lemon Sampler", "Time, largest first");
        assert!((p.rows[0].time - 10_000.0).abs() < 1e-6 && (p.rows[0].share() - 1.0).abs() < 1e-9);
        assert_eq!((p.rows[0].voices, p.rows[1].voices), (Some(3), None));
        // Quieter now: Time falls, Peak and Total stay.
        p.read(t0 + Duration::from_millis(1000), list(&a, &b));
        assert_eq!((p.rows[0].time, p.rows[0].peak, p.rows[0].total), (0.0, 10_000.0, 5_000.0));
        p.reset_peaks();
        assert_eq!(p.rows[0].peak, 0.0);
        // Text sorts A to Z; a second click reverses.
        p.sort_by(Col::Plugin);
        assert_eq!(p.rows[0].name, "Lemon Parametric EQ");
        p.sort_by(Col::Plugin);
        assert_eq!(p.rows[0].name, "Lemon Sampler");
        p.find = "EQ".into();
        assert_eq!(p.shown().len(), 1);
        p.find = "channel 0".into();
        assert_eq!(p.shown()[0].name, "Lemon Sampler", "the place is searched too");
        // Frozen: the list stays.
        p.freeze = true;
        a.add(Duration::from_millis(50));
        assert!(!p.read(t0 + Duration::from_millis(1500), list(&a, &b)));
        assert_eq!(p.rows.iter().find(|r| r.key == 0).unwrap().time, 0.0);
        p.reset();
        assert!(p.rows.is_empty());
    }

    #[test]
    fn times_read_as_the_spec_says() {
        assert_eq!(fmt_us(8412.4), "8 412 µs");
        assert_eq!(fmt_us(999.0), "999 µs");
        assert_eq!(fmt_us(99_999.0), "99 999 µs");
        assert_eq!(fmt_us(123_456.0), "0.12 s");
        assert_eq!(fmt_us(2_500_000.0), "2.50 s");
    }
}
