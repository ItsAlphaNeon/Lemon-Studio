// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Audio clips' own pitch, pan and reverse: the playlist's keys 7, 8 and 9, its Edit menu's and the clip menu's
//! entries. Each is one undo step of the arrangement.
// spec: ui/playlist-clip-properties.md

use os_model::Clip;

use crate::app::State;
use crate::playlist::{arr_edit, arr_mut, clips, source_kind, PickKind, ARRANGER_EDIT};

/// The selected clips that are audio clips (the keys and the Edit menu's entries act on them only, §1).
pub fn selected_audio(s: &State) -> Vec<usize> {
    let all = clips(&s.session);
    s.playlist.sel.iter().copied().filter(|&i| all.get(i).is_some_and(|c| is_audio(s, c))).collect()
}

fn is_audio(s: &State, c: &Clip) -> bool {
    source_kind(&s.session, c.source) == PickKind::Audio
}

/// Pitches clips `idx` by `semitones` each, from their own pitch, within the range (§1). True when one changed.
pub fn pitch(s: &mut State, idx: &[usize], semitones: i32) -> bool {
    if idx.is_empty() {
        return false;
    }
    arr_edit(s, ARRANGER_EDIT, |s| {
        let arr = arr_mut(&mut s.session);
        let mut changed = false;
        for &i in idx {
            if let Some(c) = arr.clips.get_mut(i) {
                let to = (c.pitch_semitones() + semitones).clamp(-Clip::PITCH_RANGE, Clip::PITCH_RANGE);
                changed |= to != c.pitch_semitones();
                c.set_pitch_semitones(to);
            }
        }
        changed
    })
}

/// Switches Reverse of clips `idx`, each from its own state (§1). True when there was one.
pub fn reverse(s: &mut State, idx: &[usize]) -> bool {
    if idx.is_empty() {
        return false;
    }
    arr_edit(s, ARRANGER_EDIT, |s| {
        let arr = arr_mut(&mut s.session);
        for &i in idx {
            if let Some(c) = arr.clips.get_mut(i) {
                let on = c.reversed();
                c.set_reversed(!on);
            }
        }
    });
    true
}

/// The clips a clip menu's entry acts on: the selection when clip `i` is in it, else `i` alone; audio clips only.
pub fn menu_targets(s: &State, i: usize) -> Vec<usize> {
    if s.playlist.sel.contains(&i) {
        selected_audio(s)
    } else {
        clips(&s.session).get(i).filter(|c| is_audio(s, c)).map(|_| vec![i]).unwrap_or_default()
    }
}

/// The audio clip menu's entries for clip `c` (number `i`): pitch down / up and Reverse (ticked while it plays
/// backwards).
pub fn menu_items(c: &Clip, i: usize) -> Vec<crate::playlist::PItem> {
    use crate::playlist::{Cmd, Mark, PItem};
    vec![
        PItem::sep(),
        PItem::new("Pitch one semitone down", Cmd::ClipPitch(i, -1)).key("7"),
        PItem::new("Pitch one semitone up", Cmd::ClipPitch(i, 1)).key("8"),
        PItem::new("Reverse", Cmd::ClipReverse(i)).key("9").mark(Mark::Check(c.reversed())),
    ]
}

/// What an audio clip's title shows after its name for its own pitch: `+3`, `-2.5` (semitones, one decimal when it
/// has cents); nothing at 0 (spec: ui/audio-clip-pitch-reverse.md "What the owner sees").
pub fn pitch_label(c: &Clip) -> Option<String> {
    match c.pitch_cents() {
        0 => None,
        p if p % 100 == 0 => Some(format!("{:+}", p / 100)),
        p => Some(format!("{:+.1}", f64::from(p) / 100.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::playlist::{clip_menu, options_menu, run, Cmd, Mark};
    use os_model::{ChannelKind, ClipSource};

    /// An app with two audio clips of channel 0 on track 0 and a pattern clip on track 1, all selected.
    fn app() -> App {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app.state.session.project.get_mut().channels[0].kind = ChannelKind::AudioClip;
        let ch = app.state.session.project.channels[0].index;
        let arr = arr_mut(&mut app.state.session);
        arr.clips = vec![
            Clip::new(0, 384, 0, ClipSource::Channel(ch)),
            Clip::new(384, 384, 0, ClipSource::Channel(ch)),
            Clip::new(0, 384, 1, ClipSource::Pattern(1)),
        ];
        app.state.playlist.sel = vec![0, 1, 2];
        // The keys are the playlist's while the typing keyboard is off (it is on in a new app, as in FL).
        app.state.toggles.retain(|t| *t != "typing");
        app
    }

    fn pitches(app: &App) -> Vec<i32> {
        clips(&app.state.session).iter().map(Clip::pitch_semitones).collect()
    }

    /// 7 and 8 pitch the selected audio clips a semitone down / up, each from its own pitch; the pattern clip stays.
    /// Each press is one undo step.
    #[test]
    fn keys_pitch_the_selected_audio_clips() {
        let mut app = app();
        let key = |c: &str| crate::keys::Key::Character(c.into());
        assert!(crate::playlist::key(&mut app, &key("8"), false, false));
        assert_eq!(pitches(&app), [1, 1, 0]);
        app.state.playlist.sel = vec![1];
        crate::playlist::key(&mut app, &key("8"), false, false);
        crate::playlist::key(&mut app, &key("7"), false, false);
        crate::playlist::key(&mut app, &key("7"), false, false);
        assert_eq!(pitches(&app), [1, 0, 0]);
        crate::history_ui::undo(&mut app.state);
        assert_eq!(pitches(&app), [1, 1, 0], "one step per press");
        // The range's end holds.
        app.state.playlist.sel = vec![0];
        for _ in 0..(Clip::PITCH_RANGE + 5) {
            run(&mut app, Cmd::PitchSelected(1));
        }
        assert_eq!(pitches(&app)[0], Clip::PITCH_RANGE);
    }

    /// 9 switches Reverse of each selected audio clip from its own state; with the typing keyboard on the digits are
    /// not the playlist's.
    #[test]
    fn nine_reverses_and_typing_keyboard_takes_the_digits() {
        let mut app = app();
        let nine = crate::keys::Key::Character("9".into());
        crate::playlist::key(&mut app, &nine, false, false);
        let rev = |app: &App| clips(&app.state.session).iter().map(Clip::reversed).collect::<Vec<_>>();
        assert_eq!(rev(&app), [true, true, false]);
        app.state.playlist.sel = vec![0];
        crate::playlist::key(&mut app, &nine, false, false);
        assert_eq!(rev(&app), [false, true, false]);
        app.state.toggles.push("typing");
        app.state.playlist.sel = vec![0, 1];
        assert!(!crate::playlist::key(&mut app, &nine, false, false));
        assert_eq!(rev(&app), [false, true, false]);
    }

    /// The ▸ menu's Edit has the three entries with their keys; the audio clip menu has pitch and a ticked Reverse,
    /// acting on the selection the clip is in (or the clip alone).
    #[test]
    fn menu_entries() {
        let mut app = app();
        let pop = options_menu(&app.state, 0.0, 0.0);
        let edit = &pop.items.iter().find(|i| i.label == "Edit").expect("Edit").children;
        let keys: Vec<(&str, &str)> =
            edit.iter().filter(|i| i.label.contains("audio clips")).map(|i| (i.label.as_ref(), i.key)).collect();
        assert_eq!(
            keys,
            [
                ("Pitch selected audio clips one semitone down", "7"),
                ("Pitch selected audio clips one semitone up", "8"),
                ("Reverse selected audio clips", "9")
            ]
        );
        app.state.playlist.sel = vec![1];
        let m = clip_menu(&app.state.session, 0, 0.0, 0.0);
        let rev = m.items.iter().find(|i| i.label == "Reverse").expect("Reverse").clone();
        assert_eq!(rev.mark, Mark::Check(false));
        run(&mut app, rev.cmd);
        assert_eq!(
            clips(&app.state.session).iter().map(Clip::reversed).collect::<Vec<_>>(),
            [true, false, false],
            "clip 0 alone"
        );
        let m = clip_menu(&app.state.session, 0, 0.0, 0.0);
        assert_eq!(m.items.iter().find(|i| i.label == "Reverse").expect("Reverse").mark, Mark::Check(true));
        assert!(
            !clip_menu(&app.state.session, 2, 0.0, 0.0).items.iter().any(|i| i.label == "Reverse"),
            "not on a pattern clip"
        );
    }

    /// A `.lemon` file keeps each clip's pitch, Fine, pan and reverse in its own event; an FLP leaves them out (where FL
    /// keeps them is not measured), so its clip records stay FL's.
    #[test]
    fn settings_round_trip_through_a_lemon_file() {
        let mut app = app();
        {
            let c = &mut arr_mut(&mut app.state.session).clips[1];
            c.set_pitch_semitones(-5);
            c.set_fine_cents(-37);
            c.set_clip_pan(0.5);
            c.set_reversed(true);
        }
        let want = clips(&app.state.session)[1].props;
        let d = std::env::temp_dir().join(format!("os-app-clip-props-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for (name, kept) in [("Props.lemon", true), ("Props.flp", false)] {
            app.save_to(d.join(name));
            let mut back = App::new(None);
            back.open(d.join(name));
            let props: Vec<_> = clips(&back.state.session).iter().map(|c| c.props).collect();
            assert_eq!(props[0], Default::default(), "{name}");
            assert_eq!(props[1], if kept { want } else { Default::default() }, "{name}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The whole way through Lemon Sampler, as FILE ▸ Export audio plays it: a clip of a rising ramp reversed plays
    /// falling, and pitched an octave up plays it twice as fast. With `reopen` the project is saved as a `.lemon` file
    /// and opened in a new app first, which exports it without any edit (the reversed copy is switched on as the
    /// project's samplers start).
    #[test]
    fn reversed_and_pitched_clips_export_through_lemon_sampler() {
        use std::time::Duration;
        let fresh = || {
            let mut a = App::new(None);
            a.state.session.plugins = crate::plugins::Plugins::default();
            a.state.session.plugins.available = crate::plugins::builtin();
            a.state.session.plugins.set_rate(48000);
            a
        };
        let render = |rev: bool, semis: i32, reopen: bool| -> Vec<f32> {
            let mut a = fresh();
            let wav = std::env::temp_dir().join(format!("os-app-clip-rev-{}-{rev}-{semis}.wav", std::process::id()));
            let ramp: Vec<f32> = (0..24000).flat_map(|k| [k as f32 / 24000.0; 2]).collect();
            os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &ramp).unwrap();
            crate::playlist::add_audio_clip(&mut a.state, &wav.to_string_lossy(), 0, 0).expect("audio clip added");
            let row = a.state.session.project.channels.len() - 1;
            a.plugin_tick();
            assert!(a.state.session.hosted_rows.contains(&row), "Lemon Sampler hosts the audio clip channel");
            a.state.session.transport.set_instruments(a.state.session.plugins.instruments());
            a.state.playlist.sel = vec![clips(&a.state.session).len() - 1];
            if rev {
                run(&mut a, Cmd::ReverseSelected);
            }
            run(&mut a, Cmd::PitchSelected(semis));
            os_session::clip_reverse::sync(&a.state.session);
            let saved = std::env::temp_dir().join(format!("os-app-clip-rev-{}-{rev}-{semis}.lemon", std::process::id()));
            if reopen {
                a.save_to(saved.clone());
                a = fresh();
                a.open(saved.clone());
                let t0 = std::time::Instant::now();
                while a.state.session.loading.is_some() && t0.elapsed() < Duration::from_secs(30) {
                    a.plugin_tick();
                    std::thread::sleep(Duration::from_millis(5));
                }
                a.plugin_tick();
                assert!(clips(&a.state.session).iter().any(|c| c.reversed() == rev), "settings read back");
            }
            let p = crate::rack::play_project(&a.state);
            let out =
                std::env::temp_dir().join(format!("os-app-clip-rev-{}-{rev}-{semis}-export.wav", std::process::id()));
            a.state.session.plugins.set_offline(true);
            a.state.session.transport.export(&p, os_engine::PlayMode::Song, 48000, crate::export::Job::wav(&out));
            let t0 = std::time::Instant::now();
            loop {
                match a.state.session.transport.export_status() {
                    crate::transport::Export::Done(r) => break assert!(r.is_ok(), "{r:?}"),
                    _ if t0.elapsed() > Duration::from_secs(60) => panic!("export did not finish"),
                    _ => std::thread::sleep(Duration::from_millis(10)),
                }
            }
            let bytes = std::fs::read(&out).expect("file written");
            let _ = (std::fs::remove_file(&out), std::fs::remove_file(&wav), std::fs::remove_file(&saved));
            bytes[44..].chunks_exact(8).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
        };
        let plain = render(false, 0, false);
        let k = 0.2 / plain[4800];
        assert!(plain[4800] > 0.05, "forward plays: {}", plain[4800]);
        for reopen in [false, true] {
            let rev = render(true, 0, reopen);
            assert!((rev[4800] * k - 0.8).abs() < 0.02, "reversed (reopened: {reopen}), 0.1 s in: {}", rev[4800] * k);
            assert!(rev[9600] < rev[4800], "falling");
            let up = render(false, 12, reopen);
            assert!((up[4800] * k - 0.4).abs() < 0.02, "an octave up (reopened: {reopen}), 0.1 s in: {}", up[4800] * k);
        }
    }

    /// The title's pitch: whole semitones signed, cents as one decimal, nothing at 0.
    #[test]
    fn pitch_label_after_the_name() {
        let mut c = Clip::new(0, 384, 0, ClipSource::Channel(0));
        assert_eq!(pitch_label(&c), None);
        c.set_pitch_semitones(3);
        assert_eq!(pitch_label(&c).as_deref(), Some("+3"));
        c.set_pitch_semitones(-2);
        c.set_fine_cents(-50);
        assert_eq!(pitch_label(&c).as_deref(), Some("-2.5"));
        c.set_pitch_semitones(0);
        c.set_fine_cents(0);
        assert_eq!(pitch_label(&c), None);
    }

    /// Writes `.lemon` projects for checking the playlist's clip edits in the real app (`lemonstudio <file> --view
    /// playlist --wait --screenshot <png>`, `lemonstudio --render <file> <wav> --builtin-only`) into the folder named by
    /// `OS_PLAYLIST_CHECK_DIR`: `ramp.wav` (a rising 2 s ramp), `props.lemon` (the ramp plain on track 1, reversed and
    /// panned left on track 2, an octave up on track 3), `reversed.lemon`, `pitched.lemon`, `chop.lemon` (chopped by
    /// beat), `merge.lemon` (two pattern clips merged) and `clone.lemon` (track 1 cloned). Each project is opened again
    /// and saved as `<name>-resaved.lemon`; the clips' settings must survive.
    #[test]
    #[ignore]
    fn write_check_projects() {
        let Ok(dir) = std::env::var("OS_PLAYLIST_CHECK_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("ramp.wav");
        let ramp: Vec<f32> = (0..96000).flat_map(|k| [k as f32 / 96000.0; 2]).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &ramp).unwrap();
        let wav = wav.to_string_lossy().to_string();
        let save = |app: &mut App, name: &str| {
            // Windows are kept as f32 in the file, as in FL's.
            let path = dir.join(format!("{name}.lemon"));
            app.save_to(path.clone());
            let mut back = App::new(None);
            back.open(path);
            let of = |a: &App| {
                let mut v: Vec<_> =
                    clips(&a.state.session).iter().map(|c| (c.position, c.track, c.length, c.window.map(|(a, b)| (a as f32, b as f32)), c.source, c.props)).collect();
                v.sort_by_key(|x| (x.0, x.1));
                v
            };
            let (want, got) = (of(app), of(&back));
            assert_eq!(got, want, "{name} opens as saved");
            back.save_to(dir.join(format!("{name}-resaved.lemon")));
            eprintln!("{name}: {want:?}");
        };
        let audio = |tracks: &[(bool, i32, f32)]| {
            let mut a = App::new(None);
            for (t, &(rev, semis, pan)) in tracks.iter().enumerate() {
                let i = crate::playlist::add_audio_clip(&mut a.state, &wav, t as u16, 0).expect("audio clip added");
                let c = &mut arr_mut(&mut a.state.session).clips[i];
                c.set_reversed(rev);
                c.set_pitch_semitones(semis);
                c.set_clip_pan(pan);
            }
            a
        };
        save(&mut audio(&[(false, 0, 0.0), (true, 0, -0.5), (false, 12, 0.0)]), "props");
        save(&mut audio(&[(true, 0, 0.0)]), "reversed");
        save(&mut audio(&[(false, 12, 0.0)]), "pitched");
        let mut a = audio(&[(false, 0, 0.0)]);
        run(&mut a, Cmd::Chop(0, crate::clip_chop::Unit::Beat));
        assert!(clips(&a.state.session).len() > 1, "chopped");
        save(&mut a, "chop");

        let patterns = || {
            let mut a = App::new(None);
            let mut p = crate::projects::test_project();
            let ch = p.channels[0].index;
            p.patterns = vec![
                os_model::Pattern { number: 1, notes: vec![os_model::Note::step(ch, 0), os_model::Note::step(ch, 96)], ..Default::default() },
                os_model::Pattern { number: 2, notes: vec![os_model::Note::step(ch, 48)], ..Default::default() },
            ];
            a.state.session.project = crate::locked::Locked::new(p);
            a.state.session.control.settle(a.state.session.project.get_mut());
            arr_mut(&mut a.state.session).clips = vec![
                Clip::new(0, 384, 0, ClipSource::Pattern(1)),
                Clip::new(384, 384, 0, ClipSource::Pattern(2)),
                Clip::new(1536, 384, 1, ClipSource::Pattern(1)),
            ];
            arr_mut(&mut a.state.session).tracks[0].name = Some("Drums".into());
            a
        };
        let mut a = patterns();
        a.state.playlist.sel = vec![0, 1];
        run(&mut a, Cmd::MergeClips(false));
        assert!(clips(&a.state.session).iter().any(|c| c.source == ClipSource::Pattern(3)), "merged");
        save(&mut a, "merge");
        let mut a = patterns();
        run(&mut a, Cmd::CloneTrack(0));
        assert_eq!(clips(&a.state.session).len(), 5, "cloned");
        save(&mut a, "clone");
    }
}
