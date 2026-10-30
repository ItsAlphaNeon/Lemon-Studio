// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

// spec: flp/layouts.md "Checked against FL"
//
// Older project files against their resaves by FL 26 (OPENSTUDIO_FLP_PAIRS: a text file of lines `original<TAB>resave`;
// the files are never committed). Both are read; the original through its conversion to the current layout. They
// should give the same project, apart from what FL itself changes in a resave (flp/layouts.md lists it): the parts
// compared here leave those out, and the test reports how many pairs agree in each.

use os_flp::FlpFile;
use os_flp::project::{apply, load, upgrade};
use os_model::{ClipSource, Project};

fn pairs() -> Option<Vec<(String, String)>> {
    let list = std::env::var_os("OPENSTUDIO_FLP_PAIRS")?;
    let text = std::fs::read_to_string(list).expect("pair list readable");
    Some(text.lines().filter_map(|l| l.split_once('\t')).map(|(a, b)| (a.trim().to_string(), b.trim().to_string())).collect())
}

/// The parts of a project a resave keeps, each as text to compare.
fn parts(p: &Project) -> (Vec<(&'static str, String)>, Vec<Option<String>>) {
    // FL gives a channel without a name its default one, and writes `%FLStudioData%` paths as
    // `%FLStudioFactoryData%/Data` ones (flp/layouts.md "Older files"): names are compared where the original has one.
    let path = |s: &str| s.to_lowercase().replace('\\', "/").replace("%flstudiodata%/", "%flstudiofactorydata%/data/");
    let channels: Vec<String> = p
        .channels
        .iter()
        .map(|c| format!("{} {:?} {:?} {} {} {:.2} {:.2} {:?}", c.index, c.kind, c.plugin, c.mixer_track, c.enabled, c.volume, c.pan, c.sample_path.as_deref().map(path)))
        .collect();
    let names: Vec<Option<String>> = p.channels.iter().map(|c| c.name.clone()).collect();
    let mut notes: Vec<String> = p
        .patterns
        .iter()
        .filter(|pat| !pat.notes.is_empty())
        .map(|pat| {
            let n: Vec<String> = pat.notes.iter().map(|n| format!("{}:{}:{}:{}:{}", n.position, n.channel, n.length, n.key, n.velocity)).collect();
            format!("{} {}", pat.number, n.join(","))
        })
        .collect();
    notes.sort();
    let clips: Vec<String> = p
        .arrangements
        .iter()
        .map(|a| {
            let mut c: Vec<String> = a
                .clips
                .iter()
                .map(|c| {
                    let src = match c.source {
                        ClipSource::Pattern(n) => format!("p{n}"),
                        ClipSource::Channel(n) => format!("c{n}"),
                    };
                    format!("{}:{}:{}:{}:{}", c.position, c.track, src, c.muted, c.raw[52..56] == [0; 4])
                })
                .collect();
            c.sort();
            c.join(",")
        })
        .collect();
    // FL grows a 105-track mixer to 127; the tracks both have are compared, the current track by its own number.
    let mixer: Vec<String> = p
        .mixer
        .iter()
        .take(104)
        .map(|m| {
            let slots: Vec<String> = m.slots.iter().map(|s| s.as_ref().map_or(String::new(), |s| s.name.clone())).collect();
            // FL writes level records for routes that do not exist and drops them in a resave: only routed ones count.
            let mut sends: Vec<(u16, f64)> = m.send_levels.iter().copied().filter(|(d, _)| m.routes.contains(d)).collect();
            sends.sort_by(|a, b| a.partial_cmp(b).unwrap());
            format!(
                "{} {:?} {:?} {:?} {:?} {:?} {} {:?} {:?} {:?}",
                m.index, m.name, m.volume, m.pan, m.routes, slots, m.enabled, m.slot_on, m.latency_out + 0.0, sends
            )
        })
        .collect();
    let mut links: Vec<String> = p.automations.iter().map(|a| format!("{} {:?}", a.channel, { let mut t = a.targets.clone(); t.sort(); t })).collect();
    links.sort();
    let parts = vec![
        ("tempo", format!("{:?} {} {:?}", p.tempo, p.ppq, p.bar)),
        ("channels", channels.join("\n")),
        ("notes", notes.join("\n")),
        ("clips", clips.join("\n")),
        ("mixer", mixer.join("\n")),
        ("automation", links.join("\n")),
    ];
    (parts, names)
}

#[test]
fn older_files_read_as_their_resaves() {
    let Some(pairs) = pairs() else { return };
    let mut failures = Vec::new();
    let mut agree = std::collections::BTreeMap::<&str, usize>::new();
    let mut whole = 0;
    let mut differ = Vec::new();
    for (orig, resave) in &pairs {
        let read = |p: &str| FlpFile::parse(&std::fs::read(p).unwrap()).unwrap();
        let mut a = read(orig);
        if let Err(e) = upgrade(&mut a) {
            failures.push(format!("{orig}: upgrade: {e}"));
            continue;
        }
        let (pa, pb) = match (load(&a), load(&read(resave))) {
            (Ok(pa), Ok(pb)) => (pa, pb),
            (a, b) => {
                failures.push(format!("{orig}: load: {:?} / {:?}", a.err(), b.err()));
                continue;
            }
        };
        // The converted file saves: unedited it re-serializes as converted.
        let bytes = a.to_bytes().unwrap();
        if apply(&pa, &mut a).is_err() || a.to_bytes().unwrap() != bytes {
            failures.push(format!("{orig}: unedited save changed the converted file"));
        }
        let ((x, na), (y, nb)) = (parts(&pa), parts(&pb));
        let mut all = true;
        let named = na.len() == nb.len() && na.iter().zip(&nb).all(|(a, b)| a.is_none() || a == b);
        if named {
            *agree.entry("names").or_default() += 1;
        } else {
            all = false;
            differ.push(format!("{orig}: names"));
        }
        for ((name, u), (_, v)) in x.iter().zip(&y) {
            if u == v {
                *agree.entry(name).or_default() += 1;
            } else {
                all = false;
                let first = u.lines().zip(v.lines()).find(|(a, b)| a != b).map_or_else(
                    || format!("{} / {} lines", u.lines().count(), v.lines().count()),
                    |(a, b)| {
                        // From a little before the first character that differs.
                        let at = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count().saturating_sub(40);
                        let cut = |s: &str| s.chars().skip(at).take(120).collect::<String>();
                        format!("{}  <>  {}", cut(a), cut(b))
                    },
                );
                differ.push(format!("{orig}: {name}: {first}"));
            }
        }
        whole += usize::from(all);
    }
    eprintln!("{} pairs, {whole} agree in every part compared; by part: {agree:?}", pairs.len());
    if std::env::var_os("OPENSTUDIO_FLP_PAIRS_VERBOSE").is_some() {
        eprintln!("{}", differ.join("\n"));
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}
