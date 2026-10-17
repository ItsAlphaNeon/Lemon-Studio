// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

// spec: flp/events/*.md
//
// Typed-model tests over the corpus (OPENSTUDIO_FLP_CORPUS, see tests/corpus.rs):
// 1. load → apply (no edits) → bytes identical to the input;
// 2. edits survive apply → to_bytes → parse → load;
// 3. structural sanity (channel count matches the header, notes sorted, …).

use os_flp::FlpFile;
use os_flp::project::{ProjectError, apply, load};
use os_model::Rgb;

fn corpus() -> Option<Vec<String>> {
    let list = std::env::var_os("OPENSTUDIO_FLP_CORPUS")?;
    let text = std::fs::read_to_string(list).expect("corpus list readable");
    Some(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
}

#[test]
fn unedited_model_round_trips_and_is_sane() {
    let Some(paths) = corpus() else { return };
    let mut failures = Vec::new();
    let mut legacy = 0;
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        let mut file = FlpFile::parse(&bytes).unwrap();
        let project = match load(&file) {
            Ok(p) => p,
            Err(ProjectError::LegacyLayout(_)) => {
                legacy += 1;
                continue;
            }
            Err(e) => {
                failures.push(format!("{path}: load: {e}"));
                continue;
            }
        };
        let header_channels = file.header().and_then(|h| h.channel_count()).unwrap();
        if project.channels.len() != usize::from(header_channels) {
            failures.push(format!("{path}: {} channels, header says {header_channels}", project.channels.len()));
        }
        for (n, ch) in project.channels.iter().enumerate() {
            if usize::from(ch.index) != n {
                failures.push(format!("{path}: channel {n} has index {}", ch.index));
                break;
            }
        }
        for p in &project.patterns {
            if !p.notes.windows(2).all(|w| w[0].position <= w[1].position) {
                failures.push(format!("{path}: pattern {} notes unsorted", p.number));
            }
        }
        if let Err(e) = apply(&project, &mut file) {
            failures.push(format!("{path}: apply: {e}"));
            continue;
        }
        if file.to_bytes().unwrap() != bytes {
            failures.push(format!("{path}: unedited apply changed the bytes"));
        }
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    eprintln!("{} files: load/apply round-trip OK; {legacy} legacy-layout files skipped", paths.len() - legacy);
}

#[test]
fn edits_survive_a_write_read_cycle() {
    let Some(paths) = corpus() else { return };
    let mut checked = 0;
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        let mut file = FlpFile::parse(&bytes).unwrap();
        let Ok(mut project) = load(&file) else { continue };
        if project.saved_by.split('.').next() != Some("26") || project.channels.is_empty() {
            continue;
        }
        project.tempo = Some(123.456);
        let ch = &mut project.channels[0];
        ch.name = Some("Edited Ω".into());
        ch.volume = 0.5;
        ch.pan = -0.25;
        ch.pitch_cents = -150;
        ch.color = Some(Rgb::new(0x10, 0x80, 0xFF));
        ch.enabled = !ch.enabled;
        if let Some(t) = project.mixer.get_mut(1) {
            t.name = Some("Edited mix".into());
            t.color = Some(Rgb::new(1, 2, 3));
            t.enabled = !t.enabled;
            if t.volume.is_some() {
                t.volume = Some(0.25);
            }
            // Routing: a second destination with a send level (event 235 + route-level records).
            if t.routes == [0] {
                t.routes = vec![0, 3];
                t.send_levels = vec![(3, 0.5)];
            }
        }
        if let Some(t) = project.arrangements.get_mut(0).and_then(|a| a.tracks.get_mut(2)) {
            t.name = Some("Edited track".into());
            t.enabled = false;
            t.color = Some(Rgb::new(9, 8, 7));
        }
        if let Some(p) = project.patterns.iter_mut().find(|p| !p.notes.is_empty()) {
            p.notes[0].velocity = 7;
            p.notes[0].key += 1;
        }

        apply(&project, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let reread = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();

        // The channel color is clamped on write, as FL does.
        let mut expected = project.clone();
        expected.channels[0].color = Some(Rgb::new(0x14, 0x80, 0xFF));
        // The colour the file holds now (shown on the channel's playlist clips).
        expected.channels[0].rack.stored_color = expected.channels[0].color;
        for p in &mut expected.patterns {
            p.notes.sort_by_key(|n| n.position);
        }
        assert_eq!(reread.tempo, expected.tempo, "{path}");
        assert_eq!(reread.channels, expected.channels, "{path}");
        assert_eq!(reread.mixer, expected.mixer, "{path}");
        assert_eq!(reread.arrangements, expected.arrangements, "{path}");
        assert_eq!(reread.patterns, expected.patterns, "{path}");
        checked += 1;
    }
    eprintln!("{checked} FL 26 files: edits survived write/read");
}

/// Playlist edits (spec: flp/events/playlist.md, automation.md): clips moved, muted, trimmed, copied, deleted;
/// markers added; automation points changed; a new automation channel. Everything reads back.
#[test]
fn playlist_edits_survive() {
    use os_model::{AutoPoint, ChannelKind, Clip, ClipSource, TimeMarker};
    let Some(paths) = corpus() else { return };
    let strip = |v: &[Clip]| {
        let mut v: Vec<_> = v.iter().map(|c| (c.position, c.length, c.track, c.source, c.muted, c.window.map(|(a, b)| ((a * 1000.0) as i64, (b * 1000.0) as i64)))).collect();
        v.sort_by_key(|c| (c.0, c.2, c.1));
        v
    };
    let mut checked = 0;
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        let mut file = FlpFile::parse(&bytes).unwrap();
        let Ok(mut project) = load(&file) else { continue };
        // FL 24.2 and 25 files too: their clip records are 80 bytes (spec: flp/layouts.md).
        if !matches!(project.saved_by.split('.').next(), Some("24" | "25" | "26")) || project.arrangements.first().is_none_or(|a| a.clips.len() < 3) {
            continue;
        }
        let arr = &mut project.arrangements[0];
        arr.clips[0].position += 96;
        arr.clips[0].muted = !arr.clips[0].muted;
        let mut copy = arr.clips[1];
        copy.track = (copy.track + 1).min(499);
        copy.position += 384;
        arr.clips.push(copy);
        arr.clips.remove(2);
        if let ClipSource::Pattern(_) = arr.clips[1].source {
            arr.clips[1].window = Some((48.0, 48.0 + f64::from(arr.clips[1].length)));
        }
        arr.markers.push(TimeMarker::new(1536, "Added"));
        if let Some(a) = project.automations.first_mut() {
            if a.points.len() >= 2 {
                a.points[1].value = 0.123;
            } else {
                a.points.push(AutoPoint::new(4.0, 0.5));
            }
        }
        let next = project.channels.len() as u16;
        let mut ch = project.channels[0].clone();
        ch.rack.origin = None;
        ch.index = next;
        ch.kind = ChannelKind::Automation;
        ch.name = Some("Added automation".into());
        ch.sample_path = None;
        ch.layer_children.clear();
        project.channels.push(ch);
        project.automations.push(os_flp::project::new_automation(next, os_model::target::channel(0, 1), 0.5, 16.0));

        apply(&project, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let reread = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(strip(&reread.arrangements[0].clips), strip(&project.arrangements[0].clips), "{path}");
        assert_eq!(reread.arrangements[0].markers, project.arrangements[0].markers, "{path}");
        assert_eq!(reread.automations, project.automations, "{path}");
        assert_eq!(reread.channels.len(), project.channels.len(), "{path}");
        checked += 1;
    }
    assert!(checked > 0);
    eprintln!("{checked} files: playlist edits survived");
}

/// Control points (event 223, spec: flp/events/patterns.md): adding, rewriting and removing them
/// leaves the notes alone, and removing them again restores the original bytes.
#[test]
fn control_points_survive_edits() {
    use os_model::ControlPoint;
    let Some(paths) = corpus() else { return };
    let mut checked = 0;
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        let mut file = FlpFile::parse(&bytes).unwrap();
        let Ok(original) = load(&file) else { continue };
        let Some(pi) = original.patterns.iter().position(|p| !p.notes.is_empty() && p.controls.is_empty()) else {
            continue;
        };
        let mut project = original.clone();
        project.patterns[pi].controls = vec![
            ControlPoint::new(48, 0, ControlPoint::PAN, 3200),
            ControlPoint::new(0, 0, ControlPoint::VOLUME, 12800),
            ControlPoint::new(0, 1, ControlPoint::PITCH, -37),
        ];
        apply(&project, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let mut file2 = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        let reread = load(&file2).unwrap();
        let got = &reread.patterns[pi];
        assert_eq!(got.notes, original.patterns[pi].notes, "{path}: notes unchanged");
        assert_eq!(got.controls.len(), 3, "{path}");
        assert!(got.controls.windows(2).all(|w| w[0].position <= w[1].position), "{path}: sorted");

        // Rewrite one value in place, then remove them all.
        let mut project = reread.clone();
        project.patterns[pi].controls[0].value = 777;
        apply(&project, &mut file2).unwrap();
        let reread = load(&FlpFile::parse(&file2.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(reread.patterns[pi].controls[0].value, 777, "{path}");
        project.patterns[pi].controls.clear();
        apply(&project, &mut file2).unwrap();
        assert_eq!(file2.to_bytes().unwrap(), bytes, "{path}: removing the points restores the file");
        checked += 1;
    }
    eprintln!("{checked} files: control points added, rewritten and removed");
}

/// Channel envelopes (event 218, spec: audio/channel-envelopes.md): every channel reads five records with
/// values inside the measured ranges, and some files switch the volume envelope on.
#[test]
fn channel_envelopes_decode() {
    let Some(paths) = corpus() else { return };
    let (mut vol_on, mut pitch_on) = (0, 0);
    for path in &paths {
        let file = FlpFile::parse(&std::fs::read(path).unwrap()).unwrap();
        let Ok(project) = load(&file) else { continue };
        for ch in &project.channels {
            for e in &ch.envelopes {
                assert!((0..=128).contains(&e.sustain), "{path}: sustain {}", e.sustain);
                for t in [e.attack_tension, e.decay_tension, e.release_tension] {
                    assert!((-128..=128).contains(&t), "{path}: tension {t}");
                }
                assert!((0..=65536).contains(&e.attack) && (0..=65536).contains(&e.release), "{path}: time");
            }
            vol_on += usize::from(ch.envelopes[os_model::Envelope::VOLUME].enabled);
            pitch_on += usize::from(ch.envelopes[os_model::Envelope::PITCH].enabled);
        }
    }
    assert!(vol_on > 50 && pitch_on > 5, "volume {vol_on}, pitch {pitch_on}");
}

/// Patterns that have no blocks yet gain note, control and metadata blocks (spec: flp/events/patterns.md,
/// "Where blocks go"): the new patterns read back as written, the others stay as they were, and the blocks keep the
/// order FL writes them in (note blocks ascending before the first channel, metadata after the first channel).
#[test]
fn new_pattern_blocks_are_added() {
    use os_model::{ControlPoint, Note, Pattern};
    let Some(paths) = corpus() else { return };
    let mut checked = 0;
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        let mut file = FlpFile::parse(&bytes).unwrap();
        let Ok(original) = load(&file) else { continue };
        if original.channels.is_empty() {
            continue;
        }
        let max = original.patterns.iter().map(|p| p.number).max().unwrap_or(0);
        // A gap (a pattern number that has no blocks) in the middle when there is one, then two past the end.
        let mut numbers: Vec<u16> = (1..max).filter(|n| original.patterns.iter().all(|p| p.number != *n)).take(1).collect();
        numbers.extend([max + 1, max + 2]);
        let mut project = original.clone();
        for (k, &n) in numbers.iter().enumerate() {
            project.patterns.push(Pattern {
                number: n,
                name: (k != 1).then(|| format!("New {n}")),
                color: (k != 2).then(|| Rgb::new(10 + k as u8, 20, 30)),
                notes: vec![Note::step(0, 24), Note::step(0, 0)],
                controls: if k == 0 { vec![ControlPoint::new(0, 0, ControlPoint::VOLUME, 6400)] } else { vec![] },
                ..Pattern::default()
            });
        }
        apply(&project, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let file2 = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        let reread = load(&file2).unwrap_or_else(|e| panic!("{path}: reload: {e}"));
        for p in &original.patterns {
            assert_eq!(reread.patterns.iter().find(|q| q.number == p.number), Some(p), "{path}: pattern {} unchanged", p.number);
        }
        for want in project.patterns.iter().filter(|p| numbers.contains(&p.number)) {
            let got = reread.patterns.iter().find(|q| q.number == want.number).unwrap_or_else(|| panic!("{path}: pattern {} lost", want.number));
            let mut sorted = want.notes.clone();
            sorted.sort_by_key(|n| n.position);
            assert_eq!(got.notes, sorted, "{path}: notes of {}", want.number);
            assert_eq!(got.controls, want.controls, "{path}: controls of {}", want.number);
            assert_eq!(got.name, want.name, "{path}: name of {}", want.number);
            if want.color.is_some() {
                assert_eq!(got.color, want.color, "{path}: colour of {}", want.number);
            }
        }
        // Order: pattern note blocks ascend and stay before the first channel.
        let events = file2.chunks.iter().find_map(|c| match &c.body {
            os_flp::ChunkBody::Events(e) => Some(e),
            _ => None,
        });
        if let Some(ev) = events {
            let first_channel = ev.iter().position(|e| e.id == 64).unwrap();
            let blocks: Vec<(usize, u32)> = ev
                .iter()
                .enumerate()
                .filter(|(i, e)| e.id == 65 && matches!(ev.get(i + 1).map(|n| n.id), Some(223 | 224)))
                .map(|(i, e)| (i, e.int_value().unwrap()))
                .collect();
            assert!(blocks.iter().all(|&(i, _)| i < first_channel), "{path}: note blocks before the first channel");
            assert!(blocks.windows(2).all(|w| w[0].1 < w[1].1), "{path}: note blocks ascend");
        }
        checked += 1;
    }
    eprintln!("{checked} files: new pattern blocks added");
}

/// Manual helper: `OS_IN=a.flp OS_OUT=b.flp cargo test -p os-flp --test project_corpus -- --ignored new_pattern_file`
/// writes `a.flp` with two new patterns (notes, points, name, colour) so FL can be asked to open it.
#[test]
#[ignore]
fn new_pattern_file() {
    use os_model::{ControlPoint, Note, Pattern, Rgb};
    let (Some(i), Some(o)) = (std::env::var_os("OS_IN"), std::env::var_os("OS_OUT")) else { return };
    let mut file = FlpFile::parse(&std::fs::read(i).unwrap()).unwrap();
    let mut project = load(&file).unwrap();
    let max = project.patterns.iter().map(|p| p.number).max().unwrap_or(0);
    for k in 1..=2u16 {
        project.patterns.push(Pattern {
            number: max + k,
            name: Some(format!("Lemon Studio {k}")),
            color: Some(Rgb::new(200, 60 * k as u8, 40)),
            notes: (0..4).map(|s| Note { key: 60 + 4 * s as u16 * k, length: 48, ..Note::step(0, s * 96) }).collect(),
            controls: vec![ControlPoint::new(0, 0, ControlPoint::VOLUME, 6400)],
            ..Pattern::default()
        });
    }
    apply(&project, &mut file).unwrap();
    std::fs::write(o, file.to_bytes().unwrap()).unwrap();
}

fn settled(mut p: os_model::Project) -> os_model::Project {
    for (i, c) in p.channels.iter_mut().enumerate() {
        c.rack.origin = Some(i as u16);
    }
    p
}

/// Channel rack settings written into every FL 26 file read back (spec: flp/events/project-info.md, channels.md,
/// patterns.md): swing, a new filter group, lock, zip, an explicit length and loop lengths.
#[test]
fn rack_settings_survive() {
    let Some(paths) = corpus() else { return };
    let mut checked = 0;
    for path in &paths {
        let mut file = FlpFile::parse(&std::fs::read(path).unwrap()).unwrap();
        let Ok(mut project) = load(&file) else { continue };
        if project.channels.len() < 2 {
            continue;
        }
        project.swing = Some(77);
        project.groups.insert(0, "Added".into());
        for c in &mut project.channels {
            c.rack.group += 1;
        }
        project.channels[0].rack.group = 0;
        project.current_group = Some(0);
        project.channels[0].rack.locked = !project.channels[0].rack.locked;
        project.channels[1].rack.zipped = !project.channels[1].rack.zipped;
        if let Some(pat) = project.patterns.iter_mut().find(|p| !p.notes.is_empty()) {
            pat.length = Some(96 * 3);
            pat.loops = vec![(0, 3)];
            pat.loop_off = Some(3);
        }
        apply(&project, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back, project, "{path}");
        checked += 1;
    }
    assert!(checked > 40, "only {checked} files checked");
}

/// Project info edited in every file (spec: flp/events/project-info.md): each field changed, or set where the file has
/// none (comments and URL are missing in many), reads back with nothing else changed. A file whose information events
/// stand in FL's order (spec: flp/engine-events.md "Order in the file": 194, 206, 207, 167, 202, 195, 197, 237) keeps
/// it.
#[test]
fn project_info_edits_survive() {
    let Some(paths) = corpus() else { return };
    // Where each of the information events first stands, in FL's order: whether they ascend.
    let fl_order = |file: &FlpFile| {
        let ev = file
            .chunks
            .iter()
            .find_map(|c| match &c.body {
                os_flp::ChunkBody::Events(e) => Some(e),
                _ => None,
            })
            .unwrap();
        let at: Vec<usize> = [194u8, 206, 207, 167, 202, 195, 197, 237].iter().filter_map(|&id| ev.iter().position(|e| e.id == id)).collect();
        at.windows(2).all(|w| w[0] < w[1])
    };
    let mut checked = 0;
    for path in &paths {
        let mut file = FlpFile::parse(&std::fs::read(path).unwrap()).unwrap();
        let Ok(mut project) = load(&file) else { continue };
        let ordered = fl_order(&file);
        project.info = os_model::ProjectInfo {
            title: Some("Edited title".into()),
            author: Some("Edited author".into()),
            genre: Some("Edited genre".into()),
            comments: Some("Line one\nLine two".into()),
            url: Some("https://example.org/edited".into()),
        };
        apply(&project, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        let back = load(&file).unwrap();
        assert_eq!(back, project, "{path}");
        assert!(!ordered || fl_order(&file), "{path}: information events out of FL's order");
        checked += 1;
    }
    assert!(checked > 40, "only {checked} files checked");
}

/// Track switches, main volume and main pitch in every file, old 5- and 8-byte track records included (spec:
/// flp/layouts.md "A mixer track's record (event 236)").
#[test]
fn mixer_switches_and_main_values_survive() {
    let Some(paths) = corpus() else { return };
    let mut checked = 0;
    for path in &paths {
        let mut file = FlpFile::parse(&std::fs::read(path).unwrap()).unwrap();
        let Ok(mut project) = load(&file) else { continue };
        if project.mixer.len() < 3 {
            continue;
        }
        project.mixer[1].polarity = !project.mixer[1].polarity;
        project.mixer[1].swap = !project.mixer[1].swap;
        project.mixer[2].slots_enabled = !project.mixer[2].slots_enabled;
        project.main_pitch = Some(-450);
        if project.main_volume.is_some() {
            project.main_volume = Some(9000);
        }
        apply(&project, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back, project, "{path}");
        checked += 1;
    }
    assert!(checked > 40, "only {checked} files checked");
}

/// Restructured channels in every FL 26 file (spec: flp/events/channels.md "Deleting, cloning and moving"):
/// swapping the first two channels writes the swapped model, swapping back gives the original bytes; deleting
/// the second channel and cloning the first reads back as the model.
#[test]
fn restructured_channels_survive() {
    let Some(paths) = corpus() else { return };
    let mut checked = 0;
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        let mut file = FlpFile::parse(&bytes).unwrap();
        let Ok(project) = load(&file) else { continue };
        let n = project.channels.len();
        if n < 3 {
            continue;
        }
        let mut order: Vec<usize> = (0..n).collect();
        order.swap(0, 1);
        let mut swapped = project.clone();
        swapped.reorder_channels(&order);
        apply(&swapped, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = load(&file).unwrap();
        let want = settled(swapped);
        assert_eq!(back, want, "{path}");
        let mut again = back.clone();
        again.reorder_channels(&order);
        apply(&again, &mut file).unwrap();
        assert!(file.to_bytes().unwrap() == bytes, "{path}: swapping back changed the file");

        let mut edited = project.clone();
        let mut order: Vec<usize> = (0..n).filter(|&i| i != 1).collect();
        order.insert(1, 0);
        edited.reorder_channels(&order);
        let mut file = FlpFile::parse(&bytes).unwrap();
        apply(&edited, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        let want = settled(edited);
        assert_eq!(back, want, "{path}");
        checked += 1;
    }
    assert!(checked > 50, "only {checked} files checked");
}

/// Mixer slot plugin states written into every FL 26 file read back (spec: flp/events/mixer.md "Slot sub-block"):
/// each effect state gets a changed last byte and one extra byte, and reads back so.
#[test]
fn slot_plugin_states_survive() {
    let Some(paths) = corpus() else { return };
    let mut checked = 0;
    for path in &paths {
        let mut file = FlpFile::parse(&std::fs::read(path).unwrap()).unwrap();
        let Ok(mut project) = load(&file) else { continue };
        let mut any = false;
        for s in project.mixer.iter_mut().flat_map(|m| m.slots.iter_mut().flatten()) {
            // A plugin without a state event (some analysers) keeps having none.
            if let Some(b) = s.state.last_mut() {
                *b ^= 0x5a;
                s.state.push(7);
                any = true;
            }
        }
        if !any {
            continue;
        }
        apply(&project, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back, project, "{path}");
        assert!(os_flp::project::unsaved_parts(&project, &file).is_empty(), "{path}");
        checked += 1;
    }
    assert!(checked > 20, "only {checked} files checked");
}

/// Plugin channel states (event 213, spec: flp/events/plugins.md) written into every FL 26 file read back, and a new
/// wrapped plugin channel appended to each file reads back with its plugin and state.
#[test]
fn plugin_channel_states_survive() {
    use os_model::{ChannelKind, PluginRef};
    let Some(paths) = corpus() else { return };
    let mut checked = 0;
    for path in &paths {
        let mut file = FlpFile::parse(&std::fs::read(path).unwrap()).unwrap();
        let Ok(mut project) = load(&file) else { continue };
        let mut any = false;
        for c in project.channels.iter_mut().filter(|c| c.kind == ChannelKind::Plugin) {
            assert!(c.plugin_state.is_some(), "{path}: plugin channel {} without a state", c.index);
            if let Some(b) = c.plugin_state.as_mut().and_then(|s| s.last_mut()) {
                *b ^= 0x5a;
            }
            c.plugin_state.as_mut().unwrap().push(3);
            any = true;
        }
        if !any || project.mixer.len() < 3 {
            continue;
        }
        let mut ch = project.channels[0].clone();
        ch.index = project.channels.len() as u16;
        ch.kind = ChannelKind::Plugin;
        ch.name = Some("Hosted synth".into());
        ch.plugin = Some(os_flp::wrapper::WRAPPER.into());
        ch.sample_path = None;
        ch.layer_children.clear();
        ch.envelopes = Default::default();
        ch.rack = Default::default();
        // Reads back with the colour written for it: its own, else the default channel colour.
        ch.rack.stored_color = Some(ch.color.unwrap_or(os_model::Rgb::new(0x30, 0x33, 0x34)));
        // A new channel's block stores none of the Misc / sound settings yet (they read back as defaults).
        ch.misc = Default::default();
        ch.sound = Default::default();
        ch.mixer_track = 2;
        let p = PluginRef {
            format: "clap".into(),
            name: "Synth".into(),
            vendor: "Maker".into(),
            path: "C:/CLAP/Synth.clap".into(),
            state: vec![1, 2, 3],
            ..Default::default()
        };
        ch.plugin_state = Some(os_flp::wrapper::encode(None, &p));
        project.channels.push(ch);
        apply(&project, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        let want = settled(project);
        assert_eq!(back.channels, want.channels, "{path}");
        let last = back.channels.last().unwrap();
        assert_eq!(os_flp::wrapper::decode(last.plugin_state.as_deref().unwrap()), Some(p), "{path}");
        checked += 1;
    }
    assert!(checked > 20, "only {checked} files checked");
}

/// Mixer slot edits in every FL 26 file (spec: flp/events/plugins.md §2-5): slot plugins moved within a track and to
/// another track, one removed, one added, LEDs and mix levels changed. Everything reads back; moving the plugins
/// back and restoring the LEDs gives the original bytes.
#[test]
fn slot_edits_survive() {
    use os_model::SlotPlugin;
    let Some(paths) = corpus() else { return };
    let mut checked = 0;
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        let mut file = FlpFile::parse(&bytes).unwrap();
        let Ok(project) = load(&file) else { continue };
        // A track with a plugin and an empty slot, and a second track with an empty slot.
        let Some(t) = project.mixer.iter().position(|m| m.slots.iter().any(Option::is_some) && m.slots.iter().any(Option::is_none)) else {
            continue;
        };
        let Some(u) = project.mixer.iter().position(|m| m.index != project.mixer[t].index && m.slots[9].is_none()) else { continue };
        let from = project.mixer[t].slots.iter().position(Option::is_some).unwrap();
        let to = project.mixer[t].slots.iter().position(Option::is_none).unwrap();

        // Move within the track (with its LED and mix), and back: the original bytes.
        let mut moved = project.clone();
        let m = &mut moved.mixer[t];
        m.slots.swap(from, to);
        m.slot_on.swap(from, to);
        m.slot_mix.swap(from, to);
        m.slot_on[from] = false;
        m.slot_mix[to] = 0.25;
        apply(&moved, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back.mixer, moved.mixer, "{path}");
        apply(&project, &mut file).unwrap();
        assert!(file.to_bytes().unwrap() == bytes, "{path}: moving back changed the file");

        // Move to another track, remove one, add one.
        let mut edited = project.clone();
        let p = edited.mixer[t].slots[from].take().unwrap();
        edited.mixer[u].slots[9] = Some(p);
        edited.mixer[t].slots[to] = Some(SlotPlugin { name: "Fruity Parametric EQ 2".into(), state: vec![1, 2, 3, 4] });
        if let Some(k) = (0..10).find(|&k| k != to && edited.mixer[u].slots[k].is_some() && k != 9) {
            edited.mixer[u].slots[k] = None;
        }
        apply(&edited, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back.mixer, edited.mixer, "{path}");
        assert!(os_flp::project::unsaved_parts(&edited, &file).is_empty(), "{path}");
        checked += 1;
    }
    assert!(checked > 20, "only {checked} files checked");
}

/// Mixer tracks added before the last ("current") track: the old tracks and the current track read back unchanged,
/// the new tracks are default inserts that take edits (spec: flp/events/mixer.md "Adding tracks").
#[test]
fn added_mixer_tracks_survive() {
    use os_model::MixerTrack;
    let Some(paths) = corpus() else { return };
    let mut checked = 0;
    for path in &paths {
        let mut file = FlpFile::parse(&std::fs::read(path).unwrap()).unwrap();
        let Ok(project) = load(&file) else { continue };
        let n = project.mixer.len();
        if n < 2 || n > 200 {
            continue;
        }
        let mut grown = project.clone();
        let mut current = grown.mixer.pop().unwrap();
        current.index = n as u16 + 1;
        grown.mixer.extend((n as u16 - 1..n as u16 + 1).map(MixerTrack::new));
        grown.mixer.push(current);
        grown.mixer[n].name = Some("Added".into());
        grown.mixer[n].volume = Some(0.5);
        grown.mixer[n].slot_on[2] = false;
        apply(&grown, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back.mixer, grown.mixer, "{path}");
        assert_eq!(back.channels, project.channels, "{path}");
        checked += 1;
    }
    assert!(checked > 100, "only {checked} files checked");
}

/// Every automation link (event 227 with a zero first field) is read into its clip's targets in file order, and a
/// changed order is written over the same links (spec: flp/events/automation.md "Event 227").
#[test]
fn every_automation_link_is_read() {
    let Some(paths) = corpus() else { return };
    let (mut files, mut several) = (0, 0);
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        let mut file = FlpFile::parse(&bytes).unwrap();
        let Ok(mut project) = load(&file) else { continue };
        let links = |f: &FlpFile| -> Vec<(u16, u32)> {
            f.events()
                .filter(|e| e.id == 227)
                .filter_map(|e| e.data())
                .filter(|d| d.len() >= 20 && d[0] == 0 && d[1] == 0)
                .map(|d| (u16::from_le_bytes([d[2], d[3]]), u32::from_le_bytes(d[8..12].try_into().unwrap())))
                .collect()
        };
        let all = links(&file);
        for a in &project.automations {
            let want: Vec<u32> = all.iter().filter(|l| l.0 == a.channel).map(|l| l.1).collect();
            assert_eq!(a.targets, want, "{path}: clip channel {}", a.channel);
        }
        if !project.automations.iter().any(|a| a.targets.len() > 1) {
            continue;
        }
        several += 1;
        let total = file.events().count();
        for a in &mut project.automations {
            a.targets.reverse();
        }
        apply(&project, &mut file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        assert_eq!(file.events().count(), total, "{path}: links patched in place");
        assert_eq!(load(&file).unwrap().automations, project.automations, "{path}");
        files += 1;
    }
    eprintln!("{files} files with clips on several targets rewritten ({several} found)");
}
