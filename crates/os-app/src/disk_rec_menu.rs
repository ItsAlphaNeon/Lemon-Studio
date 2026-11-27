// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The mixer menu's Options ▸ Disk recording submenu: rendering the armed or selected tracks, the recording options and
//! the recording format (spec: audio/disk-recording.md §2).

use crate::app::{App, State};
use crate::mixer::MCmd;
use crate::playlist::{Cmd, Mark, PItem};
use crate::prefs::{self, Check};

/// The submenu's checks after Auto-create audio clip, in its order (`MCmd::RecCheck` indexes them).
pub const CHECKS: [&Check; 4] =
    [&prefs::AUTO_UNARM, &prefs::REC_LATENCY_COMP, &prefs::AUTO_NAME_TRACKS, &prefs::GROUP_LOOP_TAKES];

/// The submenu, top to bottom as FL's: the two renders, Options, Format. Render armed is greyed while nothing is armed.
pub fn items(s: &State) -> Vec<PItem> {
    let mx = |c| Cmd::Mixer(c);
    let armed = !armed_tracks(s).is_empty();
    let render = PItem::new("Render armed tracks to wave file(s)...", mx(MCmd::RenderArmed)).key("Alt+R");
    let mut v = vec![
        if armed { render } else { render.off() },
        PItem::new("Render selected tracks to wave file(s)...", mx(MCmd::RenderWave(s.selected_mixer)))
            .key("Shift+Alt+R"),
        PItem::head("Options"),
    ];
    let check = |c: &Check, cmd| PItem::new(c.label, mx(cmd)).mark(Mark::Check(prefs::check(c)));
    v.push(check(&CHECKS[0], MCmd::RecCheck(0)));
    v.push(check(&prefs::AUTO_AUDIO_CLIPS, MCmd::AutoClips));
    for (i, c) in CHECKS.iter().enumerate().skip(1) {
        v.push(check(c, MCmd::RecCheck(i as u8)));
    }
    v.push(PItem::head("Format"));
    let depth = prefs::index(&prefs::REC_DEPTH);
    for (i, name) in prefs::REC_DEPTH.choices.iter().enumerate() {
        v.push(PItem::new(*name, mx(MCmd::RecDepth(i as u8))).mark(Mark::Radio(depth == i)));
    }
    v
}

/// One of `CHECKS` turned over; what the status bar says.
pub fn toggle(i: u8) -> Option<String> {
    let c = CHECKS.get(usize::from(i))?;
    let on = prefs::toggle(c);
    Some(format!("{} {}", c.label, if on { "on" } else { "off" }))
}

/// The recording format chosen.
pub fn set_depth(i: u8) -> Option<String> {
    let name = prefs::REC_DEPTH.choices.get(usize::from(i))?;
    prefs::set_choice(&prefs::REC_DEPTH, usize::from(i));
    Some(format!("Recording format: {name}"))
}

/// The armed mixer tracks, in order.
pub fn armed_tracks(s: &State) -> Vec<u16> {
    let mut v: Vec<u16> = s.session.project.mixer.iter().filter(|m| m.armed).map(|m| m.index).collect();
    v.sort_unstable();
    v
}

/// Render armed tracks to wave file(s)... (Alt+R): no file dialog, the render form for the armed tracks, into the
/// Recorded folder, under the names their recordings would get, at the recording format.
pub fn render_armed(app: &mut App) -> Option<String> {
    let tracks = armed_tracks(&app.state);
    if tracks.is_empty() {
        return Some("Render armed tracks: no mixer track is armed".into());
    }
    let name = crate::track_rec_ui::armed_file(&app.state, tracks[0]);
    let folder = crate::track_rec::folder(&app.state.session);
    app.open_track_render(tracks.clone());
    let Some(crate::dialog::Modal {
        body: crate::dialog::Body::Project(crate::project_forms::Form::Export(f)), ..
    }) = app.state.dialogs.last_mut()
    else {
        return None;
    };
    f.armed = true;
    f.folder = folder;
    f.o.wav_depth = prefs::index(&prefs::REC_DEPTH).min(2);
    if tracks.len() == 1 {
        f.name.set(name.strip_suffix(".wav").unwrap_or(&name));
        f.name.select_all();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playlist::PItem;

    fn labels(v: &[PItem]) -> Vec<String> {
        v.iter().map(|i| i.label.to_string()).collect()
    }

    /// The submenu in FL's order; Render armed greyed with nothing armed; the checks and radios follow the settings.
    #[test]
    fn the_submenu_is_fls() {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        let v = items(&app.state);
        assert_eq!(
            labels(&v),
            [
                "Render armed tracks to wave file(s)...",
                "Render selected tracks to wave file(s)...",
                "Options",
                "Auto-unarm",
                "Auto-create audio clips",
                "Latency compensation",
                "Auto name playlist tracks",
                "Group loop recorded takes",
                "Format",
                "16Bit int",
                "24Bit int",
                "32Bit float",
            ]
        );
        assert!(!v[0].enabled, "nothing armed");
        let keep = prefs::index(&prefs::REC_DEPTH);
        assert!(set_depth(1).is_some_and(|m| m.contains("24Bit")));
        assert_eq!(items(&app.state)[10].mark, Mark::Radio(true));
        prefs::set_choice(&prefs::REC_DEPTH, keep);
        let was = prefs::check(&prefs::AUTO_UNARM);
        assert!(toggle(0).is_some());
        assert_eq!(prefs::check(&prefs::AUTO_UNARM), !was);
        toggle(0);
        app.arm_tracks(&[2], true);
        assert!(items(&app.state)[0].enabled);
    }

    /// Alt+R with a track armed: the render form for it, into the Recorded folder, named as its recording would be.
    #[test]
    fn render_armed_opens_the_form_for_the_armed_track() {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app.state.session.project.get_mut().patterns[0].notes.push(os_model::Note::step(0, 0));
        app.state.set_song_mode(false);
        assert!(render_armed(&mut app).is_some_and(|m| m.contains("no mixer track")));
        app.arm_tracks(&[2], true);
        assert_eq!(render_armed(&mut app), None);
        let Some(crate::dialog::Modal {
            body: crate::dialog::Body::Project(crate::project_forms::Form::Export(f)),
            ..
        }) = app.state.dialogs.last()
        else {
            panic!("no form")
        };
        assert_eq!(f.tracks, [2]);
        assert!(f.armed && f.folder.ends_with("Recorded"));
        let bus = crate::edit::mixer::name(&app.state.session, 2);
        assert!(f.name.text.ends_with(&format!("_{bus}")), "{}", f.name.text);
    }
}
