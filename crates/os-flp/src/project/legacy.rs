// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Project files of the older generations: FL 10 to FL 24.1, and FL 24.2 betas whose channel blocks have no end event.
//! [`upgrade`] rewrites such a stream once, when the file is opened, into the current (FL 24.2 and later) layout, so
//! that [`load`] and [`apply`] see one layout and a save writes the current one, as FL 26 does with every older file
//! (spec: flp/layouts.md "Converting to the newer layout"). Nothing is written back in the older layout.
//!
// spec: flp/layouts.md, flp/engine-events.md "A channel", flp/events/channels.md "Version scope",
//       flp/events/playlist.md

use std::collections::BTreeMap;

use super::*;

/// Text events: 8-bit text in files up to FL 11, UTF-16 later (spec: flp/coverage.md, flp/engine-events.md
/// "Events"). Event 199, the version, is 8-bit in every file.
const TEXT_IDS: [u8; 19] = [192, 193, 194, 195, 196, 197, 198, 200, 201, 202, 203, 204, 205, 206, 207, 230, 231, 239, 241];

/// Events that end the channel section in a file without channel end events: the playlist, the global events after
/// it and the mixer (spec: flp/engine-events.md "A channel": the last block runs to the first event of the playlist
/// or the mixer).
const SECTION_END: [u8; 16] = [99, 233, 238, 236, 29, 31, 38, 39, 100, 242, 103, 149, 42, 27, 204, 95];

/// Events of a mixer track's header, before its 236: colour, colour flag, name, icon, and FL 10 / 11's event 27.
const TRACK_HEADER: [u8; 5] = [149, 42, 204, 95, 27];

/// Events that end a track's slot area (routes and the events after them, the parameter records).
const SLOTS_END: [u8; 8] = [235, 165, 166, 49, 154, 147, 225, 133];

/// Step sequencer dots of the older files: on / off, velocity, shift; the low byte is the step (spec: flp/layouts.md
/// "Older files").
const DOT_ON: u8 = 91;
const DOT_VELOCITY: u8 = 72;
const DOT_SHIFT: u8 = 92;

/// A channel's colour in the older files when it was not chosen (the 0x6A655C of FL 26's resaves with colour flag 0).
const OLD_CHANNEL_COLOR: u32 = 0x006A_655C;

/// The channel's mixer track before FL 24.2, a signed byte (spec: flp/engine-events.md "A channel").
const CH_TARGET_OLD: u8 = 22;

/// The mixer's first place before FL 24.2 and from it (spec: flp/layouts.md "Mixer places").
const OLD_BASE: u16 = 0x2000;
const PROJECT_PLACE: u16 = 0x4000;

/// Mixer sizes: 105 tracks in FL 11 and 12 files, 127 later (spec: flp/layouts.md "What differs").
const OLD_MIXER: usize = 105;
const MIXER: usize = 127;

/// Parameter ids (bytes 4..5 of a mixer record, 8..9 of a link): the send level to track `d`, 0x1F00 + 64 + d before
/// FL 24.2, 0x2000 + d from it (spec: flp/layouts.md "Parameter ids in an automation link").
const OLD_SEND: std::ops::RangeInclusive<u16> = 0x1F40..=0x1FBE;
const SEND: u16 = 0x2000;
const MIXER_PARAM: u16 = 0x1F00;
const PLUGIN_PARAM: u16 = 0x8000;

/// The version a file says it was saved by, as (major, minor); (26, 0) when it does not say.
fn saved_by(events: &[Event]) -> (u32, u32) {
    let text = events.iter().find(|e| e.id == id::VERSION).and_then(Event::data).map(|d| decode_text(d, TextEnc::Ansi)).unwrap_or_default();
    let mut parts = text.split('.').map(|p| p.trim_matches(char::from(0)).trim().parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(major), minor) => (major, minor.unwrap_or(0)),
        _ => (26, 0),
    }
}

/// True when `events` are in an older generation's layout: saved before FL 24.2, or with channel blocks that no event
/// 51 closes (spec: flp/layouts.md "How to tell the generation"). Blocks that event 51 closes are the current layout
/// whatever the version says: an earlier cutting-edge converted older projects and kept their version, and converting
/// such a file again would read its single stretch lengths as integers. Up to FL 11 the version still decides, as the
/// text's encoding goes by it.
pub fn is_legacy(events: &[Event]) -> bool {
    let version = saved_by(events);
    if events.iter().any(|e| e.id == id::CH_END) {
        return version.0 <= 11;
    }
    version < (24, 2) || events.iter().any(|e| e.id == id::CH_START)
}

/// Rewrites a file of an older generation into the current layout; `Ok(false)` leaves a current file as it is. What
/// changes is `flp/layouts.md`'s list: text to UTF-16 (files up to FL 11), the version, channel blocks closed by event 51
/// with the mixer track in event 104, step sequencer dots as notes, an arrangement around the playlist, clip and track
/// records at their current sizes, mixer tracks with colour flags, 12-byte records and slot markers, mixer places from
/// 0x7000 with the last track as track 501, send levels and link ids in the current form, and a 105-track mixer grown to
/// 127 tracks.
pub fn upgrade(file: &mut FlpFile) -> Result<bool, ProjectError> {
    let ppq = file.header().and_then(Header::ppq).unwrap_or(96);
    let events = events_of_mut(file)?;
    if !is_legacy(events) {
        return Ok(false);
    }
    let version = saved_by(events);
    let mut ev = std::mem::take(events);
    if version.0 <= 11 {
        text_to_utf16(&mut ev);
    }
    set_version(&mut ev);
    let dots = channel_blocks(&mut ev, version);
    add_dot_notes(&mut ev, dots, ppq);
    if version < (21, 0) {
        three_osc(&mut ev);
    }
    playlist(&mut ev, version);
    mixer(&mut ev, version);
    let events = events_of_mut(file)?;
    *events = ev;
    scan(events)?;
    Ok(true)
}

fn text_to_utf16(ev: &mut [Event]) {
    for e in ev.iter_mut().filter(|e| TEXT_IDS.contains(&e.id)) {
        if let Some(d) = e.data() {
            let text = decode_text(d, TextEnc::Ansi);
            set_data(e, encode_text(&text, TextEnc::Utf16));
        }
    }
}

/// The version and the events FL 26 writes after it (159, the build; 169, which FL needs to read more than one clip:
/// spec: flp/events/project-info.md).
fn set_version(ev: &mut Vec<Event>) {
    let v = Event::new(id::VERSION, Value::var(format!("{COMPAT_VERSION}\0").into_bytes()));
    let at = match ev.iter().position(|e| e.id == id::VERSION) {
        Some(i) => {
            ev[i] = v;
            i
        }
        None => {
            ev.insert(0, v);
            0
        }
    };
    let mut after = at + 1;
    for (eid, value) in [(159, COMPAT_BUILD), (169, 15)] {
        match ev.iter().position(|e| e.id == eid) {
            Some(i) => {
                set_int(&mut ev[i], value);
                after = after.max(i + 1);
            }
            None => {
                ev.insert(after, Event::new(eid, Value::U32(value)));
                after += 1;
            }
        }
    }
}

/// A step sequencer dot: (pattern, channel, step) → (on, velocity, shift).
type Dots = BTreeMap<(u16, u16, u8), (bool, Option<u8>, Option<u8>)>;

/// Closes every channel block with event 51 (spec: flp/events/channels.md "Version scope"). A block runs from its event
/// 64 to the next; the pattern events the older files keep inside it, from its first event 65 on, follow the block
/// instead, as pattern blocks of the current layout. The block's mixer track moves from event 22 to event 104, and
/// before FL 24.2 the sample's stretch length (event 215, int 24) from an integer to a single. The step sequencer dots
/// are taken out and returned.
fn channel_blocks(ev: &mut Vec<Event>, version: (u32, u32)) -> Dots {
    let mut dots = Dots::new();
    let Some(first) = ev.iter().position(|e| e.id == id::CH_START) else { return dots };
    let end = (first + 1..ev.len()).find(|&k| SECTION_END.contains(&ev[k].id)).unwrap_or(ev.len());
    let section: Vec<Event> = ev.drain(first..end).collect();
    let mut out = Vec::with_capacity(section.len() + 64);
    let starts: Vec<usize> = section.iter().enumerate().filter(|(_, e)| e.id == id::CH_START).map(|(k, _)| k).collect();
    for (n, &s) in starts.iter().enumerate() {
        let e = starts.get(n + 1).copied().unwrap_or(section.len());
        let block = &section[s..e];
        let channel = int(&block[0]) as u16;
        let split = block.iter().position(|x| x.id == id::PAT_START || x.id == id::CH_END).unwrap_or(block.len());
        let has_target = block[..split].iter().any(|x| x.id == id::CH_TARGET);
        let has_flag = block[..split].iter().any(|x| x.id == id::CH_COLOR_FLAG);
        for x in &block[..split] {
            match x.id {
                // The colour flag (41) after the colour: set, except on the older files' default colour, which becomes
                // the current default (as FL 26's resaves of the older files have it).
                id::COLOR if !has_flag => {
                    let old_default = int(x) == OLD_CHANNEL_COLOR;
                    out.push(if old_default { Event::new(id::COLOR, Value::U32(CHANNEL_COLOR_DEFAULT)) } else { x.clone() });
                    out.push(Event::new(id::CH_COLOR_FLAG, Value::U8(u8::from(!old_default))));
                }
                CH_TARGET_OLD if has_target => {}
                CH_TARGET_OLD => out.push(Event::new(id::CH_TARGET, Value::U16(i16::from(int(x) as u8 as i8).max(0) as u16))),
                id::CH_SAMPLE_SETTINGS if version < (24, 2) => {
                    let mut x = x.clone();
                    if let Some(d) = x.data().filter(|d| d.len() >= 100) {
                        let mut d = d.to_vec();
                        let v = i32::from_le_bytes(d[96..100].try_into().expect("four bytes"));
                        d[96..100].copy_from_slice(&(v as f32).to_le_bytes());
                        set_data(&mut x, d);
                    }
                    out.push(x);
                }
                _ => out.push(x.clone()),
            }
        }
        out.push(Event::new(id::CH_END, Value::U8(0)));
        // The pattern events: dots out, a selection that nothing follows dropped.
        let mut pattern = 1u16;
        let mut rest: Vec<Event> = Vec::new();
        for x in block[split..].iter().filter(|x| x.id != id::CH_END) {
            match x.id {
                id::PAT_START => pattern = int(x) as u16,
                DOT_ON | DOT_VELOCITY | DOT_SHIFT => {
                    let v = int(x);
                    let d = dots.entry((pattern, channel, v as u8)).or_insert((false, None, None));
                    let high = (v >> 8) as u8;
                    match x.id {
                        DOT_ON => d.0 = high != 0,
                        DOT_VELOCITY => d.1 = Some(high),
                        _ => d.2 = Some(high),
                    }
                    continue;
                }
                _ => {}
            }
            if x.id == id::PAT_START && rest.last().is_some_and(|l| l.id == id::PAT_START) {
                rest.pop();
            }
            rest.push(x.clone());
        }
        if rest.last().is_some_and(|l| l.id == id::PAT_START) {
            rest.pop();
        }
        out.extend(rest);
    }
    // Events before the first channel's 64 cannot occur (the section starts there); keep the order otherwise.
    ev.splice(first..first, out);
    dots
}

/// A dot as the note FL 26 writes for it: length 0 on key 60 at `step * PPQ / 4 + trunc(shift * PPQ / 192)` with the
/// dot's velocity (100 without one), flags 0x4000, fine pitch 120, pan 64, mod X and mod Y 128, byte 18 at 64 (spec:
/// flp/layouts.md "Older files").
fn dot_note(channel: u16, step: u8, velocity: Option<u8>, shift: Option<u8>, ppq: u16) -> [u8; NOTE_LEN] {
    let ppq = u32::from(ppq);
    let position = u32::from(step) * ppq / 4 + u32::from(shift.unwrap_or(0)) * ppq / 192;
    let mut r = [0u8; NOTE_LEN];
    r[0..4].copy_from_slice(&position.to_le_bytes());
    r[4..6].copy_from_slice(&NOTE_FLAGS_DEFAULT.to_le_bytes());
    r[6..8].copy_from_slice(&channel.to_le_bytes());
    r[12..14].copy_from_slice(&60u16.to_le_bytes());
    r[16] = 120;
    r[18] = 64;
    r[20] = 64;
    r[21] = velocity.unwrap_or(100);
    r[22] = 128;
    r[23] = 128;
    r
}

/// The dots that are on, as notes in their pattern's note list (event 224 behind the pattern's 65 before the first
/// channel), the notes of one tick in descending channel order.
fn add_dot_notes(ev: &mut Vec<Event>, dots: Dots, ppq: u16) {
    let mut by_pattern: BTreeMap<u16, Vec<[u8; NOTE_LEN]>> = BTreeMap::new();
    for (&(pattern, channel, step), &(on, velocity, shift)) in dots.iter().rev() {
        if on {
            by_pattern.entry(pattern).or_default().push(dot_note(channel, step, velocity, shift, ppq));
        }
    }
    for (pattern, mut notes) in by_pattern {
        notes.sort_by_key(|r| u32::from_le_bytes(r[0..4].try_into().expect("four bytes")));
        let prelude = ev.iter().position(|e| e.id == id::CH_START).unwrap_or(ev.len());
        let found = (0..prelude).find(|&i| ev[i].id == id::PAT_START && int(&ev[i]) as u16 == pattern);
        let at = found.map(|i| if ev.get(i + 1).is_some_and(|e| e.id == id::PAT_CONTROLS) { i + 2 } else { i + 1 });
        match at {
            Some(k) if ev.get(k).is_some_and(|e| e.id == id::PAT_NOTES) => {
                let mut all: Vec<[u8; NOTE_LEN]> = ev[k].data().unwrap_or(&[]).chunks_exact(NOTE_LEN).map(|c| c.try_into().expect("a note")).collect();
                all.extend(notes);
                all.sort_by_key(|r| u32::from_le_bytes(r[0..4].try_into().expect("four bytes")));
                set_data(&mut ev[k], all.concat());
            }
            Some(k) => ev.insert(k, Event::new(id::PAT_NOTES, Value::var(notes.concat()))),
            None => {
                // Before the first note block of a later pattern, else behind the last one, else before the links and
                // the channels.
                let later = (0..prelude).find(|&i| ev[i].id == id::PAT_START && int(&ev[i]) as u16 > pattern);
                let behind = (0..prelude).rev().find(|&i| matches!(ev[i].id, id::PAT_START | id::PAT_CONTROLS | id::PAT_NOTES | id::LOOP_CHANNEL | id::LOOP_STEPS));
                let k = later
                    .or(behind.map(|i| i + 1))
                    .unwrap_or_else(|| (0..prelude).find(|&i| matches!(ev[i].id, 226 | id::AUTO_LINK)).unwrap_or(prelude));
                ev.splice(k..k, [Event::new(id::PAT_START, Value::U16(pattern)), Event::new(id::PAT_NOTES, Value::var(notes.concat()))]);
            }
        }
    }
}

/// 3x Osc's channel settings that became its own parameters in FL 21: filter cutoff (2), resonance (3), the pitch
/// envelope's amount (0x1408) and one more envelope field (0x120B) (spec: flp/layouts.md "Older files").
const THREE_OSC: [(u16, u16); 4] = [(2, PLUGIN_PARAM + 110), (3, PLUGIN_PARAM + 111), (0x1408, PLUGIN_PARAM + 99), (0x120B, PLUGIN_PARAM + 68)];

/// Automation links and control points on those settings of a 3x Osc channel, rewritten to its parameters as FL 26
/// does when it opens a file older than FL 21. Runs after [`channel_blocks`] (each block ends with 51) and on UTF-16
/// names.
fn three_osc(ev: &mut [Event]) {
    let mut osc = Vec::new();
    let mut channel = None;
    for e in ev.iter() {
        match e.id {
            id::CH_START => channel = Some(int(e) as u16),
            id::CH_END => channel = None,
            id::PLUGIN_NAME => {
                if let (Some(c), Some(d)) = (channel, e.data()) {
                    if decode_text(d, TextEnc::Utf16) == "3x Osc" {
                        osc.push(c);
                    }
                }
            }
            _ => {}
        }
    }
    if osc.is_empty() {
        return;
    }
    let map = |target: u16, pid: u16| -> u16 {
        if osc.contains(&target) { THREE_OSC.iter().find(|m| m.0 == pid).map_or(pid, |m| m.1) } else { pid }
    };
    for e in ev.iter_mut() {
        let Some(d) = e.data() else { continue };
        let mut d = d.to_vec();
        match e.id {
            id::AUTO_LINK if d.len() >= 12 => {
                let pid = map(u16::from_le_bytes([d[10], d[11]]), u16::from_le_bytes([d[8], d[9]]));
                d[8..10].copy_from_slice(&pid.to_le_bytes());
            }
            id::PAT_CONTROLS => {
                let whole = d.len() - d.len() % CONTROL_LEN;
                for r in d[..whole].chunks_exact_mut(CONTROL_LEN) {
                    let pid = map(u16::from_le_bytes([r[6], r[7]]), u16::from_le_bytes([r[4], r[5]]));
                    r[4..6].copy_from_slice(&pid.to_le_bytes());
                }
            }
            _ => continue,
        }
        set_data(e, d);
    }
}

/// A playlist track's colour in the older files when it was not chosen (the 0x565148 of FL 26's resaves with colour
/// flag 0); the current default is 0x2D2C29 (spec: flp/events/playlist.md "Event 238 layout").
const OLD_TRACK_COLOR: u32 = 0x0056_5148;

/// Events that end an arrangement: the globals after the playlist and the mixer.
const ARRANGEMENT_END: [u8; 14] = [100, 242, 29, 31, 38, 39, 40, 103, 149, 42, 27, 236, 204, 95];

/// The playlist: an arrangement block (99, 241, 36, 233) around it where the file has none, clip records at 88 bytes,
/// track records at 70 bytes and 500 of them, and the current arrangement (100) (spec: flp/events/playlist.md,
/// flp/layouts.md "Playlist clip records").
fn playlist(ev: &mut Vec<Event>, version: (u32, u32)) {
    let channels_end = ev.iter().rposition(|e| e.id == id::CH_END).map_or(0, |i| i + 1);
    if !ev.iter().any(|e| e.id == id::ARR_START) {
        let at = (channels_end..ev.len())
            .find(|&k| matches!(ev[k].id, id::ARR_CLIPS | id::PL_TRACK) || ARRANGEMENT_END.contains(&ev[k].id))
            .unwrap_or(ev.len());
        let mut head = vec![
            Event::new(id::ARR_START, Value::U16(0)),
            Event::new(id::ARR_NAME, Value::var(encode_text("Arrangement", TextEnc::Utf16))),
            Event::new(id::ARR_UNKNOWN, Value::U8(0)),
        ];
        if ev.get(at).is_none_or(|e| e.id != id::ARR_CLIPS) {
            head.push(Event::new(id::ARR_CLIPS, Value::var(Vec::new())));
        }
        ev.splice(at..at, head);
    }
    // Track counted from the bottom: 198 - track up to FL 12.5, 499 - track later.
    let old_tracks = version < (12, 9);
    for e in ev.iter_mut().filter(|e| e.id == id::ARR_CLIPS) {
        let Some(d) = e.data() else { continue };
        let Some(len) = list_clip_len(d) else { continue };
        if len == CLIP_LEN && !old_tracks {
            continue;
        }
        let mut out = Vec::with_capacity(d.len() / len * CLIP_LEN);
        for (n, r) in d.chunks_exact(len).enumerate() {
            let mut c = Clip::TEMPLATE;
            c[..len].copy_from_slice(r);
            if len < 36 {
                c[32..36].copy_from_slice(&(n as u32 + 1).to_le_bytes());
            }
            // The clip's own gain is 1.0 on channel clips (0 would silence an audio clip).
            if len < 56 && u16::from_le_bytes([r[6], r[7]]) < 0x5000 {
                c[52..56].copy_from_slice(&1.0f32.to_le_bytes());
            }
            if len < CLIP_LEN {
                c[76..80].fill(0xff);
            }
            if old_tracks {
                let track = 198u16.saturating_sub(u16::from_le_bytes([r[12], r[13]]));
                c[12..14].copy_from_slice(&499u16.saturating_sub(track).to_le_bytes());
            }
            out.extend_from_slice(&c);
        }
        set_data(e, out);
    }
    // Track records: the shorter ones are the first bytes of the current one; the rest as a new track has it.
    for e in ev.iter_mut().filter(|e| e.id == id::PL_TRACK) {
        let Some(d) = e.data().filter(|d| d.len() >= 4 && d.len() < 70) else { continue };
        let number = u32::from_le_bytes(d[0..4].try_into().expect("four bytes"));
        let mut r = d.to_vec();
        r.extend_from_slice(&track_record(&PlaylistTrack::new(number))[d.len()..]);
        set_data(e, r);
    }
    // The colour flag (43) after each track record: set, except on the older files' default colour, which becomes the
    // current default (as FL 26's resaves of the older files have it).
    let mut k = 0;
    while k < ev.len() {
        if ev[k].id == id::PL_TRACK && ev.get(k + 1).is_none_or(|e| e.id != id::PL_COLOR_FLAG) {
            let old_default = ev[k].data().is_some_and(|d| d.len() >= 8 && d[4..8] == OLD_TRACK_COLOR.to_le_bytes());
            if old_default {
                patch_bytes(&mut ev[k], 4, &PATTERN_COLOR_DEFAULT.to_le_bytes());
            }
            ev.insert(k + 1, Event::new(id::PL_COLOR_FLAG, Value::U8(u8::from(!old_default))));
        }
        k += 1;
    }
    // Each arrangement gets its tracks up to 500.
    let starts: Vec<usize> = ev.iter().enumerate().filter(|(_, e)| e.id == id::ARR_START).map(|(i, _)| i).collect();
    for &s in starts.iter().rev() {
        let end = (s + 1..ev.len()).find(|&k| ev[k].id == id::ARR_START || ARRANGEMENT_END.contains(&ev[k].id)).unwrap_or(ev.len());
        let last = (s..end).rev().find(|&k| ev[k].id == id::PL_TRACK);
        let have = last.and_then(|k| ev[k].data()).map_or(0, |d| u32::from_le_bytes(d[0..4].try_into().expect("four bytes")));
        let mut at = last.map_or(end, |k| k + 1);
        while ev.get(at).is_some_and(|e| matches!(e.id, id::PL_COLOR_FLAG | id::PL_NAME)) && at < end {
            at += 1;
        }
        let more: Vec<Event> = (have + 1..=PLAYLIST_TRACKS)
            .flat_map(|n| [Event::new(id::PL_TRACK, Value::var(track_record(&PlaylistTrack::new(n)))), Event::new(id::PL_COLOR_FLAG, Value::U8(0))])
            .collect();
        ev.splice(at..at, more);
    }
    if !ev.iter().any(|e| e.id == id::CURRENT_ARRANGEMENT) {
        let last = ev.iter().rposition(|e| e.id == id::ARR_START).unwrap_or(0);
        let at = (last + 1..ev.len()).find(|&k| ARRANGEMENT_END.contains(&ev[k].id)).unwrap_or(ev.len());
        ev.insert(at, Event::new(id::CURRENT_ARRANGEMENT, Value::U16(0)));
    }
}

/// Where a track's header starts: its 236 and the header events right before it.
fn header_start(ev: &[Event], flags: usize) -> usize {
    let mut k = flags;
    while k > 0 && TRACK_HEADER.contains(&ev[k - 1].id) {
        k -= 1;
    }
    k
}

/// The mixer in the current form (spec: flp/layouts.md "Mixer places", "A mixer track's record", "Older files";
/// flp/events/mixer.md).
fn mixer(ev: &mut Vec<Event>, version: (u32, u32)) {
    let flags: Vec<usize> = ev.iter().enumerate().filter(|(_, e)| e.id == id::MX_FLAGS).map(|(i, _)| i).collect();
    let count = flags.len();
    if count == 0 {
        return;
    }
    let last = (count - 1) as u16;
    let old_latency = version < (12, 8);
    let params = ev.iter().position(|e| e.id == id::MX_PARAMS);
    // Older files address the mixer from 0x2000 (the master's records sit there); FL 24.2's betas already from 0x7000.
    let old_base = params.and_then(|i| ev[i].data()).is_some_and(|d| {
        let places: Vec<u16> = mixer_param_records(d).map(|r| r.3).collect();
        places.iter().any(|p| (OLD_BASE..PROJECT_PLACE).contains(p)) && !places.iter().any(|&p| p >= 0x7000)
    });
    let grow = count == OLD_MIXER;
    // A track's new number: the last stored track is the "current" one, track 501 in the records, 126 as a send
    // destination of a mixer grown to 127 tracks.
    let dest = |d: u16| if grow && d == last { (MIXER - 1) as u16 } else { d };
    let place = |p: u16| -> u16 {
        if !old_base || !(OLD_BASE..PROJECT_PLACE).contains(&p) {
            return p;
        }
        let (t, s) = ((p - OLD_BASE) / 64, (p - OLD_BASE) % 64);
        mixer_location(if t == last { MX_SPECIAL_TRACK } else { t }, s)
    };
    let param_id = |v: u16| if OLD_SEND.contains(&v) { SEND + dest(v - OLD_SEND.start()) } else { v };

    // Files up to FL 11: the routes to the four send tracks (100..103) are at 0 without a level record.
    if version.0 <= 11 {
        if let Some(pi) = params {
            let mut data = ev[pi].data().unwrap_or(&[]).to_vec();
            for (t, &f) in flags.iter().enumerate() {
                let next = flags.get(t + 1).copied().unwrap_or(ev.len());
                let Some(routes) = (f..next).find(|&k| ev[k].id == id::MX_ROUTES).and_then(|k| ev[k].data()) else { continue };
                let loc = OLD_BASE + t as u16 * 64;
                for d in 100..=103usize {
                    let on = routes.get(d).is_some_and(|&b| b != 0);
                    let pid = 64 + d as u8;
                    if on && !mixer_param_records(&data).any(|r| r.1 == pid && r.2 == MX_KIND_PARAM && r.3 == loc) {
                        set_record(&mut data, MX_KIND_PARAM, loc, pid, 0);
                    }
                }
            }
            set_data(&mut ev[pi], data);
        }
    }

    // Records: places and send ids. Event 225 and 216 hold 12-byte records, 223 its control points.
    for e in ev.iter_mut() {
        match e.id {
            id::MX_PARAMS | id::INIT_CONTROLS | id::PAT_CONTROLS => {
                let Some(d) = e.data() else { continue };
                let mut d = d.to_vec();
                let whole = d.len() - d.len() % MIXER_REC_LEN;
                for r in d[..whole].chunks_exact_mut(MIXER_REC_LEN) {
                    let pid = param_id(u16::from_le_bytes([r[4], r[5]]));
                    r[4..6].copy_from_slice(&pid.to_le_bytes());
                    let p = place(u16::from_le_bytes([r[6], r[7]]));
                    r[6..8].copy_from_slice(&p.to_le_bytes());
                }
                set_data(e, d);
            }
            id::AUTO_LINK => {
                let Some(d) = e.data().filter(|d| d.len() >= 12) else { continue };
                let mut d = d.to_vec();
                let target = u16::from_le_bytes([d[10], d[11]]);
                let mut pid = u16::from_le_bytes([d[8], d[9]]);
                // A plugin parameter up to FL 11: 0x2000 + n on a rack channel, n in a mixer slot.
                if version.0 <= 11 {
                    if target < OLD_BASE && (0x2000..PLUGIN_PARAM).contains(&pid) {
                        pid = PLUGIN_PARAM + (pid - 0x2000);
                    } else if (OLD_BASE..PROJECT_PLACE).contains(&target) && pid < MIXER_PARAM {
                        pid = PLUGIN_PARAM + pid;
                    }
                }
                d[8..10].copy_from_slice(&param_id(pid).to_le_bytes());
                for at in [2, 10] {
                    let p = place(u16::from_le_bytes([d[at], d[at + 1]]));
                    d[at..at + 2].copy_from_slice(&p.to_le_bytes());
                }
                set_data(e, d);
            }
            _ => {}
        }
    }

    // Within a place, the send levels after the other records and in destination order, as a writer puts them.
    if let Some(pi) = params {
        let d = ev[pi].data().unwrap_or(&[]).to_vec();
        let whole = d.len() - d.len() % MIXER_REC_LEN;
        let mut recs: Vec<&[u8]> = d[..whole].chunks_exact(MIXER_REC_LEN).collect();
        recs.sort_by_key(|r| {
            let send = r[5] == MX_KIND_ROUTE;
            (u16::from_le_bytes([r[6], r[7]]), send, if send { r[4] } else { 0 })
        });
        let mut out: Vec<u8> = recs.concat();
        out.extend_from_slice(&d[whole..]);
        set_data(&mut ev[pi], out);
    }

    // Tracks, last first so earlier indices hold: header flag, record, slot markers, slot plugin records, routes. A
    // track's events run to the next track's header.
    let heads: Vec<usize> = flags.iter().map(|&f| header_start(ev, f)).collect();
    // A slot marker holds the slot's number; one FL 20.8 file has 0x106 and 0x107 for slots 6 and 7.
    for e in ev[flags[0]..].iter_mut().filter(|e| e.id == id::MX_SLOT && int(e) > 0xFF) {
        let slot = int(e) & 0xFF;
        set_int(e, slot);
    }
    let any_routes = ev[flags[0]..].iter().any(|e| e.id == id::MX_ROUTES);
    for t in (0..count).rev() {
        let f = flags[t];
        let mut next = heads.get(t + 1).copied().unwrap_or_else(|| params.unwrap_or(ev.len()));
        let master_like = t == 0 || t + 1 == count;
        // The record at 12 bytes (spec: flp/layouts.md "A mixer track's record (event 236)").
        if let Some(d) = ev[f].data().filter(|d| d.len() < 12) {
            let (out, inp) = track_latency(d, old_latency);
            let mut r = [0u8; 12];
            let fl = if d.len() >= 8 {
                u32::from_le_bytes(d[4..8].try_into().expect("four bytes")) & !u32::from(MX_FLAG_LATE_OUT)
            } else {
                u32::from(if master_like { MX_FLAGS_MASTER } else { MX_FLAGS_INSERT })
            };
            r[..4].copy_from_slice(&out.to_le_bytes());
            r[4..8].copy_from_slice(&fl.to_le_bytes());
            r[8..].copy_from_slice(&inp.to_le_bytes());
            set_data(&mut ev[f], r.to_vec());
        }
        let area_end = (f + 1..next).find(|&k| SLOTS_END.contains(&ev[k].id)).unwrap_or(next);
        // Every track has its routes (235) where any has: a track without them (the master of some FL 24.2 betas) gets
        // an empty one, which routes nowhere.
        if any_routes && !ev[f..next].iter().any(|e| e.id == id::MX_ROUTES) {
            ev.insert(area_end, Event::new(id::MX_ROUTES, Value::var(route_bytes(&[]))));
            next += 1;
        }
        // Routes to the old last track go to its new place.
        if grow {
            if let Some(k) = (area_end..next).find(|&k| ev[k].id == id::MX_ROUTES) {
                let mut d = ev[k].data().unwrap_or(&[]).to_vec();
                if d.get(usize::from(last)).is_some_and(|&b| b != 0) {
                    d[usize::from(last)] = 0;
                    d.resize(d.len().max(MIXER), 0);
                    d[MIXER - 1] = 1;
                    set_data(&mut ev[k], d);
                }
            }
        }
        // Slot plugins of the last track name it as track 501.
        if old_base && t + 1 == count && t > 0 {
            for e in ev[f + 1..area_end].iter_mut().filter(|e| e.id == id::PLUGIN_REC) {
                patch_bytes(e, 0, &u32::from(MX_SPECIAL_TRACK).to_le_bytes());
            }
        }
        // Slot markers (98) after each slot, filled or not; files up to FL 12.4 have none.
        if !ev[f + 1..area_end].iter().any(|e| e.id == id::MX_SLOT) {
            let area: Vec<Event> = ev.drain(f + 1..area_end).collect();
            let first = area.iter().position(|e| e.id == id::PLUGIN_NAME).unwrap_or(area.len());
            let mut out: Vec<Event> = area[..first].to_vec();
            let mut runs: [Vec<Event>; 10] = Default::default();
            let mut k = first;
            let mut next_slot = 0usize;
            while k < area.len() {
                let end = (k + 1..area.len()).find(|&j| area[j].id == id::PLUGIN_NAME).unwrap_or(area.len());
                let run = &area[k..end];
                let slot = run
                    .iter()
                    .find(|e| e.id == id::PLUGIN_REC)
                    .and_then(Event::data)
                    .filter(|d| d.len() >= 8)
                    .map_or(next_slot, |d| u32::from_le_bytes(d[4..8].try_into().expect("four bytes")) as usize);
                match runs.get_mut(slot).filter(|r| r.is_empty()) {
                    Some(r) => r.extend_from_slice(run),
                    None => out.extend_from_slice(run),
                }
                next_slot = slot + 1;
                k = end;
            }
            for (s, run) in runs.into_iter().enumerate() {
                out.extend(run);
                out.push(Event::new(id::MX_SLOT, Value::U16(s as u16)));
            }
            ev.splice(f + 1..f + 1, out);
        }
        // The colour flag (42) after the colour, before the name.
        let h = header_start(ev, f);
        if !ev[h..f].iter().any(|e| e.id == id::MX_COLOR_FLAG) {
            let color = ev[h..f].iter().position(|e| e.id == id::MX_COLOR);
            let at = color.map_or(h, |c| h + c + 1);
            ev.insert(at, Event::new(id::MX_COLOR_FLAG, Value::U8(u8::from(color.is_some()))));
        }
    }

    // A 105-track mixer grows to 127: tracks 104..125 are new, before the old last track.
    let mut total = count;
    if grow {
        let flags: Vec<usize> = ev.iter().enumerate().filter(|(_, e)| e.id == id::MX_FLAGS).map(|(i, _)| i).collect();
        let at = header_start(ev, flags[count - 1]);
        let blocks: Vec<Event> = (last..(MIXER - 1) as u16).flat_map(|t| track_block_events(&MixerTrack::new(t), false, TextEnc::Utf16)).collect();
        ev.splice(at..at, blocks);
        if let Some(pi) = ev.iter().position(|e| e.id == id::MX_PARAMS) {
            let mut data = ev[pi].data().unwrap_or(&[]).to_vec();
            let pos = mixer_param_records(&data)
                .find(|r| r.3 >= mixer_location(last, 0) && r.3 != PROJECT_PLACE)
                .map_or(data.len() - data.len() % MIXER_REC_LEN, |r| r.0);
            let recs: Vec<u8> = (last..(MIXER - 1) as u16).flat_map(|t| track_records(t, None)).collect();
            data.splice(pos..pos, recs);
            set_data(&mut ev[pi], data);
        }
        total = MIXER;
    }
    // The track count (103), before the first track.
    match ev.iter().position(|e| e.id == id::MX_COUNT) {
        Some(i) => set_int(&mut ev[i], total as u32),
        None => {
            let first = ev.iter().position(|e| e.id == id::MX_FLAGS).expect("a track");
            let at = header_start(ev, first);
            ev.insert(at, Event::new(id::MX_COUNT, Value::U16(total as u16)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic streams in the older layouts, built event by event (no project files).

    fn ansi(s: &str) -> Value {
        Value::var(format!("{s}\0").into_bytes())
    }

    fn ids(ev: &[Event]) -> Vec<u8> {
        ev.iter().map(|e| e.id).collect()
    }

    /// A clip record of the FL 20 form (32 bytes): position, the 0x5000 base, the item, length 96, the track.
    fn clip_32(position: u32, item: u16, track: u16) -> Vec<u8> {
        let mut r = vec![0u8; 32];
        r[0..4].copy_from_slice(&position.to_le_bytes());
        r[4..6].copy_from_slice(&0x5000u16.to_le_bytes());
        r[6..8].copy_from_slice(&item.to_le_bytes());
        r[8..12].copy_from_slice(&96u32.to_le_bytes());
        r[12..14].copy_from_slice(&track.to_le_bytes());
        r
    }

    /// A 12-byte mixer record: `pid` (its kind in the high byte) at `place`.
    fn rec(pid: u16, place: u16, v: i32) -> Vec<u8> {
        let mut r = vec![0u8; 12];
        r[4..6].copy_from_slice(&pid.to_le_bytes());
        r[6..8].copy_from_slice(&place.to_le_bytes());
        r[8..12].copy_from_slice(&v.to_le_bytes());
        r
    }

    #[test]
    fn current_files_are_left_alone() {
        let mut file = create(&Project { ppq: 96, ..Default::default() }).unwrap();
        assert!(!is_legacy(&file.events().cloned().collect::<Vec<_>>()));
        let before = file.to_bytes().unwrap();
        assert!(!upgrade(&mut file).unwrap());
        assert_eq!(file.to_bytes().unwrap(), before);
    }

    #[test]
    fn the_generation_shows_in_the_version_or_the_missing_end_events() {
        let old = [Event::new(id::VERSION, ansi("12.3.0.71"))];
        assert!(is_legacy(&old));
        let beta = [Event::new(id::VERSION, ansi("24.2.99.4123")), Event::new(id::CH_START, Value::U16(0))];
        assert!(is_legacy(&beta));
        let current = [Event::new(id::VERSION, ansi("24.2.99.4123")), Event::new(id::CH_START, Value::U16(0)), Event::new(id::CH_END, Value::U8(0))];
        assert!(!is_legacy(&current));
        assert_eq!(saved_by(&[]), (26, 0));
    }

    /// Channel blocks get their end event and the mixer track in event 104; the pattern events inside a block follow it,
    /// and its dots come out (here step 4 of pattern 1, velocity 80, on).
    #[test]
    fn channel_blocks_are_closed() {
        let mut ev = vec![
            Event::new(id::VERSION, ansi("12.3")),
            Event::new(id::CH_START, Value::U16(0)),
            Event::new(CH_TARGET_OLD, Value::U8(3)),
            Event::new(id::COLOR, Value::U32(OLD_CHANNEL_COLOR)),
            Event::new(id::PAT_START, Value::U16(1)),
            Event::new(DOT_ON, Value::U16(0x0104)),
            Event::new(DOT_VELOCITY, Value::U16((80 << 8) | 4)),
            Event::new(id::CH_START, Value::U16(1)),
            Event::new(CH_TARGET_OLD, Value::U8(0xFF)),
            Event::new(id::COLOR, Value::U32(0x0011_2233)),
            Event::new(id::ARR_CLIPS, Value::var(Vec::new())),
        ];
        let dots = channel_blocks(&mut ev, (12, 3));
        assert_eq!(ids(&ev), [199, 64, 104, 128, 41, 51, 64, 104, 128, 41, 51, 233]);
        // The mixer track as an unsigned word (-1, no track, as 0); the old default colour is the current default,
        // unflagged, a chosen one is kept and flagged.
        assert_eq!((int(&ev[2]), int(&ev[7])), (3, 0));
        assert_eq!((int(&ev[3]), int(&ev[4])), (CHANNEL_COLOR_DEFAULT, 0));
        assert_eq!((int(&ev[8]), int(&ev[9])), (0x0011_2233, 1));
        assert_eq!(dots.into_iter().collect::<Vec<_>>(), [((1, 0, 4), (true, Some(80), None))]);
    }

    /// An on dot becomes a note of length 0 on key 60 at its step, in a pattern block before the channels.
    #[test]
    fn dots_become_notes() {
        let mut ev = vec![Event::new(id::VERSION, ansi("12.3")), Event::new(id::CH_START, Value::U16(0)), Event::new(id::CH_END, Value::U8(0))];
        let mut dots = Dots::new();
        dots.insert((2, 0, 4), (true, Some(80), Some(96)));
        dots.insert((2, 0, 5), (false, Some(80), None));
        add_dot_notes(&mut ev, dots, 96);
        assert_eq!(ids(&ev), [199, 65, 224, 64, 51]);
        assert_eq!(int(&ev[1]), 2);
        let n = ev[2].data().unwrap();
        assert_eq!(n.len(), NOTE_LEN);
        // Step 4 (a sixteenth each) and a shift of 96 / 192 of a beat.
        assert_eq!(u32::from_le_bytes(n[0..4].try_into().unwrap()), 4 * 96 / 4 + 48);
        assert_eq!((u16::from_le_bytes([n[12], n[13]]), n[21]), (60, 80));
    }

    /// FL 20 clip records grow to 88 bytes with the track counted the current way (from 499, not 198), an arrangement
    /// around them, track records up to 500 and the current arrangement.
    #[test]
    fn playlist_grows_to_the_current_layout() {
        let mut ev = vec![
            Event::new(id::CH_START, Value::U16(0)),
            Event::new(id::CH_END, Value::U8(0)),
            Event::new(id::ARR_CLIPS, Value::var([clip_32(0, 0x5001, 198), clip_32(96, 0x5002, 196)].concat())),
        ];
        playlist(&mut ev, (12, 3));
        assert_eq!(ids(&ev)[..6], [64, 51, 99, 241, 36, 233]);
        let clips = ev[5].data().unwrap();
        assert_eq!(clips.len(), 2 * CLIP_LEN);
        let track = |n: usize| u16::from_le_bytes([clips[n * CLIP_LEN + 12], clips[n * CLIP_LEN + 13]]);
        assert_eq!((track(0), track(1)), (499, 497));
        // The clip's id where the record had none, and the bytes the current record ends with.
        assert_eq!(u32::from_le_bytes(clips[CLIP_LEN + 32..CLIP_LEN + 36].try_into().unwrap()), 2);
        assert_eq!(clips[76..80], [0xff; 4]);
        assert_eq!(ev.iter().filter(|e| e.id == id::PL_TRACK).count(), PLAYLIST_TRACKS as usize);
        assert_eq!(ev.last().map(|e| e.id), Some(id::CURRENT_ARRANGEMENT));
    }

    /// A three-track mixer of an FL 11 file: 5-byte records become 12 bytes, colour flags and slot markers are added,
    /// places move from 0x2000 to 0x7000 with the last track as 501, and the send level takes the current form.
    #[test]
    fn mixer_moves_to_the_current_places() {
        let track = |n: u8| -> Vec<Event> {
            vec![
                Event::new(id::MX_FLAGS, Value::var(vec![0, 0, 0, 0, 0])),
                Event::new(id::MX_ROUTES, Value::var(if n == 1 { vec![0, 0, 1] } else { vec![0, 0, 0] })),
            ]
        };
        let mut ev: Vec<Event> = (0..3).flat_map(track).collect();
        // Track 1's volume, its send level to track 2 (kind 0x1F, id 64 + 2) and the last track's volume.
        let params = [rec(0x1F00, 0x2040, 12800), rec(0x1F00 + 64 + 2, 0x2040, 6400), rec(0x1F00, 0x2080, 12800)].concat();
        ev.push(Event::new(id::MX_PARAMS, Value::var(params)));
        mixer(&mut ev, (11, 0));
        // 103 first; each track: 42, 236, ten slot markers, 235.
        assert_eq!((ev[0].id, int(&ev[0])), (id::MX_COUNT, 3));
        let tracks: Vec<usize> = ev.iter().enumerate().filter(|(_, e)| e.id == id::MX_FLAGS).map(|(i, _)| i).collect();
        assert_eq!(tracks.len(), 3);
        for &f in &tracks {
            assert_eq!(ev[f - 1].id, id::MX_COLOR_FLAG);
            assert_eq!(ev[f].data().unwrap().len(), 12);
            assert!(ev[f + 1..f + 11].iter().all(|e| e.id == id::MX_SLOT));
            assert_eq!(ev[f + 1..f + 11].iter().map(int).collect::<Vec<u32>>(), (0..10).collect::<Vec<u32>>());
            assert_eq!(ev[f + 11].id, id::MX_ROUTES);
        }
        let d = ev.iter().find(|e| e.id == id::MX_PARAMS).unwrap().data().unwrap();
        let recs: Vec<(u8, u8, u16, i32)> = mixer_param_records(d).map(|r| (r.1, r.2, r.3, r.4)).collect();
        assert_eq!(
            recs,
            [
                (0, MX_KIND_PARAM, mixer_location(1, 0), 12800),
                (2, MX_KIND_ROUTE, mixer_location(1, 0), 6400),
                (0, MX_KIND_PARAM, mixer_location(MX_SPECIAL_TRACK, 0), 12800),
            ]
        );
    }

    /// A file already in the current layout (its channel blocks closed) with an older version is not converted again.
    #[test]
    fn a_closed_layout_is_current_whatever_its_version() {
        let ev = |id: u8, v: Value| Event::new(id, v);
        let version = |t: &str| ev(id::VERSION, Value::var(format!("{t}\0").into_bytes()));
        let closed = [version("20.7.1.1773"), ev(id::CH_START, Value::U16(0)), ev(id::CH_END, Value::U8(0))];
        assert!(!is_legacy(&closed));
        assert!(is_legacy(&[version("11.1.1"), ev(id::CH_START, Value::U16(0)), ev(id::CH_END, Value::U8(0))]), "8-bit text goes by the version");
        let open = [version("26.1.7.5653"), ev(id::CH_START, Value::U16(0))];
        assert!(is_legacy(&open), "blocks not closed");
        assert!(is_legacy(&[version("24.1.1.4285")]), "an older version with no channels");
        assert!(!is_legacy(&[version("26.1.7.5653")]));
    }

    /// The whole conversion of a small FL 11 file: it loads, and an unedited save keeps the converted stream.
    #[test]
    fn an_old_file_opens_and_saves_converted() {
        let mut ev = vec![
            Event::new(id::VERSION, ansi("11.0.3")),
            Event::new(id::TITLE, Value::var(b"Caf\xe9\0".to_vec())),
            Event::new(id::CH_START, Value::U16(0)),
            Event::new(CH_TARGET_OLD, Value::U8(1)),
            Event::new(id::PAT_START, Value::U16(1)),
            Event::new(DOT_ON, Value::U16(0x0100)),
            Event::new(id::ARR_CLIPS, Value::var(clip_32(0, 0x5001, 198))),
        ];
        for n in 0..3u8 {
            ev.push(Event::new(id::MX_FLAGS, Value::var(vec![0, 0, 0, 0, 0])));
            ev.push(Event::new(id::MX_ROUTES, Value::var(vec![u8::from(n == 1), 0, 0])));
        }
        ev.push(Event::new(id::MX_PARAMS, Value::var(rec(0x1F00, 0x2040, 6400))));
        let header = Header { raw: [0u16, 1, 96].iter().flat_map(|v| v.to_le_bytes()).collect() };
        let mut file = FlpFile {
            chunks: vec![Chunk { tag: Tag::HEADER, body: ChunkBody::Header(header) }, Chunk { tag: Tag::DATA, body: ChunkBody::Events(ev) }],
            trailing: Vec::new(),
        };
        let original = file.clone();
        assert!(upgrade(&mut file).unwrap());
        assert!(!is_legacy(&file.events().cloned().collect::<Vec<_>>()));
        let p = load(&file).unwrap();
        assert_eq!(p.info.title.as_deref(), Some("Caf\u{e9}"));
        assert_eq!((p.channels.len(), p.channels[0].mixer_track), (1, 1));
        assert_eq!(p.patterns.iter().find(|p| p.number == 1).map(|p| p.notes.len()), Some(1));
        assert_eq!(p.arrangements[0].clips.len(), 1);
        assert_eq!(p.mixer.len(), 3);
        // Loading the unconverted file converts a copy the same way.
        assert_eq!(load(&original).unwrap(), p);
        let bytes = file.to_bytes().unwrap();
        apply(&p, &mut file).unwrap();
        assert_eq!(file.to_bytes().unwrap(), bytes);
        assert!(!upgrade(&mut file).unwrap());
    }
}
