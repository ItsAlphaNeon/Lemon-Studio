// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The `.lemon` project file: an FLP container with Lemon Studio's header chunk (`LMhd`) before the FLP's chunks and
//! its own event chunk (`LMex`) after them. The FLP chunks keep every event's FLP meaning, so [`FlpFile::parse`] and
//! the project layer read both kinds of file; these functions turn a file into one kind or the other.

// spec: flp/lemon-format.md

use crate::{Chunk, ChunkBody, Event, EventReader, FlpFile, ReadError, Tag, Value};

/// Lemon Studio's header chunk, first in a `.lemon` file.
pub const HEADER: Tag = Tag(*b"LMhd");
/// Lemon Studio's events, after the FLP chunks.
pub const EXTRA: Tag = Tag(*b"LMex");
/// The format version written.
pub const VERSION: u16 = 1;
/// `LMex` event: the program and version that saved the file (UTF-8 text).
pub const SAVED_BY: u8 = 192;
/// `LMex` event: the icons chosen with Change icon... (channels, mixer tracks, effect slots). The payloads of these
/// events are Lemon Studio's (os-app `lemon_state`, spec: flp/lemon-format.md "LMex").
pub const ICONS: u8 = 193;
/// `LMex` event: the names given to effect slots.
pub const SLOT_NAMES: u8 = 194;
/// `LMex` event: the effect slots' colours.
pub const SLOT_COLORS: u8 = 195;
/// `LMex` event: Performance mode's settings (the playlist ▸ menu's checks).
pub const PERFORMANCE: u8 = 196;
/// `LMex` event: the mixer's Plugin delay compensation ▸ Automatic and View ▸ checks.
pub const MIXER: u8 = 197;
/// `LMex` event: the plugin windows' Scaling ▸ (channels and effect slots).
pub const SCALING: u8 = 198;
/// `LMex` event: each channel's Show waveform preview and MIDI channel through checks.
pub const CHANNEL_CHECKS: u8 = 199;
/// `LMex` event: Set keyboard focus of all plugins to ▸.
pub const KEY_FOCUS: u8 = 200;
/// `LMex` event: the playlist tracks locked to their clips.
pub const TRACK_LOCKS: u8 = 201;

/// Whether `file` is a `.lemon` file (its first chunk is `LMhd`).
pub fn is_lemon(file: &FlpFile) -> bool {
    file.chunks.first().is_some_and(|c| c.tag == HEADER)
}

/// The `.lemon` format version, `None` for an FLP.
pub fn version(file: &FlpFile) -> Option<u16> {
    let c = file.chunks.first().filter(|c| c.tag == HEADER)?;
    match &c.body {
        ChunkBody::Raw(raw) => Some(u16::from_le_bytes(raw.get(..2)?.try_into().ok()?)),
        _ => None,
    }
}

/// The `LMex` events, in file order (none for an FLP or a file without them).
pub fn events(file: &FlpFile) -> Result<Vec<Event>, ReadError> {
    let mut out = Vec::new();
    for c in file.chunks.iter().filter(|c| c.tag == EXTRA) {
        if let ChunkBody::Raw(raw) = &c.body {
            // Lemon events follow the ID ranges only: the FLP's size exceptions are the original's.
            for e in EventReader::new(raw, 0).ranges_only() {
                out.push(e?.1);
            }
        }
    }
    Ok(out)
}

/// The payload of the last `LMex` event with `id`, if the file has one.
pub fn data(file: &FlpFile, id: u8) -> Option<Vec<u8>> {
    let e = events(file).ok()?.into_iter().rev().find(|e| e.id == id)?;
    Some(e.data()?.to_vec())
}

/// Makes `file` a `.lemon` file: an `LMhd` chunk first (an existing one keeps its extra bytes, with the version
/// written), and one `LMex` chunk last holding `extra` (an event of the same ID there is replaced; the others are kept
/// in order). The FLP chunks are not touched.
pub fn to_lemon(file: &mut FlpFile, extra: &[Event]) -> Result<(), ReadError> {
    let mut kept = events(file)?;
    kept.retain(|e| !extra.iter().any(|x| x.id == e.id));
    kept.extend_from_slice(extra);
    let mut header = match file.chunks.first() {
        Some(Chunk { tag, body: ChunkBody::Raw(raw) }) if *tag == HEADER => raw.clone(),
        _ => Vec::new(),
    };
    header.resize(header.len().max(4), 0);
    header[..2].copy_from_slice(&VERSION.to_le_bytes());
    file.chunks.retain(|c| c.tag != HEADER && c.tag != EXTRA);
    file.chunks.insert(0, Chunk { tag: HEADER, body: ChunkBody::Raw(header) });
    let mut raw = Vec::new();
    for e in &kept {
        e.encode_into(&mut raw);
    }
    file.chunks.push(Chunk { tag: EXTRA, body: ChunkBody::Raw(raw) });
    Ok(())
}

/// Makes `file` a plain FLP: Lemon Studio's chunks are dropped.
pub fn to_flp(file: &mut FlpFile) {
    file.chunks.retain(|c| c.tag != HEADER && c.tag != EXTRA);
}

/// The `SAVED_BY` event for `text`.
pub fn saved_by(text: &str) -> Event {
    Event::new(SAVED_BY, Value::var(text.as_bytes()))
}

/// The text of the `SAVED_BY` event, if the file has one.
pub fn saved_by_text(file: &FlpFile) -> Option<String> {
    String::from_utf8(data(file, SAVED_BY)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flp() -> FlpFile {
        let mut b = Vec::new();
        b.extend_from_slice(b"FLhd");
        b.extend_from_slice(&6u32.to_le_bytes());
        b.extend_from_slice(&[0, 0, 0, 0, 96, 0]);
        b.extend_from_slice(b"FLdt");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&[64, 0x8c, 0x00]);
        FlpFile::parse(&b).unwrap()
    }

    /// A file becomes .lemon and back to the same FLP; the version and the saved-by text read back; unknown Lemon
    /// events are kept and a known one is replaced.
    #[test]
    fn lemon_and_back() {
        let plain = flp();
        let mut f = plain.clone();
        assert!(!is_lemon(&f) && version(&f).is_none());
        let unknown = Event::new(230, Value::var([1, 2, 3]));
        to_lemon(&mut f, &[saved_by("Lemon Studio 0.1"), unknown.clone()]).unwrap();
        let bytes = f.to_bytes().unwrap();
        assert_eq!(&bytes[..4], b"LMhd");
        let back = FlpFile::parse(&bytes).unwrap();
        assert!(is_lemon(&back));
        assert_eq!(version(&back), Some(VERSION));
        assert_eq!(saved_by_text(&back).as_deref(), Some("Lemon Studio 0.1"));
        assert_eq!(back.header().and_then(|h| h.ppq()), Some(96), "the FLP chunks read as before");
        let mut again = back.clone();
        to_lemon(&mut again, &[saved_by("Lemon Studio 0.2")]).unwrap();
        let ids: Vec<(u8, Vec<u8>)> = events(&again).unwrap().iter().map(|e| (e.id, e.data().unwrap().to_vec())).collect();
        assert_eq!(ids, vec![(230, vec![1, 2, 3]), (SAVED_BY, b"Lemon Studio 0.2".to_vec())]);
        assert_eq!(again.chunks.iter().filter(|c| c.tag == EXTRA).count(), 1);
        to_flp(&mut again);
        assert_eq!(again.to_bytes().unwrap(), plain.to_bytes().unwrap());
    }
}
