// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Sample loading: decode an audio file to stereo `f32` frames.
//!
// spec: flp/sample-paths.md (placeholders, formats), audio/sample-formats.md (Vorbis in WAV, MP3, AIFF)

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, SeekFrom};
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

/// The decodes `load_shared` handed out, by file, with the stamp of the file they were made from.
fn live() -> &'static std::sync::Mutex<HashMap<PathBuf, (std::sync::Weak<Sample>, Stamp)>> {
    static LIVE: OnceLock<std::sync::Mutex<HashMap<PathBuf, (std::sync::Weak<Sample>, Stamp)>>> = OnceLock::new();
    LIVE.get_or_init(Default::default)
}

/// Whether the last decode `load_shared` made of the file at `path` is still held anywhere (a sampler, the engine's
/// library, a project loading).
pub fn held(path: &Path) -> bool {
    live().lock().is_ok_and(|map| map.get(path).is_some_and(|(w, _)| w.strong_count() > 0))
}

/// `load`, shared: while any holder keeps a decoded file alive, loading the same unchanged file again returns that
/// copy instead of decoding it again. Project loading decodes samples on worker threads this way so that the plugin
/// instances made afterwards on the UI thread find them ready (spec: ui/project-loading.md).
pub fn load_shared(path: &Path) -> Result<Arc<Sample>, SampleError> {
    let live = live();
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

/// The length of the audio file at `path`: its frames and their rate. Read from the file's headers where they give it
/// (WAV, AIFF, FLAC, most MP3s), else found by decoding the file. `None` when it can't be read.
pub fn length(path: &Path) -> Option<(u64, u32)> {
    if let Some(n) = header_length(path) {
        return Some(n);
    }
    let s = load_shared(path).ok()?;
    Some((s.frames() as u64, s.rate))
}

/// The length the file's headers give, without decoding it.
fn header_length(path: &Path) -> Option<(u64, u32)> {
    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let format = probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default()).ok()?;
    let track = format.default_track(TrackType::Audio)?;
    let rate = track.codec_params.as_ref()?.audio()?.sample_rate.filter(|&r| r > 0)?;
    Some((track.num_frames?, rate))
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

/// What a WAV file's sampler and loop chunks say about playing it (spec: ui/settings-choices.md "Advanced: Read sample
/// root note and Read sample tempo information"): `smpl`'s MIDI unity note, the sample's root key, and `acid`'s beat
/// count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WavTags {
    /// The first `smpl` chunk's MIDI unity note (the 4th u32 of its body), when it is a key (0..=127).
    pub unity_note: Option<u8>,
    /// The first `acid` chunk's beat count (the u32 at body offset 12, after the flags u32, the root note u16, a u16
    /// and a float), when it is above 0 and the flags don't mark the file a one-shot (bit 0): a one-shot's beat count
    /// is not a length to fit it to.
    pub beats: Option<u32>,
}

/// The `smpl` and `acid` chunks of the file at `path`. A file that isn't a RIFF WAV, or can't be read, has neither.
/// Only the chunk headers and those two bodies are read.
pub fn wav_tags(path: &Path) -> WavTags {
    std::fs::File::open(path).ok().and_then(|f| tags_of(std::io::BufReader::new(f))).unwrap_or_default()
}

fn tags_of(mut r: impl Read + Seek) -> Option<WavTags> {
    let mut head = [0u8; 12];
    r.read_exact(&mut head).ok()?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }
    let mut tags = WavTags::default();
    let mut acid_seen = false;
    let mut chunk = [0u8; 8];
    while r.read_exact(&mut chunk).is_ok() {
        let len = u64::from(u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]));
        let mut skip = len + (len & 1);
        if matches!(&chunk[0..4], b"smpl" | b"acid") && len >= 16 {
            let mut body = [0u8; 16];
            if r.read_exact(&mut body).is_err() {
                break;
            }
            skip -= 16;
            let v = u32::from_le_bytes([body[12], body[13], body[14], body[15]]);
            if &chunk[0..4] == b"smpl" {
                tags.unity_note = tags.unity_note.or(u8::try_from(v).ok().filter(|&k| k <= 127));
            } else if !acid_seen {
                acid_seen = true;
                let one_shot = body[0] & 1 != 0;
                tags.beats = (v > 0 && !one_shot).then_some(v);
            }
        }
        if r.seek(SeekFrom::Current(i64::try_from(skip).ok()?)).is_err() {
            break;
        }
    }
    Some(tags)
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

/// The placeholders of sample paths: the FL Studio install folder and its `Data` folder (spec: flp/sample-paths.md).
const FACTORY: &str = "%FLStudioFactoryData%";
const DATA: &str = "%FLStudioData%";

/// `text` without `prefix`, compared without regard to ASCII case.
fn strip_prefix_ignoring_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
}

/// `path` as it is on disk when its parts match existing ones without regard to case (an exact match first): `None`
/// when a part is not found.
fn match_case(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        let Component::Normal(name) = part else {
            out.push(part.as_os_str());
            continue;
        };
        let want = name.to_string_lossy().to_lowercase();
        let mut found = None;
        let dir = if out.as_os_str().is_empty() { Path::new(".") } else { &out };
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let n = e.file_name();
            if n == name {
                found = Some(n);
                break;
            }
            if found.is_none() && n.to_string_lossy().to_lowercase() == want {
                found = Some(n);
            }
        }
        // A name the folder's listing does not show (a short 8.3 name on Windows) that opens all the same.
        let found = found.or_else(|| out.join(name).exists().then(|| name.to_os_string()))?;
        out.push(found);
    }
    Some(out)
}

/// The folders a missing sample is looked for in by its file name (spec: flp/sample-paths.md "Missing samples"): the
/// opened project's folder and data folder, then the folders the user picked for this project.
fn search() -> &'static std::sync::RwLock<Vec<PathBuf>> {
    static SEARCH: OnceLock<std::sync::RwLock<Vec<PathBuf>>> = OnceLock::new();
    SEARCH.get_or_init(Default::default)
}

thread_local! {
    /// The folders as the thread that set them last gave them: a host that opens several projects at once on threads
    /// of their own (tests) keeps each thread's.
    static SEARCH_HERE: std::cell::RefCell<Option<Vec<PathBuf>>> = const { std::cell::RefCell::new(None) };
}

/// The folders missing samples are looked for in, in order (`SampleLibrary::resolve`), for every thread.
pub fn set_search_folders(dirs: Vec<PathBuf>) {
    SEARCH_HERE.with(|h| *h.borrow_mut() = Some(dirs.clone()));
    if let Ok(mut s) = search().write() {
        *s = dirs;
    }
}

/// The folders for this thread only (other threads keep theirs): a test starts its own empty.
pub fn set_thread_search_folders(dirs: Vec<PathBuf>) {
    SEARCH_HERE.with(|h| *h.borrow_mut() = Some(dirs));
}

/// The search folders: this thread's own when it set them, else the last ones set.
pub fn search_folders() -> Vec<PathBuf> {
    if let Some(d) = SEARCH_HERE.with(|h| h.borrow().clone()) {
        return d;
    }
    search().read().map(|s| s.clone()).unwrap_or_default()
}

/// The file of `path`'s name in the first search folder that has one (its name matched without regard to case). A
/// Windows path's name is found off Windows too, where `\` does not separate folders.
fn by_name(path: &Path) -> Option<PathBuf> {
    let name = path.to_str().map_or(path.file_name(), |p| p.rsplit(['\\', '/']).next().map(std::ffi::OsStr::new))?;
    if name.is_empty() {
        return None;
    }
    search_folders().iter().filter(|d| reachable(d)).find_map(|d| {
        let p = d.join(name);
        if p.is_file() {
            return Some(p);
        }
        match_case(&p).filter(|p| p.is_file())
    })
}

/// How long the first look at a drive or network share may take before its samples count as missing.
pub const REACH_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);
/// How long a drive or share that did not answer in time is skipped before it is tried again.
const UNREACHABLE_FOR: std::time::Duration = std::time::Duration::from_secs(60);

/// The drive or network share a path is on (`C:\`, `\\server\share\`), or `/` off Windows; `None` for a relative path.
fn root_of(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let mut root = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => root.push(c.as_os_str()),
            _ => break,
        }
    }
    (!root.as_os_str().is_empty()).then_some(root)
}

/// Whether the drive or share `path` is on answers (spec: flp/sample-paths.md "Missing samples"). A sample path names
/// a drive or share of the machine the project was made on, which here can be a network location that is gone or a
/// drive that blocks every look at it for many seconds. Each root is looked at once, on a thread of its own, for at
/// most `REACH_TIMEOUT`; one that did not answer counts as missing for `UNREACHABLE_FOR`, without waiting again.
pub fn reachable(path: &Path) -> bool {
    use std::sync::mpsc;
    use std::time::Instant;
    /// A root's last look: when, and whether it answered (`None`: still looking).
    type Roots = HashMap<PathBuf, (Instant, Option<bool>)>;
    static ROOTS: OnceLock<std::sync::Mutex<Roots>> = OnceLock::new();
    let Some(root) = root_of(path) else { return true };
    let roots = ROOTS.get_or_init(Default::default);
    if let Some(&(at, answer)) = roots.lock().ok().as_ref().and_then(|r| r.get(&root)) {
        match answer {
            Some(true) => return true,
            Some(false) if at.elapsed() < UNREACHABLE_FOR => return false,
            // A look still running from before: the root has not answered for that long.
            None if at.elapsed() >= REACH_TIMEOUT => return false,
            _ => {}
        }
    }
    let (tx, rx) = mpsc::channel();
    let probe = root.clone();
    if std::thread::Builder::new().name("sample-root".into()).spawn(move || tx.send(probe.exists()).ok()).is_err() {
        return true;
    }
    if let Ok(mut r) = roots.lock() {
        r.insert(root.clone(), (Instant::now(), None));
    }
    // A root that does not exist answers at once (false): its paths are simply missing, not slow.
    let answer = rx.recv_timeout(REACH_TIMEOUT).ok();
    if let Ok(mut r) = roots.lock() {
        r.insert(root, (Instant::now(), Some(answer.is_some())));
    }
    answer.is_some()
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
    /// Reads a file's length in frames and its rate from its headers, the frames exactly those decoding gives, for the
    /// files it knows (`None` for the others): for a host whose sampler plugin plays the channels' samples itself
    /// (Lemon Studio gives Lemon Sampler's, which knows the files it can play from disk). Engines built with hosted
    /// instruments then keep only that of a sample one of them plays, not its decoded frames (`length`,
    /// `Engine::with_instruments`).
    pub length_reader: Option<fn(&Path) -> Option<(usize, u32)>>,
    /// Lengths read so far, by project path, with the stamp of the file they were read from.
    lengths: HashMap<String, (Stamp, Option<(usize, u32)>)>,
    /// Tells whether the host's sampler plugin plays a file with a sustain loop of its own (a WAV `smpl` loop), for a
    /// host whose sampler plays the channels' samples itself (Lemon Studio gives Lemon Sampler's): such a channel's
    /// voices sound until their notes end, so the note rules must hand them their note ends (`file_loops`).
    pub loop_reader: Option<fn(&Path) -> bool>,
    /// Loops read so far, by project path, with the stamp of the file they were read from.
    loops: HashMap<String, (Stamp, bool)>,
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

    /// The file a project's sample path names: a placeholder resolved against the install folder (`None` while it is
    /// not set), else the path itself. Paths are matched without regard to case and take either slash as a separator
    /// (spec: flp/layouts.md "Older files", flp/sample-paths.md): where the path as written does not exist, each part
    /// is looked for ignoring case.
    pub fn resolve(&self, project_path: &str) -> Option<PathBuf> {
        let rel = |rest: &str| rest.trim_start_matches(['\\', '/']).replace(['\\', '/'], std::path::MAIN_SEPARATOR_STR);
        let path = if let Some(rest) = strip_prefix_ignoring_case(project_path, FACTORY) {
            self.factory_dir.as_ref()?.join(rel(rest))
        } else if let Some(rest) = strip_prefix_ignoring_case(project_path, DATA) {
            self.factory_dir.as_ref()?.join("Data").join(rel(rest))
        } else {
            Self::local_path(project_path)
        };
        if !reachable(&path) {
            return Some(by_name(&path).unwrap_or(path));
        }
        if path.exists() {
            return Some(path);
        }
        Some(match_case(&path).or_else(|| by_name(&path)).unwrap_or(path))
    }

    /// Whether a project's sample path starts with one of the install folder's placeholders (in any case).
    pub fn has_placeholder(project_path: &str) -> bool {
        [FACTORY, DATA].iter().any(|p| strip_prefix_ignoring_case(project_path, p).is_some())
    }

    /// The install folder becomes `dir`: samples found through the old one are looked for again.
    pub fn set_factory_dir(&mut self, dir: Option<PathBuf>) {
        if dir == self.factory_dir {
            return;
        }
        self.factory_dir = dir;
        self.cache.retain(|k, _| !Self::has_placeholder(k));
        self.failures.retain(|(k, _)| !Self::has_placeholder(k));
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

    /// The length in frames and the rate of the sample at `project_path`, read by `length_reader` without decoding
    /// the file: all an engine needs of a sample a hosted instrument plays (spec: plugins/lemon-sampler.md "Keep on
    /// disk (Lemon Studio)"). The library lets go of its own decode of the file, if it made one (an engine that still
    /// plays it keeps its copy until it goes). `None` without a reader, or for a file the reader does not know: the
    /// file is decoded then (`get`).
    pub fn length(&mut self, project_path: &str) -> Option<(usize, u32)> {
        let read = self.length_reader?;
        let path = self.resolve(project_path)?;
        let st = stamp(&path)?;
        let n = match self.lengths.get(project_path) {
            Some(&(s, n)) if s == st => n,
            _ => {
                let n = read(&path);
                self.lengths.insert(project_path.into(), (st, n));
                n
            }
        };
        if n.is_some() {
            self.cache.remove(project_path);
        }
        n
    }

    /// Whether the sample at `project_path` has a loop of its own the host's sampler plugin plays (`loop_reader`, read
    /// once per version of the file). False without a reader, or for a file that is not found.
    pub fn file_loops(&mut self, project_path: &str) -> bool {
        let Some(read) = self.loop_reader else { return false };
        let Some(path) = self.resolve(project_path) else { return false };
        let Some(st) = stamp(&path) else { return false };
        match self.loops.get(project_path) {
            Some(&(s, l)) if s == st => l,
            _ => {
                let l = read(&path);
                self.loops.insert(project_path.into(), (st, l));
                l
            }
        }
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

    // spec: ui/settings-choices.md "Advanced: Fast sample preview, Auto keep long audio on disk, Threaded project
    // sample loading (2026-10-16)"
    /// A file's length is its frames and their rate, from the header (16-bit mono at 22.05 kHz, float stereo at
    /// 96 kHz) or by decoding it (the MP3); a file that can't be read has none.
    #[test]
    fn lengths_in_frames_and_rate() {
        let dir = std::env::temp_dir();
        let mono = dir.join(format!("os_engine_length_mono_{}.wav", std::process::id()));
        let mut fmt = vec![1, 0, 1, 0];
        fmt.extend(22050u32.to_le_bytes());
        fmt.extend(44100u32.to_le_bytes());
        fmt.extend([2, 0, 16, 0]);
        let mut body = b"WAVE".to_vec();
        body.extend(chunk(b"fmt ", &fmt));
        body.extend(chunk(b"data", &vec![0; 2 * 661_500]));
        let mut b = b"RIFF".to_vec();
        b.extend((body.len() as u32).to_le_bytes());
        b.extend(body);
        std::fs::write(&mono, b).unwrap();
        assert_eq!(length(&mono), Some((661_500, 22050)), "30 s at 22.05 kHz");
        let stereo = dir.join(format!("os_engine_length_stereo_{}.wav", std::process::id()));
        crate::wav::write_f32_stereo(&mut std::fs::File::create(&stereo).unwrap(), 96000, &vec![0.1; 2 * 96_000]).unwrap();
        assert_eq!(length(&stereo), Some((96_000, 96000)), "1 s at 96 kHz");
        for p in [mono, stereo] {
            let _ = std::fs::remove_file(p);
        }
        let mp3 = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/click_sine.mp3");
        let (frames, rate) = length(&mp3).expect("the mp3's length");
        assert_eq!(rate, 44100);
        assert!((13230..=13230 + 2 * 1152).contains(&frames), "about 0.3 s: {frames}");
        assert_eq!(length(Path::new("Z:/no such folder/none.wav")), None);
    }

    /// A RIFF chunk: id, little-endian length, body, a pad byte after an odd length.
    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut c = id.to_vec();
        c.extend((body.len() as u32).to_le_bytes());
        c.extend(body);
        if body.len() % 2 == 1 {
            c.push(0);
        }
        c
    }

    /// A WAV of 4 silent 16-bit mono frames with `extra` chunks before `data`.
    fn wav(extra: &[Vec<u8>]) -> Vec<u8> {
        let mut fmt = vec![1, 0, 1, 0];
        fmt.extend(44100u32.to_le_bytes());
        fmt.extend(88200u32.to_le_bytes());
        fmt.extend([2, 0, 16, 0]);
        let mut body = b"WAVE".to_vec();
        body.extend(chunk(b"fmt ", &fmt));
        extra.iter().for_each(|c| body.extend(c));
        body.extend(chunk(b"data", &[0; 8]));
        let mut b = b"RIFF".to_vec();
        b.extend((body.len() as u32).to_le_bytes());
        b.extend(body);
        b
    }

    /// `smpl` with MIDI unity note `note`: manufacturer, product, sample period, unity note, pitch fraction, SMPTE
    /// format and offset, no loops, no sampler data.
    fn smpl(note: u32) -> Vec<u8> {
        let mut b = Vec::new();
        for v in [0, 0, 22675, note, 0, 0, 0, 0, 0] {
            b.extend(u32::to_le_bytes(v));
        }
        chunk(b"smpl", &b)
    }

    /// `acid` with `beats` beats: flags (root note set), root note, two unused fields, beats, meter 4/4, tempo.
    fn acid(beats: u32, root: u16, bpm: f32) -> Vec<u8> {
        acid_flags(2, beats, root, bpm)
    }

    fn acid_flags(flags: u32, beats: u32, root: u16, bpm: f32) -> Vec<u8> {
        let mut b = flags.to_le_bytes().to_vec();
        b.extend(root.to_le_bytes());
        b.extend(0x8000u16.to_le_bytes());
        b.extend(0f32.to_le_bytes());
        b.extend(beats.to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend(bpm.to_le_bytes());
        chunk(b"acid", &b)
    }

    fn tags(b: Vec<u8>) -> Option<WavTags> {
        tags_of(Cursor::new(b))
    }

    /// The chunk combinations of the measured table (spec: ui/settings-choices.md "Advanced: Read sample root note and
    /// Read sample tempo information"), each read back.
    #[test]
    fn wav_tags_read_the_sampler_and_loop_chunks() {
        let none = WavTags::default();
        assert_eq!(tags(wav(&[])), Some(none), "no chunks");
        assert_eq!(tags(wav(&[smpl(72)])), Some(WavTags { unity_note: Some(72), beats: None }));
        assert_eq!(tags(wav(&[acid(4, 64, 120.0)])), Some(WavTags { unity_note: None, beats: Some(4) }), "acid's root is not read");
        assert_eq!(tags(wav(&[smpl(67), acid(4, 60, 95.0)])), Some(WavTags { unity_note: Some(67), beats: Some(4) }));
        assert_eq!(tags(wav(&[acid(8, 60, 120.0)])), Some(WavTags { unity_note: None, beats: Some(8) }), "the tempo is not used");
        // After `data`, and behind an odd-length chunk with its pad byte.
        let mut b = wav(&[chunk(b"LIST", b"INFOx")]);
        let extra = [smpl(48), acid(16, 0, 0.0)].concat();
        b.extend(&extra);
        let riff = (b.len() - 8) as u32;
        b[4..8].copy_from_slice(&riff.to_le_bytes());
        assert_eq!(tags(b), Some(WavTags { unity_note: Some(48), beats: Some(16) }));
        // Unusable values count as no chunk: 0 beats, a unity note above 127; a body too short is skipped.
        assert_eq!(tags(wav(&[acid(0, 60, 120.0), smpl(200)])), Some(none));
        assert_eq!(tags(wav(&[chunk(b"smpl", &[0; 12])])), Some(none));
        // A one-shot (flags bit 0) has no length to fit: its beats are not read (user report 2026-10-19: short
        // one-shots stretched to a beat sounded like a feedback loop on every hit).
        assert_eq!(tags(wav(&[acid_flags(1, 4, 60, 120.0)])), Some(none));
        assert_eq!(tags(wav(&[acid_flags(0x13, 2, 60, 120.0)])), Some(none));
        assert_eq!(tags(wav(&[acid_flags(0, 2, 60, 120.0)])).unwrap().beats, Some(2));
        // The first chunk of each kind counts.
        assert_eq!(tags(wav(&[smpl(50), smpl(70)])).unwrap().unity_note, Some(50));
        // Not a WAV.
        assert_eq!(tags(b"fLaC\0\0\0\x22".to_vec()), None);
        assert_eq!(tags(b"RIFF".to_vec()), None);
        // A cut-off file keeps what it read before the cut.
        let mut b = wav(&[smpl(61), acid(2, 0, 0.0)]);
        b.truncate(b.len() - 20);
        assert_eq!(tags(b).unwrap().unity_note, Some(61));
        // From a file; a missing file has none.
        let path = std::env::temp_dir().join(format!("os_engine_wav_tags_{}.wav", std::process::id()));
        std::fs::write(&path, wav(&[smpl(67), acid(4, 60, 95.0)])).unwrap();
        assert_eq!(wav_tags(&path), WavTags { unity_note: Some(67), beats: Some(4) });
        let _ = std::fs::remove_file(&path);
        assert_eq!(wav_tags(&path), none);
    }

    /// Sample paths are matched without regard to case, placeholders included, and take either slash (spec:
    /// flp/layouts.md "Older files", flp/sample-paths.md); a changed install folder finds its samples again.
    #[test]
    fn sample_paths_ignore_case() {
        let root = std::env::temp_dir().join(format!("os_engine_case_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (one, two) = (root.join("FL One"), root.join("FL Two"));
        for d in [&one, &two] {
            std::fs::create_dir_all(d.join("Data").join("Patches").join("Packs")).unwrap();
            std::fs::write(d.join("Data").join("Patches").join("Packs").join("Kick.WAV"), wav(&[])).unwrap();
        }
        let file = one.join("Data").join("Patches").join("Packs").join("Kick.WAV");
        // Each part found as it is on disk, the exact name first.
        let asked = one.join("data").join("PATCHES").join("packs").join("kick.wav");
        let tail = Path::new("FL One").join("Data").join("Patches").join("Packs").join("Kick.WAV");
        assert!(match_case(&asked).is_some_and(|p| p.ends_with(&tail)), "{:?}", match_case(&asked));
        assert_eq!(match_case(&one.join("Data").join("nothing.wav")), None);
        let mut lib = SampleLibrary::new(Some(one.clone()));
        for path in [
            r"%FLStudioFactoryData%\Data\Patches\Packs\Kick.wav",
            r"%flstudiofactorydata%\data\patches\packs\KICK.WAV",
            "%FLStudioFactoryData%/Data/Patches/Packs/Kick.WAV",
            r"%FLSTUDIODATA%\Patches\packs\kick.wav",
        ] {
            assert!(SampleLibrary::has_placeholder(path), "{path}");
            assert!(lib.resolve(path).is_some_and(|p| p.is_file()), "{path}");
        }
        assert!(!SampleLibrary::has_placeholder(&file.to_string_lossy()));
        let asked = asked.to_string_lossy().into_owned();
        assert!(lib.resolve(&asked).is_some_and(|p| p.is_file()), "an absolute path too");
        // A missing file keeps the path as written, for the message that names it.
        assert_eq!(lib.resolve("%FLStudioFactoryData%/Data/x.wav"), Some(one.join("Data").join("x.wav")));
        // Another install folder: the placeholders' samples are looked for there, the other paths stay cached.
        let placeholder = r"%FLStudioFactoryData%\Data\Patches\Packs\Kick.wav";
        let first = lib.get(placeholder).unwrap();
        let direct = lib.get(&asked).unwrap();
        lib.set_factory_dir(Some(two.clone()));
        assert_eq!(lib.resolve(placeholder).and_then(|p| p.parent().map(Path::to_path_buf)), Some(two.join("Data").join("Patches").join("Packs")));
        assert!(Arc::ptr_eq(&lib.get(&asked).unwrap(), &direct), "kept");
        assert!(lib.get(placeholder).is_some());
        assert!(lib.cache.contains_key(placeholder));
        lib.set_factory_dir(None);
        assert!(lib.get(placeholder).is_none() && lib.failures.iter().any(|(k, _)| k == placeholder), "no folder: not found");
        drop(first);
        let _ = std::fs::remove_dir_all(&root);
    }
}
