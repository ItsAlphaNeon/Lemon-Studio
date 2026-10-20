// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Projects saved before FL 24.2, converted to the newer layout in memory before they are read, the way FL 26 does
//! when it opens and saves such a file (spec: flp/layouts.md, "What differs" and "Saving a file of an older
//! generation", converting). A converted project is then read and written as a newer one; saving it in its own
//! older layout is not done.
//!
//! What is converted, by the table of `flp/layouts.md`: channel blocks get their end event (51); a channel's mixer
//! track moves from event 22 to event 104; playlist clip records grow to 88 bytes and track records to 70; mixer
//! places move from 0x2000 to 0x7000, with the last track becoming the "current" track 501; send levels take the newer
//! record form and link id; the stretch length of event 215 becomes a float; the mixer gets its track count (103).
//! Not converted: the rewrites FL also makes for FL 10 / 11 files (step dots, their link ids, wrapped Fruity effects).
//!
//! Only files saved by FL 20 or later are converted ([`OLDEST`]): checked against FL 26's own conversions of the
//! projects FL bundles, those read as FL reads them, while files of FL 12.5 and before lose their playlist and mixer
//! here. Older files stay refused (`ProjectError::LegacyLayout`).

use crate::container::{ChunkBody, FlpFile};
use crate::event::{Event, Value};
use super::id;

const OLD_BASE: u16 = 0x2000;
/// A mixer track's icon.
const MX_ICON: u8 = 95;
const NEW_BASE: u16 = 0x7000;
const CURRENT_TRACK: u16 = 501;
/// Clip record bytes 60..88 of a newer record (spec: flp/layouts.md "Playlist clip records": zeros and a double 1.0
/// at 64..71).
const CLIP_TAIL: [u8; 28] = {
    let mut t = [0u8; 28];
    let one = 1.0f64.to_le_bytes();
    let mut i = 0;
    while i < 8 {
        t[4 + i] = one[i];
        i += 1;
    }
    t
};

/// The oldest FL version (major number) whose files are converted.
pub const OLDEST: u32 = 20;

/// The major number of the FL version that saved the file (event 199, `major.minor.patch.build`), if it has one.
fn saved_by_major(events: &[Event]) -> Option<u32> {
    let e = events.iter().find(|e| e.id == id::VERSION)?;
    let text = String::from_utf8_lossy(e.data()?).into_owned();
    text.trim_matches(char::from(0)).trim().split('.').next()?.parse().ok()
}

/// Converts a project of the older layout (no channel block ends with event 51) saved by FL [`OLDEST`] or later to the
/// newer one. True when it did.
pub fn upgrade(file: &mut FlpFile) -> bool {
    let Some(events) = file.chunks.iter_mut().find_map(|c| match &mut c.body {
        ChunkBody::Events(e) => Some(e),
        _ => None,
    }) else {
        return false;
    };
    if events.iter().any(|e| e.id == id::CH_END) || !events.iter().any(|e| e.id == id::CH_START) {
        return false;
    }
    if saved_by_major(events).is_none_or(|v| v < OLDEST) {
        return false;
    }
    let tracks = events.iter().filter(|e| e.id == id::MX_FLAGS).count() as u16;
    let has_count = events.iter().any(|e| e.id == id::MX_COUNT);
    let place = |p: u16| -> u16 {
        if !(OLD_BASE..0x4000).contains(&p) {
            return p;
        }
        let (track, slot) = ((p - OLD_BASE) / 64, (p - OLD_BASE) % 64);
        let track = if tracks > 1 && track + 1 == tracks { CURRENT_TRACK } else { track };
        NEW_BASE + track * 64 + slot
    };
    // A send level's id: 64 + destination in kind 0x1F, the destination in kind 0x20 (records); 0x1F00 + 64 + d,
    // 0x2000 + d (link and control-point ids).
    let link_id = |v: u16| if (0x1F40..0x1FBF).contains(&v) { 0x2000 + (v - 0x1F40) } else { v };

    let mut out: Vec<Event> = Vec::with_capacity(events.len() + 64);
    let mut in_channel = false;
    let mut counted = has_count;
    let mut track = 0usize;
    for e in std::mem::take(events) {
        let ends_block = matches!(e.id, id::CH_START | id::PAT_START | id::ARR_START | id::ARR_CLIPS | id::PL_TRACK | id::MX_FLAGS);
        if in_channel && ends_block {
            out.push(Event::new(id::CH_END, Value::U8(0)));
            in_channel = false;
        }
        if e.id == id::CH_START {
            in_channel = true;
        }
        if e.id == id::MX_FLAGS {
            track += 1;
            // A newer track block opens with its colour switch (event 42; 1 = own colour), the older one with its colour
            // (149) when it has one, then its icon (95) and name: the switch goes in front of that run. The track
            // count (103) goes in front of the first block.
            let at = out.iter().rposition(|x| !matches!(x.id, id::MX_COLOR | MX_ICON | id::MX_NAME)).map_or(0, |k| k + 1);
            let has_switch = at > 0 && out[at - 1].id == id::MX_COLOR_FLAG;
            let at = if has_switch { at - 1 } else { at };
            if !has_switch {
                let own = out[at..].iter().any(|x| x.id == id::MX_COLOR);
                out.insert(at, Event::new(id::MX_COLOR_FLAG, Value::U8(u8::from(own))));
            }
            if !counted {
                out.insert(at, Event::new(id::MX_COUNT, Value::U16(tracks)));
                counted = true;
            }
        }
        out.push(match e.id {
            22 if in_channel => Event::new(id::CH_TARGET, Value::U16(e.int_value().unwrap_or(0) as u8 as i8 as i16 as u16)),
            id::ARR_CLIPS => with_data(e, |d| clips(d)),
            // An older master lists a route to itself; newer files do not.
            id::MX_ROUTES if track == 1 => with_data(e, |d| {
                let mut d = d.to_vec();
                if let Some(b) = d.first_mut() {
                    *b = 0;
                }
                d
            }),
            id::PL_TRACK => with_data(e, |d| {
                let mut d = d.to_vec();
                if d.len() < 70 {
                    d.resize(70, 0);
                }
                d
            }),
            id::MX_PARAMS | 216 => with_data(e, |d| {
                let mut d = d.to_vec();
                for r in d.chunks_exact_mut(12) {
                    let p = u16::from_le_bytes([r[6], r[7]]);
                    if r[5] == 0x1F && (64..=190).contains(&r[4]) && (OLD_BASE..0x4000).contains(&p) {
                        (r[4], r[5]) = (r[4] - 64, 0x20);
                    }
                    r[6..8].copy_from_slice(&place(p).to_le_bytes());
                }
                d
            }),
            id::AUTO_LINK => with_data(e, |d| {
                let mut d = d.to_vec();
                if d.len() >= 12 {
                    let src = u16::from_le_bytes([d[2], d[3]]);
                    d[2..4].copy_from_slice(&place(src).to_le_bytes());
                    let pid = u16::from_le_bytes([d[8], d[9]]);
                    d[8..10].copy_from_slice(&link_id(pid).to_le_bytes());
                    let target = u16::from_le_bytes([d[10], d[11]]);
                    d[10..12].copy_from_slice(&place(target).to_le_bytes());
                }
                d
            }),
            id::PAT_CONTROLS => with_data(e, |d| {
                let mut d = d.to_vec();
                for r in d.chunks_exact_mut(12) {
                    let (setting, target) = (u16::from_le_bytes([r[4], r[5]]), u16::from_le_bytes([r[6], r[7]]));
                    if (OLD_BASE..0x4000).contains(&target) {
                        r[4..6].copy_from_slice(&link_id(setting).to_le_bytes());
                    }
                    r[6..8].copy_from_slice(&place(target).to_le_bytes());
                }
                d
            }),
            id::CH_SAMPLE_SETTINGS => with_data(e, |d| {
                let mut d = d.to_vec();
                if d.len() >= 100 {
                    let t = i32::from_le_bytes(d[96..100].try_into().unwrap());
                    if t != 0 {
                        d[96..100].copy_from_slice(&(t as f32).to_bits().to_le_bytes());
                    }
                }
                d
            }),
            _ => e,
        });
    }
    if in_channel {
        out.push(Event::new(id::CH_END, Value::U8(0)));
    }
    *events = out;
    true
}

fn with_data(e: Event, f: impl FnOnce(&[u8]) -> Vec<u8>) -> Event {
    match e.data() {
        Some(d) => Event::new(e.id, Value::var(f(d))),
        None => e,
    }
}

/// A clip list of 32- or 60-byte records as 88-byte ones; other lengths are left alone.
fn clips(d: &[u8]) -> Vec<u8> {
    let fits = |n: usize| !d.is_empty() && d.len() % n == 0 && d.chunks_exact(n).all(|r| r[4..6] == [0x00, 0x50]);
    let Some(n) = [60usize, 32].into_iter().find(|&n| fits(n)) else { return d.to_vec() };
    let mut out = Vec::with_capacity(d.len() / n * 88);
    for r in d.chunks_exact(n) {
        let mut rec = r.to_vec();
        if n == 32 {
            rec.resize(60, 0);
            // The clip's own gain (bytes 52..55) is 1.0 on audio clips (a channel's clip); 0 silences one.
            if u16::from_le_bytes([r[6], r[7]]) < 0x5000 {
                rec[52..56].copy_from_slice(&1.0f32.to_le_bytes());
            }
        }
        rec.extend_from_slice(&CLIP_TAIL);
        out.extend(rec);
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::container::FlpFile;
    use crate::project::load;

    /// Checked against FL's own conversions: `OPENSTUDIO_LEGACY_PAIRS` names a file of `original<TAB>FL 26 resave`
    /// lines (project files are never committed).
    /// Each original is converted and read; what it gives is compared with the resave read as it is.
    #[test]
    fn converted_originals_read_like_their_resaves() {
        let Some(list) = std::env::var_os("OPENSTUDIO_LEGACY_PAIRS") else { return };
        let read = |p: &str, up: bool| -> Result<crate::project::Project, String> {
            let mut f = FlpFile::parse(&std::fs::read(p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            if up {
                super::upgrade(&mut f);
            }
            load(&f).map_err(|e| e.to_string())
        };
        let (mut ok, mut failed, mut differ) = (0, 0, 0);
        for line in std::fs::read_to_string(list).unwrap().lines() {
            let Some((orig, res)) = line.split_once('\t') else { continue };
            let name = orig.rsplit('/').next().unwrap_or(orig);
            let (a, b) = match (read(orig, true), read(res, false)) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) => {
                    failed += 1;
                    println!("FAIL  {name}: {e}");
                    continue;
                }
                (_, Err(_)) => continue,
            };
            let mut d = Vec::new();
            let notes = |p: &crate::project::Project| p.patterns.iter().map(|p| (p.number, p.notes.len())).filter(|x| x.1 > 0).collect::<Vec<_>>();
            let clips = |p: &crate::project::Project| {
                let mut v: Vec<_> = p.arrangement().map(|a| a.clips.iter().map(|c| (c.position, c.track, c.source, c.muted)).collect()).unwrap_or_default();
                v.sort_by_key(|c| (c.0, c.1));
                v
            };
            let mixer = |p: &crate::project::Project| p.mixer.iter().take(100).map(|m| (m.volume, m.pan, m.routes.clone(), m.enabled, m.slots.iter().filter(|s| s.is_some()).count())).collect::<Vec<_>>();
            let chans = |p: &crate::project::Project| p.channels.iter().map(|c| (c.name.clone(), c.kind, c.mixer_track)).collect::<Vec<_>>();
            if std::env::var("OPENSTUDIO_LEGACY_SHOW").is_ok_and(|n| name.contains(&n)) {
                let first = |x: Vec<String>, y: Vec<String>| x.iter().zip(&y).find(|(p, q)| p != q).map(|(p, q)| format!("{p}  <>  {q}")).unwrap_or(format!("len {} vs {}", x.len(), y.len()));
                println!("  ch: {}", first(chans(&a).iter().map(|c| format!("{c:?}")).collect(), chans(&b).iter().map(|c| format!("{c:?}")).collect()));
                println!("  mx: {}", first(mixer(&a).iter().map(|c| format!("{c:?}")).collect(), mixer(&b).iter().map(|c| format!("{c:?}")).collect()));
                println!("  cl: {}", first(clips(&a).iter().map(|c| format!("{c:?}")).collect(), clips(&b).iter().map(|c| format!("{c:?}")).collect()));
            }
            if chans(&a) != chans(&b) { d.push("channels"); }
            if notes(&a) != notes(&b) { d.push("notes"); }
            if clips(&a) != clips(&b) { d.push("clips"); }
            if mixer(&a) != mixer(&b) { d.push("mixer"); }
            if a.automations.len() != b.automations.len() { d.push("automations"); }
            if d.is_empty() { ok += 1 } else { differ += 1; println!("DIFF  {} {name}: {d:?}", a.saved_by.trim()) }
        }
        println!("converted and equal {ok}, different {differ}, not read {failed}");
    }

    /// `OPENSTUDIO_LEGACY_ONE=<file>` prints what a converted older project reads as.
    #[test]
    fn summary_of_one() {
        let Some(path) = std::env::var_os("OPENSTUDIO_LEGACY_ONE") else { return };
        let mut f = FlpFile::parse(&std::fs::read(path).unwrap()).unwrap();
        println!("converted: {}", super::upgrade(&mut f));
        let p = load(&f).unwrap();
        println!("saved by {}, tempo {:?}, channels {}, patterns with notes {}, clips {}, mixer tracks {}, slot plugins {}, automations {}",
            p.saved_by.trim(), p.tempo, p.channels.len(), p.patterns.iter().filter(|p| !p.notes.is_empty()).count(),
            p.arrangement().map_or(0, |a| a.clips.len()), p.mixer.len(),
            p.mixer.iter().map(|m| m.slots.iter().filter(|s| s.is_some()).count()).sum::<usize>(), p.automations.len());
        for c in &p.channels { println!("  ch {:?} {:?} -> track {}", c.name, c.kind, c.mixer_track); }
        for m in p.mixer.iter().filter(|m| m.slots.iter().any(|s| s.is_some()) || !m.routes.is_empty()).take(12) {
            println!("  mx {} {:?} routes {:?} slots {:?}", m.index, m.name, m.routes, m.slots.iter().flatten().map(|s| s.name.clone()).collect::<Vec<_>>());
        }
    }
}
