// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! An audio clip's fade, tension and gain handles beyond dragging: Alt+click resets one, a right click opens its menu
//! (reset, crossfade, copy / paste, type in a value, normalize). Each change is one undo step of the arrangement.
// spec: ui/playlist-fades-gain.md

use std::sync::Mutex;

use os_model::Clip;

use crate::app::State;
use crate::playlist::{arr_edit, arr_mut, clips, Cmd, Handle, PItem, Popup, ARRANGER_EDIT};

/// What a handle menu's entry does (§2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HCmd {
    Reset(Handle),
    /// Create crossfade: the fade grows over the neighbouring clip's overlap (§2).
    Crossfade(Handle),
    Copy(Handle),
    Paste(Handle),
    TypeIn(Handle),
    /// Normalize selection individually (false) or as a group (true) (§2).
    Normalize(bool),
}

/// The value "Copy value" took: fades in milliseconds, gain as the linear factor. One for all clips, as FL has it.
static COPIED: Mutex<Option<(Kind, f32)>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Fade,
    Tension,
    Gain,
}

fn kind(h: Handle) -> Kind {
    match h {
        Handle::FadeIn | Handle::FadeOut => Kind::Fade,
        Handle::TensionIn | Handle::TensionOut | Handle::Crossfade => Kind::Tension,
        Handle::Gain => Kind::Gain,
    }
}

/// The value handle `h` of `c` shows: a fade's milliseconds, a tension, the gain factor.
pub fn value(c: &Clip, h: Handle) -> f32 {
    match h {
        Handle::FadeIn => c.stored_fade_in().map_or(0.0, |f| f.0),
        Handle::FadeOut => c.stored_fade_out().map_or(0.0, |f| f.0),
        Handle::TensionIn | Handle::Crossfade => c.fade_in_tension(),
        Handle::TensionOut => c.fade_out_tension(),
        Handle::Gain => c.gain(),
    }
}

/// A tension's range, as dragging its handle gives it.
pub const TENSION_MAX: f32 = 40.0;

/// Sets handle `h` of `c` to `v` (as `value` gives it).
pub fn set(c: &mut Clip, h: Handle, v: f32) {
    match h {
        Handle::FadeIn => c.set_fade_in(Some((v.max(0.0), c.fade_in_tension()))),
        Handle::FadeOut => c.set_fade_out(Some((v.max(0.0), c.fade_out_tension()))),
        Handle::TensionIn | Handle::Crossfade => c.set_fade_in_tension(v.clamp(-TENSION_MAX, TENSION_MAX)),
        Handle::TensionOut => c.set_fade_out_tension(v.clamp(-TENSION_MAX, TENSION_MAX)),
        Handle::Gain => c.set_gain(gain_clamped(v)),
    }
}

/// `v` for handle `h` of `c` as dragging keeps it: a fade within the clip, up to where the other fade starts.
fn within(s: &crate::session::Session, c: &Clip, h: Handle, v: f32) -> f32 {
    let room = |other: Option<(f32, f32)>| {
        let len = (f64::from(c.length) * crate::playlist::fade_ms_per_tick(s)) as f32;
        (len - other.map_or(0.0, |f| f.0)).max(0.0)
    };
    match h {
        Handle::FadeIn => v.clamp(0.0, room(c.stored_fade_out())),
        Handle::FadeOut => v.clamp(0.0, room(c.stored_fade_in())),
        _ => v,
    }
}

/// Puts `v` on handle `h` of clip `i` as dragging would: a fade kept within the clip, a crossfade's tension on both
/// clips (the earlier one's fade-out takes the opposite, §9a).
fn put(s: &mut crate::session::Session, i: usize, h: Handle, v: f32) {
    let all = clips(s);
    let Some(c) = all.get(i) else { return };
    let v = within(s, c, h, v);
    let partner = if h == Handle::Crossfade { os_model::crossfade::partner_before(&s.project, all, i) } else { None };
    let arr = arr_mut(s);
    set(&mut arr.clips[i], h, v);
    if let Some(a) = partner {
        let t = arr.clips[i].fade_in_tension();
        arr.clips[a].set_fade_out_tension(os_model::crossfade::opposite(t));
    }
}

/// The gain kept for −∞. A stored 0 reads as unity (a record too short to hold a gain), so −∞ is kept as the
/// smallest positive single: silent. How FL stores −∞ is not measured.
pub const SILENT: f32 = f32::MIN_POSITIVE;

/// The gain handle's range (§2): up to +36 dB; below −96 dB it is −∞ (`SILENT`).
pub fn gain_clamped(g: f32) -> f32 {
    let max = 10f32.powf(36.0 / 20.0);
    if !(g > 10f32.powf(-96.0 / 20.0)) { SILENT } else { g.min(max) }
}

/// The value a reset gives (§1): no fade, a straight tension, 0 dB.
pub fn default_of(h: Handle) -> f32 {
    match h {
        Handle::Gain => 1.0,
        _ => 0.0,
    }
}

/// Alt+click on handle `h` of clip `i`: back to its default, one undo step (§1). True when it changed.
pub fn reset(s: &mut State, i: usize, h: Handle) -> bool {
    let Some(before) = clips(&s.session).get(i).copied() else { return false };
    let mut after = before;
    // A crossfade's handle is the later clip's fade-in tension.
    set(&mut after, h, default_of(h));
    if after == before {
        return false;
    }
    arr_edit(s, ARRANGER_EDIT, |s| {
        if let Some(c) = arr_mut(&mut s.session).clips.get_mut(i) {
            *c = after;
        }
    });
    true
}

/// The right-click menu of handle `h` of clip `i` (§2).
pub fn menu(s: &State, i: usize, h: Handle, x: f32, y: f32) -> Popup {
    let it = |label: &'static str, c: HCmd| PItem::new(label, Cmd::Handle(i, c));
    let copied = COPIED.lock().ok().and_then(|g| *g).filter(|(k, _)| *k == kind(h));
    let paste = it("Paste value", HCmd::Paste(h));
    let paste = if copied.is_some() { paste } else { paste.off() };
    let mut items = match h {
        Handle::Gain => vec![
            it("Reset gain", HCmd::Reset(h)),
            PItem::sep(),
            it("Normalize selection individually", HCmd::Normalize(false)),
            it("Normalize selection as a group", HCmd::Normalize(true)),
            PItem::sep(),
        ],
        Handle::FadeIn | Handle::FadeOut => {
            let label = if h == Handle::FadeIn { "Reset fade in" } else { "Reset fade out" };
            vec![it(label, HCmd::Reset(h)), it("Create crossfade", HCmd::Crossfade(h)), PItem::sep()]
        }
        _ => vec![it("Reset tension", HCmd::Reset(h)), PItem::sep()],
    };
    items.extend([it("Copy value", HCmd::Copy(h)), paste, it("Type in value...", HCmd::TypeIn(h))]);
    let _ = s;
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// Runs a handle menu's entry on clip `i`; the message to show, if any.
pub fn run(s: &mut State, i: usize, c: HCmd) -> Option<String> {
    match c {
        HCmd::Reset(h) => {
            reset(s, i, h);
            None
        }
        HCmd::Copy(h) => {
            let v = value(clips(&s.session).get(i)?, h);
            if let Ok(mut g) = COPIED.lock() {
                *g = Some((kind(h), v));
            }
            None
        }
        HCmd::Paste(h) => {
            let (k, v) = COPIED.lock().ok().and_then(|g| *g)?;
            if k != kind(h) {
                return None;
            }
            arr_edit(s, ARRANGER_EDIT, |s| put(&mut s.session, i, h, v));
            None
        }
        // Opened by `playlist::run`, which knows where the menu was.
        HCmd::TypeIn(_) => None,
        HCmd::Crossfade(h) => crossfade(s, i, h),
        HCmd::Normalize(group) => normalize(s, i, group),
    }
}

/// Create crossfade (§2): clip `i` and the clip its fade `h` meets on the track both get a fade over their overlap
/// (fade `h` on clip `i`, the other edge's fade on the other clip), each kept within its clip as dragging keeps it.
fn crossfade(s: &mut State, i: usize, h: Handle) -> Option<String> {
    let all = clips(&s.session);
    let c = *all.get(i)?;
    let out = h == Handle::FadeOut;
    // The neighbour this fade meets: the clip on the track overlapping its start (fade in) or its end (out) most.
    let (j, overlap) = all
        .iter()
        .enumerate()
        .filter(|(j, o)| *j != i && o.track == c.track)
        .filter(|(_, o)| {
            if out {
                o.position < c.end() && o.position > c.position
            } else {
                o.end() > c.position && o.position < c.position
            }
        })
        .map(|(j, o)| (j, if out { c.end() - o.position } else { o.end() - c.position }))
        .max_by_key(|p| p.1)?;
    let ms = (f64::from(overlap) * crate::playlist::fade_ms_per_tick(&s.session)) as f32;
    let theirs = if out { Handle::FadeIn } else { Handle::FadeOut };
    arr_edit(s, ARRANGER_EDIT, |s| {
        put(&mut s.session, i, h, ms);
        put(&mut s.session, j, theirs, ms);
    });
    None
}

/// The peak of what clip `c` plays of its sample (its window), as a linear level; `None` while the sample's peaks are
/// not known.
pub fn clip_peak(s: &State, c: &Clip) -> Option<f32> {
    let os_model::ClipSource::Channel(ch) = c.source else { return None };
    let path = s.session.project.channel(ch).and_then(|chan| chan.sample_path.clone())?;
    let w = s.session.samples.wave(&s.session.settings, &path)?;
    let (w0, w1) = crate::playlist::window(&s.session, c);
    let per_ms = f64::from(w.rate) / 1000.0;
    let fa = ((w0 * per_ms).max(0.0) as usize / w.per.max(1)).min(w.peaks.len());
    let fb = (((w1 * per_ms).max(0.0) as usize).div_ceil(w.per.max(1))).clamp(fa, w.peaks.len());
    let p = w.peaks[fa..fb].iter().fold(0f32, |m, &(lo, hi)| m.max(lo.abs()).max(hi.abs()));
    (p > 0.0).then_some(p)
}

/// Normalize selection individually / as a group (§2): the selected audio clips (clip `i` alone when it is not
/// selected) get the gain that brings their peak to 0 dB, each its own, or the one gain that brings the loudest to
/// 0 dB.
fn normalize(s: &mut State, i: usize, group: bool) -> Option<String> {
    let idx = crate::clip_props::menu_targets(s, i);
    let peaks: Vec<(usize, f32)> =
        idx.iter().filter_map(|&j| clips(&s.session).get(j).and_then(|c| clip_peak(s, c)).map(|p| (j, p))).collect();
    if peaks.is_empty() {
        return Some("Normalize: the sample is not read yet".into());
    }
    let loudest = peaks.iter().map(|p| p.1).fold(0f32, f32::max);
    arr_edit(s, ARRANGER_EDIT, |s| {
        let arr = arr_mut(&mut s.session);
        for &(j, p) in &peaks {
            if let Some(c) = arr.clips.get_mut(j) {
                c.set_gain(gain_clamped(1.0 / if group { loudest } else { p }));
            }
        }
    });
    None
}

/// The value box's text for handle `h` (§2): a fade in milliseconds, a tension, the gain in dB.
pub fn text(c: &Clip, h: Handle) -> String {
    let v = value(c, h);
    match kind(h) {
        Kind::Fade => format!("{v:.0}"),
        Kind::Tension => format!("{v:.2}"),
        Kind::Gain if v <= SILENT => "-inf".into(),
        Kind::Gain => format!("{:.1}", 20.0 * v.log10()),
    }
}

/// The value typed for handle `h` of clip `i` (§2): milliseconds for a fade, −1..1 for a tension, dB for the gain
/// ("-inf" for none); anything else is ignored. One undo step.
pub fn commit(s: &mut State, i: usize, h: Handle, text: &str) {
    let t = text.trim().trim_end_matches("ms").trim_end_matches("dB").trim();
    let v = match kind(h) {
        Kind::Gain if t.eq_ignore_ascii_case("-inf") => Some(0.0),
        Kind::Gain => t.parse::<f32>().ok().map(|db| 10f32.powf(db / 20.0)),
        _ => t.parse::<f32>().ok(),
    };
    let Some(v) = v.filter(|v| v.is_finite()) else { return };
    arr_edit(s, ARRANGER_EDIT, |s| put(&mut s.session, i, h, v));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use os_model::{ChannelKind, ClipSource};

    fn app() -> App {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app.state.session.project.get_mut().channels[0].kind = ChannelKind::AudioClip;
        let ch = app.state.session.project.channels[0].index;
        let mut c = Clip::new(0, 384, 0, ClipSource::Channel(ch));
        c.set_fade_in(Some((120.0, 0.5)));
        c.set_fade_out(Some((80.0, -0.25)));
        c.set_gain(2.0);
        arr_mut(&mut app.state.session).clips = vec![c, Clip::new(384, 384, 0, ClipSource::Channel(ch))];
        app
    }

    fn clip(app: &App) -> Clip {
        clips(&app.state.session)[0]
    }

    /// Alt+click resets a fade (no fade), a tension (straight) and the gain (0 dB), each one undo step.
    #[test]
    fn reset_each_handle() {
        let mut app = app();
        assert!(reset(&mut app.state, 0, Handle::TensionIn));
        assert_eq!((clip(&app).fade_in_tension(), clip(&app).stored_fade_in().map(|f| f.0)), (0.0, Some(120.0)));
        assert!(reset(&mut app.state, 0, Handle::FadeOut));
        assert_eq!(clip(&app).stored_fade_out(), None);
        assert!(reset(&mut app.state, 0, Handle::Gain));
        assert_eq!(clip(&app).gain(), 1.0);
        assert!(!reset(&mut app.state, 0, Handle::Gain), "already there: no step");
        crate::history_ui::undo(&mut app.state);
        assert_eq!(clip(&app).gain(), 2.0);
    }

    /// The ▸ menu's Edit ▸ Automatic crossfades is a check that switches the project's setting (one undo step).
    #[test]
    fn edit_menu_automatic_crossfades() {
        let mut app = app();
        let entry = |app: &App| {
            let pop = crate::playlist::options_menu(&app.state, 0.0, 0.0);
            let edit = pop.items.iter().find(|i| i.label == "Edit").unwrap().children.clone();
            edit.into_iter().find(|i| i.label == "Automatic crossfades").expect("entry")
        };
        let was = app.state.session.project.auto_crossfades.unwrap_or(false);
        let e = entry(&app);
        assert_eq!(e.mark, crate::playlist::Mark::Check(was));
        crate::playlist::run(&mut app, e.cmd);
        assert_eq!(app.state.session.project.auto_crossfades, Some(!was));
        assert_eq!(entry(&app).mark, crate::playlist::Mark::Check(!was));
        crate::history_ui::undo(&mut app.state);
        assert_eq!(app.state.session.project.auto_crossfades.unwrap_or(false), was);
    }

    /// The menus: a fade's, a tension's and the gain's entries; Paste value is off until a value of that kind is
    /// copied, and pastes it.
    #[test]
    fn menus_copy_and_paste() {
        let mut app = app();
        let labels = |h| {
            menu(&app.state, 0, h, 0.0, 0.0)
                .items
                .iter()
                .filter(|i| !i.sep)
                .map(|i| i.label.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(Handle::FadeIn),
            ["Reset fade in", "Create crossfade", "Copy value", "Paste value", "Type in value..."]
        );
        assert_eq!(
            labels(Handle::Gain),
            [
                "Reset gain",
                "Normalize selection individually",
                "Normalize selection as a group",
                "Copy value",
                "Paste value",
                "Type in value..."
            ]
        );
        run(&mut app.state, 0, HCmd::Copy(Handle::FadeIn));
        let paste = |app: &App, h| {
            menu(&app.state, 1, h, 0.0, 0.0).items.into_iter().find(|i| i.label == "Paste value").unwrap()
        };
        assert!(paste(&app, Handle::FadeOut).enabled, "a fade's value pastes on either fade");
        assert!(!paste(&app, Handle::Gain).enabled);
        run(&mut app.state, 1, HCmd::Paste(Handle::FadeOut));
        assert_eq!(clips(&app.state.session)[1].stored_fade_out().map(|f| f.0), Some(120.0));
    }

    /// Typed, pasted and crossfade values keep to what dragging allows: a tension up to ±40, a fade within its clip and
    /// not past the other fade. Create crossfade gives both clips a fade over their overlap.
    #[test]
    fn values_keep_to_the_drag_range_and_crossfades_set_both_clips() {
        let mut app = app();
        commit(&mut app.state, 0, Handle::TensionIn, "20");
        assert_eq!(clip(&app).fade_in_tension(), 20.0);
        commit(&mut app.state, 0, Handle::TensionOut, "-99");
        assert_eq!(clip(&app).fade_out_tension(), -TENSION_MAX);
        let len = (384.0 * crate::playlist::fade_ms_per_tick(&app.state.session)) as f32;
        commit(&mut app.state, 0, Handle::FadeIn, "100000");
        assert!((clip(&app).stored_fade_in().unwrap().0 - (len - 80.0)).abs() < 1e-3, "up to the fade-out");
        run(&mut app.state, 0, HCmd::Copy(Handle::FadeIn));
        run(&mut app.state, 0, HCmd::Paste(Handle::FadeOut));
        assert!(clip(&app).stored_fade_out().unwrap().0 <= len - clip(&app).stored_fade_in().unwrap().0 + 1e-3);
        // The second clip moved to overlap the first by a beat.
        let mut app = self::app();
        arr_mut(&mut app.state.session).clips[1].position = 288;
        run(&mut app.state, 0, HCmd::Crossfade(Handle::FadeOut));
        let beat = (96.0 * crate::playlist::fade_ms_per_tick(&app.state.session)) as f32;
        let all = clips(&app.state.session);
        assert!((all[0].stored_fade_out().unwrap().0 - beat).abs() < 1e-3);
        assert!((all[1].stored_fade_in().unwrap().0 - beat).abs() < 1e-3, "the other clip fades in over the overlap");
        crate::history_ui::undo(&mut app.state);
        assert_eq!(clips(&app.state.session)[1].stored_fade_in(), None, "one step");
    }

    /// Type in value...: milliseconds for a fade, dB for the gain ("-inf" for none), within the gain's range.
    #[test]
    fn typed_values() {
        let mut app = app();
        assert_eq!(text(&clip(&app), Handle::FadeIn), "120");
        assert_eq!(text(&clip(&app), Handle::Gain), "6.0");
        commit(&mut app.state, 0, Handle::FadeIn, "250 ms");
        assert_eq!(clip(&app).stored_fade_in().map(|f| f.0), Some(250.0));
        commit(&mut app.state, 0, Handle::Gain, "-6");
        assert!((20.0 * clip(&app).gain().log10() + 6.0).abs() < 1e-3);
        commit(&mut app.state, 0, Handle::Gain, "48");
        assert!((20.0 * clip(&app).gain().log10() - 36.0).abs() < 1e-3, "+36 dB at most");
        commit(&mut app.state, 0, Handle::Gain, "-inf");
        assert_eq!(value(&clip(&app), Handle::Gain), SILENT);
        assert_eq!(text(&clip(&app), Handle::Gain), "-inf");
        commit(&mut app.state, 0, Handle::FadeIn, "abc");
        assert_eq!(clip(&app).stored_fade_in().map(|f| f.0), Some(250.0), "ignored");
    }
}
