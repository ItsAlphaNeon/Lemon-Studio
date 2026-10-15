// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Sample loading: decode an audio file to stereo `f32` frames.
//!
// spec: flp/sample-paths.md (placeholders, formats), audio/sample-formats.md (Vorbis in WAV, MP3, AIFF)

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::{Hint, Probe};
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Decoded audio, always stereo (mono is duplicated, extra channels are dropped).
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl Sample {
    pub fn frames(&self) -> usize {
        self.left.len()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SampleError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: {message}")]
    Decode { path: PathBuf, message: String },
}

fn codecs() -> &'static CodecRegistry {
    static REG: OnceLock<CodecRegistry> = OnceLock::new();
    REG.get_or_init(|| {
        let mut r = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut r);
        r.register_audio_decoder::<symphonia_codec_wavpack::WavPackDecoder>();
        r
    })
}

fn probe() -> &'static Probe {
    static PROBE: OnceLock<Probe> = OnceLock::new();
    PROBE.get_or_init(|| {
        let mut p = Probe::default();
        symphonia::default::register_enabled_formats(&mut p);
        p.register_format::<symphonia_codec_wavpack::WavPackReader<'_>>();
        p
    })
}

/// Size and modification time of a file: a cached decode is reused only while both match.
type Stamp = (u64, Option<std::time::SystemTime>);

fn stamp(path: &Path) -> Option<Stamp> {
    std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().ok()))
}

/// `load`, shared: while any holder keeps a decoded file alive, loading the same unchanged file again returns that
/// copy instead of decoding it again. Project loading decodes samples on worker threads this way so that the plugin
/// instances made afterwards on the UI thread find them ready (spec: ui/project-loading.md).
pub fn load_shared(path: &Path) -> Result<Arc<Sample>, SampleError> {
    static LIVE: OnceLock<std::sync::Mutex<HashMap<PathBuf, (std::sync::Weak<Sample>, Stamp)>>> = OnceLock::new();
    let live = LIVE.get_or_init(Default::default);
    let st = stamp(path);
    if let (Some(st), Ok(map)) = (st, live.lock()) {
        if let Some(s) = map.get(path).filter(|(_, s)| *s == st).and_then(|(w, _)| w.upgrade()) {
            return Ok(s);
        }
    }
    let s = Arc::new(load(path)?);
    if let (Some(st), Ok(mut map)) = (st, live.lock()) {
        map.retain(|_, (w, _)| w.strong_count() > 0);
        map.insert(path.to_owned(), (Arc::downgrade(&s), st));
    }
    Ok(s)
}

/// Decodes a WAV (PCM or Vorbis-in-WAV) / FLAC / Ogg Vorbis / WavPack / MP3 / AIFF file.
pub fn load(path: &Path) -> Result<Sample, SampleError> {
    let bytes = std::fs::read(path).map_err(|source| SampleError::Io { path: path.to_owned(), source })?;
    let decode_err = |message: String| SampleError::Decode { path: path.to_owned(), message };
    if let Some(vw) = vorbis_in_wav(&bytes) {
        let mut s = decode(vw.ogg.to_vec(), Some("ogg")).map_err(decode_err)?;
        if let Some(frames) = vw.fact_frames {
            s.left.truncate(frames);
            s.right.truncate(frames);
        }
        return Ok(s);
    }
    let ext = path.extension().and_then(|e| e.to_str());
    decode(bytes, ext).map_err(decode_err)
}

/// A WAV file whose `fmt ` tag is one of the Vorbis-ACM tags: the `data` chunk holds a
/// complete Ogg Vorbis stream, and `fact` holds the true length in frames.
struct VorbisInWav<'a> {
    ogg: &'a [u8],
    fact_frames: Option<usize>,
}

fn vorbis_in_wav(b: &[u8]) -> Option<VorbisInWav<'_>> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let (mut tag, mut data, mut fact) = (None, None, None);
    let mut pos = 12;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = b.get(pos + 8..(pos + 8 + len).min(b.len()))?;
        match id {
            b"fmt " if body.len() >= 2 => tag = Some(u16::from_le_bytes([body[0], body[1]])),
            b"data" => data = Some(body),
            b"fact" if body.len() >= 4 => fact = Some(u32::from_le_bytes(body[..4].try_into().ok()?) as usize),
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    match tag? {
        0x674F | 0x6750 | 0x6751 | 0x676F | 0x6770 | 0x6771 => Some(VorbisInWav { ogg: data?, fact_frames: fact }),
        _ => None,
    }
}

/// The length in frames of an MP3 stream's leading info frame (a Xing / Info / VBRI tag in the first MPEG frame, after
/// any ID3v2 tag), when it has one: 1152 for MPEG-1, 576 for MPEG-2 and 2.5. The tag is recognised as the decoder
/// recognises it (Layer III, the side information before it all zero).
fn mp3_info_frame(b: &[u8]) -> Option<usize> {
    let mut pos = 0;
    if b.len() >= 10 && &b[..3] == b"ID3" {
        let size = b[6..10].iter().fold(0usize, |n, &x| n << 7 | usize::from(x & 0x7F));
        pos = 10 + size + if b[5] & 0x10 != 0 { 10 } else { 0 };
    }
    // The first frame header: sync, a valid version, Layer III, a valid bitrate and sample rate.
    let h = b.get(pos..pos + 4)?;
    let version = (h[1] >> 3) & 3;
    if h[0] != 0xFF || h[1] & 0xE0 != 0xE0 || version == 1 || (h[1] >> 1) & 3 != 1 || h[2] >> 4 == 0xF || (h[2] >> 2) & 3 == 3 {
        return None;
    }
    let (mpeg1, mono) = (version == 3, h[3] >> 6 == 3);
    let side = match (mpeg1, mono) {
        (true, true) => 17,
        (true, false) => 32,
        (false, true) => 9,
        (false, false) => 17,
    };
    let header = if h[1] & 1 == 0 { 6 } else { 4 };
    let zero = |a: usize, z: usize| b.get(pos + a..pos + z).is_some_and(|s| s.iter().all(|&x| x == 0));
    let id = |at: usize| b.get(pos + at..pos + at + 4);
    let xing = matches!(id(4 + side), Some(b"Xing" | b"Info")) && zero(header, 4 + side);
    let vbri = id(36) == Some(b"VBRI") && zero(header, 36);
    (xing || vbri).then_some(if mpeg1 { 1152 } else { 576 })
}

fn decode(bytes: Vec<u8>, ext: Option<&str>) -> Result<Sample, String> {
    // MP3 plays every decoded frame, the encoder's info frame as silence in front (spec: audio/sampler.md, the table
    // of formats; audio/sample-formats.md).
    let info_frame = mp3_info_frame(&bytes);
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = ext {
        hint.with_extension(ext);
    }
    let mut format = probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| e.to_string())?;
    let track = format.default_track(TrackType::Audio).ok_or("no audio track")?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("no audio codec parameters")?;
    let mp3 = format.format_info().short_name == "mp3";
    let mut decoder = codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default().gapless(!mp3))
        .map_err(|e| e.to_string())?;

    let mut rate = 0;
    let silence = if mp3 { info_frame.unwrap_or(0) } else { 0 };
    let (mut left, mut right) = (vec![0.0; silence], vec![0.0; silence]);
    let mut buf: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id != track_id {
            continue;
        }
        let audio = match decoder.decode(&packet) {
            Ok(a) => a,
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        };
        rate = audio.spec().rate();
        let ch = audio.spec().channels().count().max(1);
        buf.resize(audio.samples_interleaved(), 0.0);
        audio.copy_to_slice_interleaved(&mut buf);
        for frame in buf.chunks_exact(ch) {
            left.push(frame[0]);
            right.push(if ch > 1 { frame[1] } else { frame[0] });
        }
    }
    if rate == 0 {
        return Err("no audio decoded".into());
    }
    Ok(Sample { rate, left, right })
}

/// Resolves project sample paths (with `%FLStudioFactoryData%` / `%FLStudioData%`
/// placeholders) and caches decoded samples.
#[derive(Debug, Default)]
pub struct SampleLibrary {
    /// The user's FL Studio install folder; `None` = placeholder paths can't be resolved.
    pub factory_dir: Option<PathBuf>,
    cache: HashMap<String, Option<Arc<Sample>>>,
    /// Built-in tones by output rate (the engine's empty-sampler fallback), built once.
    pub(crate) tones: HashMap<u32, Arc<Sample>>,
    /// Paths that couldn't be resolved or decoded, with the reason.
    pub failures: Vec<(String, String)>,
    /// Stretched copies made so far: (source, length in frames, pitch factor bits, copy).
    stretched: Vec<(Arc<Sample>, usize, u64, Arc<Sample>)>,
    /// Engines built from this library play every stretch method by resampling, at the stretched length: for a host
    /// whose sampler plugin stretches for itself (spec: audio/time-stretch.md "Lemon Studio").
    pub resample_stretches: bool,
}

impl SampleLibrary {
    pub fn new(factory_dir: Option<PathBuf>) -> Self {
        SampleLibrary { factory_dir, ..Default::default() }
    }

    /// The FL Studio install folder on this machine, if one is found: the folder named by the environment
    /// variable `OPENSTUDIO_FACTORY`, else the default install location (on macOS the newest
    /// `/Applications/FL Studio ....app`, whose install folder is `Contents/Resources/FL`), else (off Windows)
    /// the newest "FL Studio ..." folder in the Wine prefix (`WINEPREFIX`, or `~/.wine`).
    pub fn default_factory_dir() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("OPENSTUDIO_FACTORY").map(PathBuf::from) {
            return p.is_dir().then_some(p);
        }
        let p = PathBuf::from(r"C:\Program Files\Image-Line\FL Studio 2026");
        if p.is_dir() {
            return Some(p);
        }
        #[cfg(target_os = "macos")]
        if let Some(p) = Self::mac_factory_dir() {
            return Some(p);
        }
        Self::wine_factory_dir()
    }

    #[cfg(target_os = "macos")]
    fn mac_factory_dir() -> Option<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir("/Applications")
            .ok()?
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with("FL Studio") && n.ends_with(".app")))
            .map(|e| e.path().join("Contents/Resources/FL"))
            .filter(|p| p.join("Data/Patches").is_dir())
            .collect();
        found.sort();
        found.pop()
    }

    #[cfg(windows)]
    fn wine_factory_dir() -> Option<PathBuf> {
        None
    }
    #[cfg(not(windows))]
    fn wine_factory_dir() -> Option<PathBuf> {
        let prefix = std::env::var_os("WINEPREFIX").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".wine")))?;
        let mut found: Vec<PathBuf> = std::fs::read_dir(prefix.join("drive_c/Program Files/Image-Line"))
            .ok()?
            .flatten()
            .map(|e| e.path())
            // (Image-Line's ASIO driver installs to "FL Studio ASIO": an install has the factory data.)
            .filter(|p| p.join("Data/Patches").is_dir() && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("FL Studio")))
            .collect();
        found.sort();
        found.pop()
    }

    pub fn resolve(&self, project_path: &str) -> Option<PathBuf> {
        const FACTORY: &str = "%FLStudioFactoryData%";
        const DATA: &str = "%FLStudioData%";
        let rel = |rest: &str| rest.trim_start_matches(['\\', '/']).replace('\\', std::path::MAIN_SEPARATOR_STR);
        if let Some(rest) = project_path.strip_prefix(FACTORY) {
            Some(self.factory_dir.as_ref()?.join(rel(rest)))
        } else if let Some(rest) = project_path.strip_prefix(DATA) {
            Some(self.factory_dir.as_ref()?.join("Data").join(rel(rest)))
        } else {
            Some(Self::local_path(project_path))
        }
    }

    /// An absolute path as stored in a project. Projects store Windows paths; off Windows a path on drive Z:
    /// (where Wine shows the whole file system) is the same path from the root, with forward slashes.
    #[cfg(windows)]
    fn local_path(project_path: &str) -> PathBuf {
        PathBuf::from(project_path)
    }
    #[cfg(not(windows))]
    fn local_path(project_path: &str) -> PathBuf {
        let b = project_path.as_bytes();
        if b.len() >= 3 && b[0].eq_ignore_ascii_case(&b'z') && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') {
            return PathBuf::from(project_path[2..].replace('\\', "/"));
        }
        PathBuf::from(project_path)
    }

    pub fn get(&mut self, project_path: &str) -> Option<Arc<Sample>> {
        if let Some(s) = self.cache.get(project_path) {
            return s.clone();
        }
        let loaded = match self.resolve(project_path) {
            None => {
                self.failures.push((project_path.into(), "factory folder not set".into()));
                None
            }
            // Shared with the other users of the file (plugin instances, playlist waveforms).
            Some(p) => match load_shared(&p) {
                Ok(s) => Some(s),
                Err(e) => {
                    self.failures.push((project_path.into(), e.to_string()));
                    None
                }
            },
        };
        self.cache.insert(project_path.into(), loaded.clone());
        loaded
    }

    /// `raw` stretched to `frames` frames with its pitch multiplied by `transpose` (spec: audio/time-stretch.md), made
    /// once per library.
    pub fn stretched(&mut self, raw: &Arc<Sample>, frames: usize, transpose: f64) -> Arc<Sample> {
        let bits = transpose.to_bits();
        if let Some(e) = self.stretched.iter().find(|e| Arc::ptr_eq(&e.0, raw) && e.1 == frames && e.2 == bits) {
            return e.3.clone();
        }
        let (left, right) = os_dsp::stretch::whole(&raw.left, &raw.right, raw.rate, frames, transpose);
        let s = Arc::new(Sample { rate: raw.rate, left, right });
        self.stretched.push((raw.clone(), frames, bits, s.clone()));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tests/data/click_sine.mp3` (made with ffmpeg + LAME, 64 kbit/s mono, ID3v2 tag, LAME info frame): 0.3 s at
    /// 44.1 kHz, a 10-frame click of 0.9 at frame 4410, then a 440 Hz sine of 0.25 from frame 8820.
    #[test]
    fn mp3_decodes_in_time_and_level() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/click_sine.mp3");
        assert_eq!(mp3_info_frame(&std::fs::read(&path).unwrap()), Some(1152));
        let s = load(&path).expect("mp3 decodes");
        assert_eq!(s.rate, 44100);
        assert_eq!(s.left, s.right, "mono is duplicated");
        let peak = (0..s.frames()).max_by(|&a, &b| s.left[a].abs().total_cmp(&s.left[b].abs())).unwrap();
        eprintln!("frames {} click at {peak}", s.frames());
        // Every decoded frame plays: the info frame's 1152 frames of silence, then the encoder delay (576) and the
        // decoder's own (529) before the click, and whole MPEG frames to the end.
        assert!(s.left[..1152].iter().all(|&v| v == 0.0));
        assert!((4410 + 1152 + 1105 - 5..4410 + 1152 + 1105 + 10).contains(&peak), "click at {peak}");
        assert_eq!(s.frames() % 1152, 0, "length {}", s.frames());
        assert!(s.frames() >= 1152 + 1105 + 13230, "length {}", s.frames());
        let tail = &s.left[9000 + 2257..12000 + 2257];
        let rms = (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt();
        assert!((rms - 0.25 / 2f32.sqrt()).abs() < 0.02, "sine rms {rms}");
    }

    /// A kept decode is handed out again; once every holder dropped it, or the file changed, the file is decoded anew.
    #[test]
    fn shared_loads_reuse_a_live_decode() {
        let path = std::env::temp_dir().join(format!("os_engine_shared_{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&path).unwrap(), 48000, &[0.25; 960]).unwrap();
        let a = load_shared(&path).unwrap();
        let b = load_shared(&path).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "the live decode is shared");
        drop((a, b));
        let c = load_shared(&path).unwrap();
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&path).unwrap(), 48000, &[0.5; 1920]).unwrap();
        let d = load_shared(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(!Arc::ptr_eq(&c, &d), "a changed file is decoded again");
        assert_eq!(d.frames(), 960);
    }
}
