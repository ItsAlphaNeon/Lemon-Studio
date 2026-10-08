//! Sample loading: decode an audio file to stereo `f32` frames.
//!
// spec: flp/sample-paths.md (placeholders, formats)

use std::collections::HashMap;
use std::fs::File;
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

/// Decodes a WAV / FLAC / Ogg Vorbis / WavPack file.
pub fn load(path: &Path) -> Result<Sample, SampleError> {
    let decode_err = |message: String| SampleError::Decode { path: path.to_owned(), message };
    let file = File::open(path).map_err(|source| SampleError::Io { path: path.to_owned(), source })?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| decode_err(e.to_string()))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| decode_err("no audio track".into()))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| decode_err("no audio codec parameters".into()))?;
    let mut decoder = codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| decode_err(e.to_string()))?;

    let mut rate = 0;
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let mut buf: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(decode_err(e.to_string())),
        };
        if packet.track_id != track_id {
            continue;
        }
        let audio = match decoder.decode(&packet) {
            Ok(a) => a,
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(decode_err(e.to_string())),
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
        return Err(decode_err("no audio decoded".into()));
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
    /// Paths that couldn't be resolved or decoded, with the reason.
    pub failures: Vec<(String, String)>,
}

impl SampleLibrary {
    pub fn new(factory_dir: Option<PathBuf>) -> Self {
        SampleLibrary { factory_dir, ..Default::default() }
    }

    /// The default install location, if it exists on this machine.
    pub fn default_factory_dir() -> Option<PathBuf> {
        let p = PathBuf::from(r"C:\Program Files\Image-Line\FL Studio 2026");
        p.is_dir().then_some(p)
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
            Some(PathBuf::from(project_path))
        }
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
            Some(p) => match load(&p) {
                Ok(s) => Some(Arc::new(s)),
                Err(e) => {
                    self.failures.push((project_path.into(), e.to_string()));
                    None
                }
            },
        };
        self.cache.insert(project_path.into(), loaded.clone());
        loaded
    }
}
