//! Channel note rules of instrument channels: maximum polyphony, mono, portamento glides and cut groups (the plugin
//! window's Misc tab). They act on the note events the engine sends an instrument, so they work with any plugin; the
//! glides need a plugin that takes per-key tuning (`Instrument::tune`: CLAP note expression).
// spec: audio/channel-voices.md

use os_model::ChannelMisc;

/// Frames between glide tuning updates.
const GLIDE_STEP: u32 = 64;

/// One channel's rules.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct Rules {
    /// Most notes at once; 0 = unlimited.
    pub max: u32,
    pub mono: bool,
    pub porta: bool,
    /// Glide length in frames.
    pub glide: u32,
    /// Cut group this channel cuts, and the group that cuts it (0 = none).
    pub cut: u32,
    pub cut_by: u32,
}

impl Rules {
    pub fn new(m: &ChannelMisc, rate: u32) -> Rules {
        Rules {
            max: m.max_poly,
            mono: m.mono,
            porta: m.porta,
            glide: (crate::laws::slide_seconds(m.slide) * f64::from(rate)).round() as u32,
            cut: m.cut,
            cut_by: m.cut_by,
        }
    }
}

/// A note event for an instrument.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Note {
    On { key: u8, vel: f32, start: f32 },
    Off(u8),
    Choke(u8),
    /// Tuning of the voices of a key, semitones from the key's own pitch.
    Tune(u8, f32),
}

/// A tuning glide on one instrument key.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Glide {
    key: u8,
    from: f32,
    to: f32,
    /// Span frame where the glide is at its start (negative: started in an earlier span).
    at: i64,
    len: u32,
}

impl Glide {
    fn value(&self, frame: i64) -> f32 {
        let done = (frame - self.at).max(0) as f32;
        if self.len == 0 || done >= self.len as f32 {
            self.to
        } else {
            self.from + (self.to - self.from) * done / self.len as f32
        }
    }
}

/// One channel's notes at its instrument.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Voices {
    /// Notes sounding at the instrument, oldest first: (key sent to the instrument, key played now). They differ after
    /// a mono legato glide.
    held: Vec<(u8, u8)>,
    /// The last key played (where a portamento glide starts).
    last: Option<u8>,
    glides: Vec<Glide>,
}

impl Voices {
    /// Instrument keys of notes that now play another key (mono legato).
    pub fn taken_over(&self) -> impl Iterator<Item = u8> + '_ {
        self.held.iter().filter(|h| h.0 != h.1).map(|h| h.0)
    }

    /// Current tuning of instrument key `key` at span frame `frame`.
    fn tuning(&self, key: u8, frame: i64) -> f32 {
        self.glides.iter().rev().find(|g| g.key == key).map_or(0.0, |g| g.value(frame))
    }

    fn glide(&mut self, key: u8, from: f32, to: f32, at: u32, len: u32) {
        self.glides.retain(|g| g.key != key);
        self.glides.push(Glide { key, from, to, at: i64::from(at), len });
    }

    fn cut_all(&mut self, off: u32, ch: usize, out: &mut Vec<(usize, u32, Note)>) {
        for (sent, _) in self.held.drain(..) {
            out.push((ch, off, Note::Choke(sent)));
        }
        self.glides.clear();
    }
}

/// Event velocities below zero: a note off, a choke (the engine's `inst_events` encoding).
pub(super) const OFF: f32 = -1.0;
pub(super) const CHOKE: f32 = -2.0;

/// Applies each channel's rules to one span's events `(channel, offset, key, velocity, start)` (sorted by offset) and
/// adds the glides' tuning steps; `out` gets `(channel, offset, note)` sorted by channel, then offset.
pub(super) fn apply(events: &[(usize, u32, u8, f32, f32)], rules: &[Rules], voices: &mut [Voices], n: u32, out: &mut Vec<(usize, u32, Note)>) {
    out.clear();
    for &(c, off, key, vel, start) in events {
        let (Some(r), true) = (rules.get(c).copied(), c < voices.len()) else { continue };
        if vel <= CHOKE {
            // An audio clip's end cuts the note it started.
            let v = &mut voices[c];
            match v.held.iter().position(|h| h.1 == key) {
                Some(i) => {
                    let (sent, _) = v.held.remove(i);
                    out.push((c, off, Note::Choke(sent)));
                }
                None => out.push((c, off, Note::Choke(key))),
            }
            continue;
        }
        if vel < 0.0 {
            let v = &mut voices[c];
            match v.held.iter().position(|h| h.1 == key) {
                Some(i) => {
                    let (sent, _) = v.held.remove(i);
                    // A mono legato note that another key took over is already gone from `held`.
                    out.push((c, off, Note::Off(sent)));
                }
                // Not ours (a legato note taken over, or a note from before the rules changed): only plain channels pass it.
                None if !r.mono => out.push((c, off, Note::Off(key))),
                None => {}
            }
            continue;
        }
        // Cut groups: this note silences every channel cut by its group (itself too when it is in that group).
        if r.cut > 0 {
            for (d, rd) in rules.iter().enumerate() {
                if rd.cut_by == r.cut && d < voices.len() {
                    voices[d].cut_all(off, d, out);
                }
            }
        }
        let v = &mut voices[c];
        if r.mono && !v.held.is_empty() {
            if r.porta {
                // Legato: the sounding note glides to the new key without a new attack.
                let (sent, _) = *v.held.last().expect("held");
                let now = v.tuning(sent, i64::from(off));
                let to = f32::from(key) - f32::from(sent);
                let keep = v.held.len() - 1;
                for (s, _) in v.held.drain(..keep).collect::<Vec<_>>() {
                    out.push((c, off, Note::Choke(s)));
                }
                v.held[0].1 = key;
                v.glide(sent, now, to, off, r.glide);
                out.push((c, off, Note::Tune(sent, now)));
                v.last = Some(key);
                continue;
            }
            v.cut_all(off, c, out);
        }
        if r.max > 0 && !r.mono {
            while v.held.len() >= r.max as usize {
                let (sent, _) = v.held.remove(0);
                out.push((c, off, Note::Choke(sent)));
            }
        }
        // A retriggered key: its old tuning ends with it.
        v.glides.retain(|g| g.key != key);
        out.push((c, off, Note::On { key, vel, start }));
        v.held.push((key, key));
        if let (true, Some(last)) = (r.porta, v.last) {
            if last != key && r.glide > 0 {
                let from = f32::from(last) - f32::from(key);
                v.glide(key, from, 0.0, off, r.glide);
                out.push((c, off, Note::Tune(key, from)));
            }
        }
        v.last = Some(key);
    }
    // Glide steps: every GLIDE_STEP frames of the span, plus the final value where a glide ends.
    for (c, v) in voices.iter_mut().enumerate() {
        for g in &v.glides {
            let end = g.at + i64::from(g.len);
            let mut f = g.at.max(0);
            // Continue on the GLIDE_STEP grid of the glide.
            let rem = (f - g.at).rem_euclid(i64::from(GLIDE_STEP));
            if rem != 0 {
                f += i64::from(GLIDE_STEP) - rem;
            } else if f == g.at {
                f += i64::from(GLIDE_STEP); // the start value went out with the note
            }
            while f < end.min(i64::from(n)) {
                out.push((c, f as u32, Note::Tune(g.key, g.value(f))));
                f += i64::from(GLIDE_STEP);
            }
            if end >= 0 && end < i64::from(n) {
                out.push((c, end as u32, Note::Tune(g.key, g.to)));
            }
        }
        for g in &mut v.glides {
            g.at -= i64::from(n);
        }
        v.glides.retain(|g| g.at + i64::from(g.len) >= 0);
        // Keys no longer held keep no glide.
        let held: Vec<u8> = v.held.iter().map(|h| h.0).collect();
        v.glides.retain(|g| held.contains(&g.key));
    }
    out.sort_by_key(|e| (e.0, e.1));
}

/// True when no channel has a rule (the engine then passes notes through as they are).
pub(super) fn all_plain(rules: &[Rules]) -> bool {
    rules.iter().all(|r| r.max == 0 && !r.mono && !r.porta && r.cut == 0 && r.cut_by == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rules: &[Rules], voices: &mut [Voices], ev: &[(usize, u32, u8, f32, f32)], n: u32) -> Vec<(usize, u32, Note)> {
        let mut out = Vec::new();
        apply(ev, rules, voices, n, &mut out);
        out
    }

    #[test]
    fn max_polyphony_cuts_the_oldest() {
        let r = [Rules { max: 2, ..Rules::default() }];
        let mut v = vec![Voices::default()];
        let out = run(&r, &mut v, &[(0, 0, 60, 0.8, 0.0), (0, 10, 62, 0.8, 0.0), (0, 20, 64, 0.8, 0.0)], 128);
        assert_eq!(
            out.iter().map(|e| (e.1, e.2)).collect::<Vec<_>>(),
            vec![
                (0, Note::On { key: 60, vel: 0.8, start: 0.0 }),
                (10, Note::On { key: 62, vel: 0.8, start: 0.0 }),
                (20, Note::Choke(60)),
                (20, Note::On { key: 64, vel: 0.8, start: 0.0 }),
            ]
        );
        // The cut note's own note off is dropped.
        let out = run(&r, &mut v, &[(0, 5, 60, OFF, 0.0), (0, 6, 62, OFF, 0.0)], 128);
        assert_eq!(out.iter().map(|e| e.2).collect::<Vec<_>>(), vec![Note::Off(60), Note::Off(62)]);
    }

    #[test]
    fn mono_cuts_and_mono_porta_glides_legato() {
        let r = [Rules { mono: true, ..Rules::default() }];
        let mut v = vec![Voices::default()];
        let out = run(&r, &mut v, &[(0, 0, 60, 0.8, 0.0), (0, 10, 67, 0.8, 0.0)], 128);
        assert_eq!(out[1].2, Note::Choke(60));
        assert_eq!(out[2].2, Note::On { key: 67, vel: 0.8, start: 0.0 });

        let r = [Rules { mono: true, porta: true, glide: 100, ..Rules::default() }];
        let mut v = vec![Voices::default()];
        let out = run(&r, &mut v, &[(0, 0, 60, 0.8, 0.0), (0, 10, 67, 0.8, 0.0)], 128);
        // No second attack: key 60 glides up 7 semitones over 100 frames.
        assert_eq!(out.iter().filter(|e| matches!(e.2, Note::On { .. })).count(), 1);
        let tunes: Vec<(u32, f32)> = out.iter().filter_map(|e| if let Note::Tune(60, s) = e.2 { Some((e.1, s)) } else { None }).collect();
        assert_eq!(tunes.first(), Some(&(10, 0.0)));
        assert_eq!(tunes.last(), Some(&(110, 7.0)));
        assert!(tunes.windows(2).all(|w| w[1].1 >= w[0].1));
        // Releasing the old key does nothing; releasing the new one ends the sounding note (key 60 at the plugin).
        let out = run(&r, &mut v, &[(0, 0, 60, OFF, 0.0)], 128);
        assert!(out.is_empty());
        let out = run(&r, &mut v, &[(0, 0, 67, OFF, 0.0)], 128);
        assert_eq!(out[0].2, Note::Off(60));
    }

    #[test]
    fn porta_glides_each_new_note_from_the_last_key() {
        let r = [Rules { porta: true, glide: 300, ..Rules::default() }];
        let mut v = vec![Voices::default()];
        run(&r, &mut v, &[(0, 0, 60, 0.8, 0.0), (0, 1, 60, OFF, 0.0)], 128);
        let out = run(&r, &mut v, &[(0, 50, 72, 0.8, 0.0)], 128);
        assert_eq!(out[0].2, Note::On { key: 72, vel: 0.8, start: 0.0 });
        assert_eq!(out[1].2, Note::Tune(72, -12.0));
        // The glide continues over the next spans and ends at 0 at frame 50 + 300.
        let mut last = None;
        for _ in 0..3 {
            for e in run(&r, &mut v, &[], 128) {
                if let Note::Tune(72, s) = e.2 {
                    last = Some(s);
                }
            }
        }
        assert_eq!(last, Some(0.0));
        assert!(v[0].glides.is_empty());
    }

    #[test]
    fn cut_groups_choke_other_channels() {
        // Open hat (0) is cut by group 1; closed hat (1) cuts group 1.
        let r = [Rules { cut_by: 1, ..Rules::default() }, Rules { cut: 1, ..Rules::default() }];
        let mut v = vec![Voices::default(), Voices::default()];
        let out = run(&r, &mut v, &[(0, 0, 60, 0.8, 0.0), (1, 30, 60, 0.8, 0.0)], 128);
        assert!(out.contains(&(0, 30, Note::Choke(60))));
        assert!(out.contains(&(1, 30, Note::On { key: 60, vel: 0.8, start: 0.0 })));
        assert!(all_plain(&[Rules::default()]));
    }
}
