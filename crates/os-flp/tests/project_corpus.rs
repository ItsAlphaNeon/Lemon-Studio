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
