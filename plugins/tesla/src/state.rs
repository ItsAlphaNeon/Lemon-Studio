// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What Tesla saves, and Edison's state in and out.
//!
//! Tesla's own state is JSON: the sample as a plain WAV file embedded in base64 (the bytes File > Save would write,
//! with no size limit), the path it came from, the view and the recording settings. Edison's state (FLP event 213)
//! is a short head, the embedded WAVE and a tail with the path and the recording settings: Lemon Studio opens an
//! Edison slot as Tesla through [`from_edison`] and writes it back for FL through [`to_edison`].
//!
// spec: plugins/tesla/README.md "Version 1" (state), "Opening Edison slots"; plugins/edison/state.md

use serde::{Deserialize, Serialize};

use crate::sample::Sample;
use crate::wav::{self, Depth};

/// When / what to record (Edison's saved values).
pub const WHEN_NOW: u8 = 0;
pub const WHEN_ON_INPUT: u8 = 1;
pub const WHEN_INPUT: u8 = 2;
pub const WHEN_ON_PLAY: u8 = 3;

/// The "When" selector's labels, by value.
pub const WHEN_NAMES: [&str; 4] = ["Now", "On input", "Input", "On play"];

/// The maximum recording time menu (Edison's index 0 to 11).
pub const MAX_NAMES: [&str; 12] = ["Forever", "1 minute", "3 minutes", "5 minutes", "10 minutes", "30 minutes", "60 minutes", "50MB", "100MB", "200MB", "500MB", "1GB"];
/// The same, as the selector shows them.
pub const MAX_SHORT: [&str; 12] = ["\u{221e}", "1'", "3'", "5'", "10'", "30'", "60'", "50MB", "100MB", "200MB", "500MB", "1GB"];

/// The limit of maximum time index `i`: seconds, or bytes of recorded audio as 32-bit float; `None` = forever.
pub enum Limit {
    Forever,
    Seconds(f64),
    Bytes(u64),
}

pub fn limit(i: u8) -> Limit {
    const MB: u64 = 1024 * 1024;
    match i {
        1 => Limit::Seconds(60.0),
        2 => Limit::Seconds(180.0),
        3 => Limit::Seconds(300.0),
        4 => Limit::Seconds(600.0),
        5 => Limit::Seconds(1800.0),
        6 => Limit::Seconds(3600.0),
        7 => Limit::Bytes(50 * MB),
        8 => Limit::Bytes(100 * MB),
        9 => Limit::Bytes(200 * MB),
        10 => Limit::Bytes(500 * MB),
        11 => Limit::Bytes(1024 * MB),
        _ => Limit::Forever,
    }
}

/// The recording settings (Edison's five, plus Tesla's input mute).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rec {
    pub when: u8,
    /// Maximum time, Edison's menu index.
    pub max: u8,
    pub append: bool,
    pub slave: bool,
    pub armed: bool,
    pub mute: bool,
}

impl Default for Rec {
    /// FL's default Edison instance: On input, 30 minutes, Append on, not slaved, not armed.
    fn default() -> Self {
        Rec { when: WHEN_ON_INPUT, max: 5, append: true, slave: false, armed: false, mute: false }
    }
}

/// View > Time format.
pub const TIME_SAMPLES: u8 = 0;
pub const TIME_MSM: u8 = 1;
pub const TIME_BBT: u8 = 2;
pub const TIME_AUTO: u8 = 3;

/// Snap modes (Snap menu; pitch period is Edison's only, not in Tesla 1).
pub const SNAP_GRID: u8 = 0;
pub const SNAP_REGIONS: u8 = 1;
pub const SNAP_SAMPLES: u8 = 2;
pub const SNAP_ZERO: u8 = 3;

/// Spectrum band counts (View > Display settings).
pub const BANDS: [usize; 8] = [64, 128, 256, 512, 1024, 2048, 4096, 8192];

/// The view and the editing toggles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    /// The first frame shown and how many frames the width shows (0 = the whole sample).
    pub start: f64,
    pub len: f64,
    pub spectrum: bool,
    pub regions: bool,
    pub looped: bool,
    pub time: u8,
    pub follow: bool,
    pub snap: u8,
    pub snap_on: bool,
    pub clickfree: bool,
    pub freeze: bool,
    pub slide: bool,
    /// Spectrum precision: index into [`BANDS`].
    pub bands: u8,
    /// Spectrum gradient: index into `spectrum::GRADIENTS`.
    pub gradient: u8,
    /// Edit > Enable undo for large samples.
    pub undo_large: bool,
    /// The editor's size in logical pixels (resizable; 0 = the default).
    pub w: u32,
    pub h: u32,
    /// Settings: slave playback ignores the host's / Tesla's own selection.
    pub ignore_host_sel: bool,
    pub ignore_own_sel: bool,
}

impl Default for View {
    fn default() -> Self {
        View {
            start: 0.0,
            len: 0.0,
            spectrum: false,
            regions: true,
            looped: true,
            time: TIME_AUTO,
            follow: true,
            snap: SNAP_GRID,
            snap_on: true,
            clickfree: true,
            freeze: true,
            slide: false,
            bands: 4,
            gradient: 0,
            undo_large: true,
            w: 0,
            h: 0,
            ignore_host_sel: false,
            ignore_own_sel: false,
        }
    }
}

/// Everything Tesla saves besides the audio.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Saved {
    pub sample: Sample,
    /// The file the sample was loaded from ("" when none).
    pub path: String,
    pub rec: Rec,
    pub view: View,
    /// An Edison slot's envelope blocks, kept as they came when they are not the default (written back to FL).
    pub edison_env: Vec<u8>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Json {
    version: u32,
    /// The sample as a WAV file, base64.
    wav: String,
    path: String,
    rec: Rec,
    view: View,
    edison_env: String,
}

/// The persisted field holding Tesla's state.
pub const STATE_FIELD: &str = "tesla";

/// Tesla's state as JSON text (the persisted field's value).
pub fn to_json(s: &Saved) -> String {
    let j = Json {
        version: 1,
        wav: b64_encode(&wav::write(&s.sample, Depth::Own)),
        path: s.path.clone(),
        rec: s.rec.clone(),
        view: s.view.clone(),
        edison_env: b64_encode(&s.edison_env),
    };
    serde_json::to_string(&j).unwrap_or_default()
}

pub fn from_json(text: &str) -> Option<Saved> {
    let j: Json = serde_json::from_str(text).ok()?;
    let bytes = b64_decode(&j.wav)?;
    let sample = if bytes.is_empty() { Sample::default() } else { wav::read(&bytes).ok()? };
    Some(Saved { sample, path: j.path, rec: j.rec, view: j.view, edison_env: b64_decode(&j.edison_env).unwrap_or_default() })
}

/// A plugin state blob (the host's: its length, then nih-plug's JSON) holding `s`.
pub fn plugin_state(s: &Saved) -> Vec<u8> {
    let json = serde_json::json!({ "version": env!("CARGO_PKG_VERSION"), "params": {}, "fields": { STATE_FIELD: to_json(s) } }).to_string();
    let mut blob = (json.len() as u64).to_le_bytes().to_vec();
    blob.extend_from_slice(json.as_bytes());
    blob
}

/// The state inside a plugin state blob (see [`plugin_state`]); `None` when the blob is not a Tesla state.
pub fn from_plugin_state(blob: &[u8]) -> Option<Saved> {
    let json = if blob.len() > 8 && blob[0] != b'{' { &blob[8..] } else { blob };
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    from_json(v.get("fields")?.get(STATE_FIELD)?.as_str()?)
}

// ---------------------------------------------------------------------------------------------------------------
// Edison's state

/// The head version FL 2026 writes.
pub const EDISON_VERSION: u32 = 0x000D_0003;
/// The tail's size after the path.
const TAIL: usize = 376;
/// Where the envelope blocks start after the path, and their size.
const ENV_AT: usize = 36;
const ENV_LEN: usize = 4 * 80;

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn i32_at(b: &[u8], i: usize) -> Option<i32> {
    u32_at(b, i).map(|v| v as i32)
}

/// Edison's state (event 213, or an `.fst` preset's data) as Tesla's: the embedded WAVE becomes the sample with its
/// markers, regions and loop; the path, When, maximum time, Append (stored inverted), slave and armed carry over.
pub fn from_edison(b: &[u8]) -> Option<Saved> {
    let version = u32_at(b, 0)?;
    let (head, armed, size): (usize, bool, usize) = match version {
        EDISON_VERSION => (24, u32_at(b, 8)? != 0, u64::from_le_bytes(b.get(16..24)?.try_into().ok()?) as usize),
        6 => (20, u32_at(b, 8)? != 0, u32_at(b, 16)? as usize),
        4 => (16, false, u32_at(b, 12)? as usize),
        _ => return None,
    };
    let wave = b.get(head..head.checked_add(size)?)?;
    let sample = wav::read(wave).ok()?;
    let mut out = Saved { sample, ..Saved::default() };
    out.rec.armed = armed;
    // The older tails (versions 4 and 6) aren't decoded: their settings stay at the defaults.
    if version == EDISON_VERSION {
        let tail = &b[head + size..];
        if let Some(&n) = tail.first() {
            let n = n as usize;
            if let Some(p) = tail.get(1..1 + 2 * n) {
                let units: Vec<u16> = p.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                out.path = String::from_utf16_lossy(&units);
            }
            let t = 1 + 2 * n;
            if tail.len() >= t + TAIL {
                let when = i32_at(tail, t + 356)?;
                let append_off = i32_at(tail, t + 360)?;
                let max = i32_at(tail, t + 364)?;
                let slave = i32_at(tail, t + 372)?;
                out.rec.when = when.clamp(0, 3) as u8;
                out.rec.append = append_off == 0;
                out.rec.max = max.clamp(0, 11) as u8;
                out.rec.slave = slave != 0;
                let env = &tail[t + ENV_AT..t + ENV_AT + ENV_LEN];
                let len = f64::from_le_bytes(tail[t + 24..t + 32].try_into().ok()?);
                if env != default_env(len).as_slice() {
                    out.edison_env = env.to_vec();
                }
            }
        }
    }
    Some(out)
}

/// The four flat envelope blocks Edison saves while no envelope was edited, for a sample `len` long (the tail's
/// length field).
fn default_env(len: f64) -> Vec<u8> {
    let mut v = Vec::with_capacity(ENV_LEN);
    for _ in 0..4 {
        for i in [6i32, 3, 2] {
            v.extend_from_slice(&i.to_le_bytes());
        }
        for f in [0.0f64, 0.5, 0.0, len, 0.5] {
            v.extend_from_slice(&f.to_le_bytes());
        }
        v.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 2]);
        v.extend_from_slice(&0i32.to_le_bytes());
        for _ in 0..4 {
            v.extend_from_slice(&(-1i32).to_le_bytes());
        }
    }
    v
}

/// Tesla's state as Edison's (version `0x000D0003`), so FL keeps the slot as Edison with its audio, markers,
/// regions, loop and recording settings.
pub fn to_edison(s: &Saved) -> Vec<u8> {
    let wave = wav::write_as(&s.sample, Depth::Own, "Edison");
    let mut b = Vec::with_capacity(24 + wave.len() + 1 + TAIL + 512);
    for v in [EDISON_VERSION, 0, u32::from(s.rec.armed), 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&(wave.len() as u64).to_le_bytes());
    b.extend_from_slice(&wave);
    let path: Vec<u16> = s.path.encode_utf16().take(255).collect();
    b.push(path.len() as u8);
    for u in &path {
        b.extend_from_slice(&u.to_le_bytes());
    }
    let len = 2.0 * s.sample.frames().max(1) as f64 / s.sample.rate.max(1) as f64;
    for f in [-1.0f64, -1.0, 0.0, len] {
        b.extend_from_slice(&f.to_le_bytes());
    }
    b.extend_from_slice(&(if s.sample.is_empty() { 0.02f32 } else { 0.0 }).to_le_bytes());
    if s.edison_env.len() == ENV_LEN {
        b.extend_from_slice(&s.edison_env);
    } else {
        b.extend_from_slice(&default_env(len));
    }
    for v in [i32::from(s.rec.when), i32::from(!s.rec.append), i32::from(s.rec.max), 3, i32::from(s.rec.slave)] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

// ---------------------------------------------------------------------------------------------------------------
// Base64 (standard alphabet, padded)

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        s.push(B64[(n >> 18) as usize & 63] as char);
        s.push(B64[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    s
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut val = [255u8; 256];
    for (i, &c) in B64.iter().enumerate() {
        val[c as usize] = i as u8;
    }
    let t: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    if t.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(t.len() / 4 * 3);
    for q in t.chunks(4) {
        let pad = q.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 {
            return None;
        }
        let mut n = 0u32;
        for (k, &c) in q.iter().enumerate() {
            let v = if k >= 4 - pad { 0 } else { val[c as usize] };
            if v == 255 {
                return None;
            }
            n = (n << 6) | u32::from(v);
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Format;

    #[test]
    fn base64_round_trips() {
        for n in 0..10 {
            let v: Vec<u8> = (0..n).map(|i| (i * 37 + 5) as u8).collect();
            assert_eq!(b64_decode(&b64_encode(&v)).unwrap(), v);
        }
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert_eq!(b64_encode(b"Ma"), "TWE=");
        assert!(b64_decode("TWF").is_none());
        assert!(b64_decode("T!Fu").is_none());
    }

    fn saved() -> Saved {
        let mut s = Sample::from_channels(48000, vec![vec![0.1, 0.2, -0.3, 0.4], vec![0.0, -0.1, 0.2, 0.3]], Format::Float32);
        s.add_cue("mk1", 1, None);
        s.add_cue("rg1", 1, Some(2));
        s.looped = Some((0, 3));
        Saved { sample: s, path: "C:\\samples\\dc.wav".into(), rec: Rec { when: WHEN_ON_PLAY, max: 3, append: false, slave: true, armed: true, mute: true }, view: View { spectrum: true, ..View::default() }, edison_env: Vec::new() }
    }

    #[test]
    fn tesla_state_round_trips() {
        let s = saved();
        assert_eq!(from_json(&to_json(&s)).unwrap(), s);
        assert_eq!(from_plugin_state(&plugin_state(&s)).unwrap(), s);
        assert!(from_plugin_state(b"{}").is_none());
    }

    #[test]
    fn edison_state_round_trips_with_its_layout() {
        let mut s = saved();
        s.rec.mute = false;
        s.view = View::default();
        let b = to_edison(&s);
        let wave = u64::from_le_bytes(b[16..24].try_into().unwrap()) as usize;
        assert_eq!(&b[0..4], &EDISON_VERSION.to_le_bytes());
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), 1);
        // Tail: 1 + 2n + 376 bytes.
        assert_eq!(b.len(), 24 + wave + 1 + 2 * s.path.len() + 376);
        let t = &b[24 + wave..];
        let at = 1 + 2 * s.path.len();
        assert_eq!(i32::from_le_bytes(t[at + 356..at + 360].try_into().unwrap()), 3);
        assert_eq!(i32::from_le_bytes(t[at + 360..at + 364].try_into().unwrap()), 1, "Append is stored inverted");
        assert_eq!(i32::from_le_bytes(t[at + 368..at + 372].try_into().unwrap()), 3);
        let r = from_edison(&b).unwrap();
        assert_eq!(r, s);
        // An empty Edison: an empty float stereo WAVE, defaults.
        let e = to_edison(&Saved::default());
        let r = from_edison(&e).unwrap();
        assert!(r.sample.is_empty());
        assert_eq!(r.rec, Rec::default());
        // The old preset head (version 4): 16 bytes then the WAVE; settings stay default.
        let w = wav::write_as(&s.sample, Depth::Own, "Edison");
        let mut old = Vec::new();
        for v in [4u32, 0, 0, w.len() as u32] {
            old.extend_from_slice(&v.to_le_bytes());
        }
        old.extend_from_slice(&w);
        let r = from_edison(&old).unwrap();
        assert_eq!(r.sample, s.sample);
        assert!(!r.rec.armed);
        // Edited envelopes are kept and written back as they came.
        let mut edited = to_edison(&s);
        let env_at = 24 + wave + at + ENV_AT;
        edited[env_at + 20] ^= 0x40;
        let r = from_edison(&edited).unwrap();
        assert_eq!(r.edison_env.len(), ENV_LEN);
        assert_eq!(to_edison(&r), edited);
    }
}
