// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Standard MIDI Files: reading types 0 and 1 and writing type 1, after the public MIDI 1.0 file format.
//!
//! A file is its header (format, ticks per quarter note) and its tracks; a track is its events at absolute ticks, in
//! file order. Reading takes running status, skips system-exclusive messages, unknown meta events and unknown
//! chunks, and keeps what a truncated track held before it ends. Writing gives every event its status byte and ends
//! each track with its end-of-track event. What Lemon Studio makes of the events: spec ui/midi-files.md.

use std::fmt;

/// A whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Smf {
    /// 0 (one track) or 1 (several tracks played together).
    pub format: u16,
    /// Ticks per quarter note.
    pub ppq: u16,
    pub tracks: Vec<Track>,
}

/// A track: its events at absolute ticks, in file order (ticks never decrease).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Track {
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub tick: u32,
    pub kind: Kind,
}

/// The events Lemon Studio reads; channels are 0..=15.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    NoteOff { channel: u8, key: u8, velocity: u8 },
    /// A note-on as stored: velocity 0 is left to the reader to take as a note-off or not.
    NoteOn { channel: u8, key: u8, velocity: u8 },
    KeyPressure { channel: u8, key: u8, value: u8 },
    Controller { channel: u8, number: u8, value: u8 },
    Program { channel: u8, program: u8 },
    ChannelPressure { channel: u8, value: u8 },
    /// -8192..=8191, 0 = centre.
    PitchBend { channel: u8, value: i16 },
    /// Microseconds per quarter note.
    Tempo(u32),
    /// The denominator as a note value (4 = quarter notes), not as the stored power of two.
    TimeSignature { numerator: u8, denominator: u8 },
    TrackName(String),
    Marker(String),
}

impl Kind {
    /// The channel of a channel message.
    pub fn channel(&self) -> Option<u8> {
        match *self {
            Kind::NoteOff { channel, .. }
            | Kind::NoteOn { channel, .. }
            | Kind::KeyPressure { channel, .. }
            | Kind::Controller { channel, .. }
            | Kind::Program { channel, .. }
            | Kind::ChannelPressure { channel, .. }
            | Kind::PitchBend { channel, .. } => Some(channel),
            _ => None,
        }
    }
}

/// Why a file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file doesn't start with a MIDI file header.
    NotMidi,
    /// A type 2 file (independent sequences).
    Format(u16),
    /// Time in SMPTE frames instead of ticks per quarter note.
    Smpte,
    /// The header is cut short.
    Truncated,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotMidi => write!(f, "not a MIDI file"),
            Error::Format(n) => write!(f, "MIDI file type {n} is not supported (only types 0 and 1)"),
            Error::Smpte => write!(f, "MIDI files timed in SMPTE frames are not supported"),
            Error::Truncated => write!(f, "the MIDI file is cut short"),
        }
    }
}

impl std::error::Error for Error {}

/// Microseconds per quarter note of `bpm`.
pub fn tempo_of_bpm(bpm: f64) -> u32 {
    (60_000_000.0 / bpm.max(1.0)).round().clamp(1.0, 16_777_215.0) as u32
}

/// Beats per minute of a tempo event's microseconds per quarter note.
pub fn bpm_of_tempo(us: u32) -> f64 {
    60_000_000.0 / f64::from(us.max(1))
}

/// Reads a file.
pub fn read(b: &[u8]) -> Result<Smf, Error> {
    if b.len() < 8 || &b[0..4] != b"MThd" {
        return Err(Error::NotMidi);
    }
    let len = be32(&b[4..8]) as usize;
    if len < 6 || b.len() < 8 + 6 {
        return Err(Error::Truncated);
    }
    let format = be16(&b[8..10]);
    let ntracks = be16(&b[10..12]);
    let division = be16(&b[12..14]);
    if format > 1 {
        return Err(Error::Format(format));
    }
    if division & 0x8000 != 0 {
        return Err(Error::Smpte);
    }
    let mut at = 8usize.saturating_add(len).min(b.len());
    let mut tracks = Vec::new();
    while at + 8 <= b.len() && tracks.len() < usize::from(ntracks) {
        let id = &b[at..at + 4];
        let size = be32(&b[at + 4..at + 8]) as usize;
        let start = at + 8;
        let end = start.saturating_add(size).min(b.len());
        if id == b"MTrk" {
            tracks.push(read_track(&b[start..end]));
        }
        at = end;
    }
    Ok(Smf { format, ppq: division.max(1), tracks })
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Text of a meta event: UTF-8 when it is, else one character per byte (Latin-1).
fn text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| char::from(c)).collect(),
    }
}

/// A track chunk's events; a cut-short event ends the track.
fn read_track(b: &[u8]) -> Track {
    let mut r = Reader { b, at: 0 };
    let mut tick = 0u32;
    let mut running: Option<u8> = None;
    let mut events = Vec::new();
    while r.at < b.len() {
        let Some(delta) = r.vlq() else { break };
        tick = tick.saturating_add(delta);
        let Some(first) = r.byte() else { break };
        let status = if first & 0x80 != 0 {
            first
        } else {
            // Running status: the byte read is the first data byte.
            match running {
                Some(s) => {
                    r.at -= 1;
                    s
                }
                None => continue,
            }
        };
        match status {
            0xFF => {
                let (Some(ty), Some(n)) = (r.byte(), r.vlq()) else { break };
                let Some(data) = r.take(n as usize) else { break };
                let kind = match ty {
                    0x2F => break,
                    0x51 if data.len() >= 3 => Some(Kind::Tempo(u32::from_be_bytes([0, data[0], data[1], data[2]]))),
                    0x58 if data.len() >= 2 => {
                        Some(Kind::TimeSignature { numerator: data[0], denominator: 1u8.checked_shl(u32::from(data[1])).unwrap_or(0) })
                    }
                    0x03 => Some(Kind::TrackName(text(data))),
                    0x06 => Some(Kind::Marker(text(data))),
                    _ => None,
                };
                events.extend(kind.map(|kind| Event { tick, kind }));
            }
            0xF0 | 0xF7 => {
                let Some(n) = r.vlq() else { break };
                if r.take(n as usize).is_none() {
                    break;
                }
            }
            // System common and real-time bytes don't belong in a file; skip their data and carry on.
            0xF1..=0xFE => {
                let n = match status {
                    0xF2 => 2,
                    0xF1 | 0xF3 => 1,
                    _ => 0,
                };
                if r.take(n).is_none() {
                    break;
                }
            }
            _ => {
                running = Some(status);
                let ch = status & 0x0F;
                let n = if matches!(status & 0xF0, 0xC0 | 0xD0) { 1 } else { 2 };
                let Some(d) = r.take(n) else { break };
                let (a, c) = (d[0] & 0x7F, d.get(1).map_or(0, |&v| v & 0x7F));
                let kind = match status & 0xF0 {
                    0x80 => Kind::NoteOff { channel: ch, key: a, velocity: c },
                    0x90 => Kind::NoteOn { channel: ch, key: a, velocity: c },
                    0xA0 => Kind::KeyPressure { channel: ch, key: a, value: c },
                    0xB0 => Kind::Controller { channel: ch, number: a, value: c },
                    0xC0 => Kind::Program { channel: ch, program: a },
                    0xD0 => Kind::ChannelPressure { channel: ch, value: a },
                    _ => Kind::PitchBend { channel: ch, value: ((i16::from(c) << 7) | i16::from(a)) - 8192 },
                };
                events.push(Event { tick, kind });
            }
        }
    }
    Track { events }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let v = *self.b.get(self.at)?;
        self.at += 1;
        Some(v)
    }

    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.b.len())?;
        let s = &self.b[self.at..end];
        self.at = end;
        Some(s)
    }

    /// A variable-length quantity (at most four bytes).
    fn vlq(&mut self) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..4 {
            let c = self.byte()?;
            v = (v << 7) | u32::from(c & 0x7F);
            if c & 0x80 == 0 {
                return Some(v);
            }
        }
        Some(v)
    }
}

/// Writes `smf` (its `format` as given; tracks in order, each track's events sorted by tick, the order of events at
/// one tick kept).
pub fn write(smf: &Smf) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"MThd");
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&smf.format.to_be_bytes());
    out.extend_from_slice(&(smf.tracks.len() as u16).to_be_bytes());
    out.extend_from_slice(&smf.ppq.max(1).to_be_bytes());
    for t in &smf.tracks {
        let body = write_track(t);
        out.extend_from_slice(b"MTrk");
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
    }
    out
}

fn write_track(t: &Track) -> Vec<u8> {
    let mut ev: Vec<&Event> = t.events.iter().collect();
    ev.sort_by_key(|e| e.tick);
    let mut b = Vec::new();
    let mut last = 0u32;
    for e in ev {
        vlq(&mut b, e.tick - last);
        last = e.tick;
        let meta = |b: &mut Vec<u8>, ty: u8, data: &[u8]| {
            b.extend_from_slice(&[0xFF, ty]);
            vlq(b, data.len() as u32);
            b.extend_from_slice(data);
        };
        match &e.kind {
            Kind::NoteOff { channel, key, velocity } => b.extend_from_slice(&[0x80 | channel & 15, key & 127, velocity & 127]),
            Kind::NoteOn { channel, key, velocity } => b.extend_from_slice(&[0x90 | channel & 15, key & 127, velocity & 127]),
            Kind::KeyPressure { channel, key, value } => b.extend_from_slice(&[0xA0 | channel & 15, key & 127, value & 127]),
            Kind::Controller { channel, number, value } => b.extend_from_slice(&[0xB0 | channel & 15, number & 127, value & 127]),
            Kind::Program { channel, program } => b.extend_from_slice(&[0xC0 | channel & 15, program & 127]),
            Kind::ChannelPressure { channel, value } => b.extend_from_slice(&[0xD0 | channel & 15, value & 127]),
            Kind::PitchBend { channel, value } => {
                let v = (i32::from(*value).clamp(-8192, 8191) + 8192) as u16;
                b.extend_from_slice(&[0xE0 | channel & 15, (v & 127) as u8, (v >> 7) as u8]);
            }
            Kind::Tempo(us) => meta(&mut b, 0x51, &(*us).min(0xFF_FFFF).to_be_bytes()[1..]),
            Kind::TimeSignature { numerator, denominator } => {
                let power = (*denominator).max(1).ilog2() as u8;
                // 24 MIDI clocks per metronome click and 8 32nd notes per quarter, as files usually have them.
                meta(&mut b, 0x58, &[*numerator, power, 24, 8]);
            }
            Kind::TrackName(s) => meta(&mut b, 0x03, s.as_bytes()),
            Kind::Marker(s) => meta(&mut b, 0x06, s.as_bytes()),
        }
    }
    b.extend_from_slice(&[0, 0xFF, 0x2F, 0]);
    b
}

fn vlq(b: &mut Vec<u8>, v: u32) {
    let v = v.min(0x0FFF_FFFF);
    let mut groups = [0u8; 4];
    let mut n = 0;
    let mut x = v;
    loop {
        groups[n] = (x & 0x7F) as u8;
        n += 1;
        x >>= 7;
        if x == 0 {
            break;
        }
    }
    for i in (0..n).rev() {
        b.push(groups[i] | if i > 0 { 0x80 } else { 0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(tick: u32, kind: Kind) -> Event {
        Event { tick, kind }
    }

    #[test]
    fn written_files_read_back() {
        let smf = Smf {
            format: 1,
            ppq: 480,
            tracks: vec![
                Track {
                    events: vec![
                        ev(0, Kind::Tempo(500_000)),
                        ev(0, Kind::TimeSignature { numerator: 6, denominator: 8 }),
                        ev(960, Kind::Marker("Verse".into())),
                    ],
                },
                Track {
                    events: vec![
                        ev(0, Kind::TrackName("Piano".into())),
                        ev(0, Kind::NoteOn { channel: 9, key: 36, velocity: 100 }),
                        ev(0, Kind::Controller { channel: 9, number: 7, value: 127 }),
                        ev(240, Kind::NoteOff { channel: 9, key: 36, velocity: 64 }),
                        ev(300, Kind::PitchBend { channel: 9, value: -8192 }),
                        ev(301, Kind::PitchBend { channel: 9, value: 8191 }),
                        ev(200_000, Kind::Program { channel: 0, program: 5 }),
                    ],
                },
            ],
        };
        assert_eq!(read(&write(&smf)), Ok(smf));
    }

    #[test]
    fn running_status_and_skipped_events() {
        // Header: type 0, one track, 96 ticks per quarter note.
        let mut b = b"MThd\0\0\0\x06\0\0\0\x01\0\x60".to_vec();
        let body = [
            0x00, 0x90, 60, 100, // note-on
            0x10, 62, 90, // running status: another note-on
            0x00, 0xF0, 0x03, 1, 2, 0xF7, // system exclusive, skipped
            0x00, 0xFF, 0x01, 0x02, b'h', b'i', // text, skipped
            0x20, 60, 0, // running status again: velocity 0
            0x81, 0x00, 0x80, 62, 30, // delta 128, note-off
            0x00, 0xFF, 0x2F, 0x00,
        ];
        b.extend_from_slice(b"MTrk");
        b.extend_from_slice(&(body.len() as u32).to_be_bytes());
        b.extend_from_slice(&body);
        let f = read(&b).unwrap();
        assert_eq!((f.format, f.ppq, f.tracks.len()), (0, 96, 1));
        let k: Vec<(u32, Kind)> = f.tracks[0].events.iter().map(|e| (e.tick, e.kind.clone())).collect();
        assert_eq!(
            k,
            vec![
                (0, Kind::NoteOn { channel: 0, key: 60, velocity: 100 }),
                (16, Kind::NoteOn { channel: 0, key: 62, velocity: 90 }),
                (48, Kind::NoteOn { channel: 0, key: 60, velocity: 0 }),
                (176, Kind::NoteOff { channel: 0, key: 62, velocity: 30 }),
            ]
        );
    }

    #[test]
    fn refused_files() {
        assert_eq!(read(b"RIFF...."), Err(Error::NotMidi));
        assert_eq!(read(b"MThd\0\0\0\x06\0\x02\0\x01\0\x60"), Err(Error::Format(2)));
        assert_eq!(read(b"MThd\0\0\0\x06\0\x01\0\x01\xE7\x28"), Err(Error::Smpte));
        assert_eq!(read(b"MThd\0\0\0\x06\0\x01"), Err(Error::Truncated));
    }

    #[test]
    fn a_cut_track_keeps_what_came_before() {
        let mut b = b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk\0\0\0\x20".to_vec();
        b.extend_from_slice(&[0x00, 0x90, 60, 100, 0x60, 0x80, 60]);
        let f = read(&b).unwrap();
        assert_eq!(f.tracks[0].events.len(), 1);
    }

    #[test]
    fn tempo_conversions() {
        assert_eq!(tempo_of_bpm(120.0), 500_000);
        assert!((bpm_of_tempo(428_571) - 140.0).abs() < 0.001);
    }

    #[test]
    fn variable_lengths() {
        for (v, bytes) in [(0u32, vec![0x00]), (0x7F, vec![0x7F]), (0x80, vec![0x81, 0x00]), (0x0FFF_FFFF, vec![0xFF, 0xFF, 0xFF, 0x7F])] {
            let mut b = Vec::new();
            vlq(&mut b, v);
            assert_eq!(b, bytes);
            assert_eq!(Reader { b: &bytes, at: 0 }.vlq(), Some(v));
        }
    }
}
