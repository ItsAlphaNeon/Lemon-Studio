// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Audio device settings (Settings > Audio): the output device, sample rate and buffer length. They are kept in the
//! user folder (`audio.txt`) and every stream the transport opens uses them; a device that is gone, a rate it does not
//! offer or a buffer length it refuses falls back to the system's default device and that device's own rate and buffer.
// spec: ui/settings.md "Audio"

use std::sync::Mutex;

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{BufferSize, SampleFormat, StreamConfig, SupportedBufferSize};

/// The chosen output; `None` = the device's (or the system's) own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioCfg {
    /// Output device by name.
    pub device: Option<String>,
    /// Sample rate, Hz.
    pub rate: Option<u32>,
    /// Buffer length, frames.
    pub buffer: Option<u32>,
}

/// The buffer length until one is chosen. Linux: 512 frames, since devices' own lengths there are often short enough
/// to pop; elsewhere the device's own.
pub const DEFAULT_BUFFER: Option<u32> = if cfg!(target_os = "linux") { Some(512) } else { None };

impl Default for AudioCfg {
    fn default() -> AudioCfg {
        AudioCfg { device: None, rate: None, buffer: DEFAULT_BUFFER }
    }
}

/// One setting changed from the Audio page's menus (`Cmd::Audio`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Set {
    Device(Option<String>),
    Rate(Option<u32>),
    Buffer(Option<u32>),
}

/// Sample rates the settings offer.
pub const RATES: [u32; 6] = [44100, 48000, 88200, 96000, 176400, 192000];
/// Buffer lengths the settings offer, frames.
pub const BUFFERS: [u32; 8] = [64, 128, 256, 512, 1024, 2048, 4096, 8192];

static CURRENT: Mutex<Option<AudioCfg>> = Mutex::new(None);

impl AudioCfg {
    /// The settings in use: read from `audio.txt` the first time (tests always start from the defaults).
    pub fn current() -> AudioCfg {
        let mut c = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
        c.get_or_insert_with(load).clone()
    }

    /// Streams opened from now on use `self`; it is kept for the next start.
    pub fn apply(self) {
        save(&self);
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(self);
    }

    pub fn with(mut self, s: Set) -> AudioCfg {
        match s {
            Set::Device(d) => self.device = d,
            Set::Rate(r) => self.rate = r,
            Set::Buffer(b) => self.buffer = b,
        }
        self
    }

    /// `key=value` lines: `device`, `rate`, `buffer` (`device`: the device's own); anything else is ignored.
    pub fn parse(text: &str) -> AudioCfg {
        let mut c = AudioCfg::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "device" if !v.is_empty() => c.device = Some(v.to_string()),
                "rate" => c.rate = v.parse().ok().filter(|&r| r >= 8000),
                "buffer" if v == "device" => c.buffer = None,
                "buffer" => c.buffer = v.parse().ok().filter(|&b| b > 0).or(c.buffer),
                _ => {}
            }
        }
        c
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        if let Some(d) = &self.device {
            s += &format!("device={d}\n");
        }
        if let Some(r) = self.rate {
            s += &format!("rate={r}\n");
        }
        match self.buffer {
            Some(b) => s += &format!("buffer={b}\n"),
            // The device's own length, where the default is another (Linux).
            None if DEFAULT_BUFFER.is_some() => s += "buffer=device\n",
            None => {}
        }
        s
    }
}

fn load() -> AudioCfg {
    if cfg!(test) {
        return AudioCfg::default();
    }
    os_paths::user_file("audio.txt").and_then(|f| std::fs::read_to_string(f).ok()).map(|t| AudioCfg::parse(&t)).unwrap_or_default()
}

fn save(c: &AudioCfg) {
    if cfg!(test) {
        return;
    }
    if let Some(dir) = os_paths::user_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("audio.txt"), c.text());
    }
}

/// A device's name as the settings show and keep it.
pub fn device_name(d: &cpal::Device) -> String {
    d.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "(unknown device)".into())
}

/// The output devices' names, and the system default's.
pub fn devices() -> (Vec<String>, Option<String>) {
    let host = cpal::default_host();
    let list = host.output_devices().map(|ds| ds.map(|d| device_name(&d)).collect()).unwrap_or_default();
    (list, host.default_output_device().map(|d| device_name(&d)))
}

/// What a stream opens.
pub struct Output {
    pub device: cpal::Device,
    pub name: String,
    pub format: SampleFormat,
    pub config: StreamConfig,
}

/// The output for `cfg`: its device (else the system default), its rate when the device offers it (else the
/// device's own) and its buffer length when the device takes it (else the device's default).
pub fn output(cfg: &AudioCfg) -> Result<Output, String> {
    let host = cpal::default_host();
    let chosen = cfg.device.as_ref().and_then(|want| host.output_devices().ok()?.find(|d| device_name(d) == *want));
    let device = match chosen {
        Some(d) => d,
        None => host.default_output_device().ok_or("no audio output device")?,
    };
    let own = device.default_output_config().map_err(|e| e.to_string())?;
    let supported = match cfg.rate.filter(|&r| r != own.sample_rate()) {
        Some(r) => device
            .supported_output_configs()
            .ok()
            .and_then(|mut cs| {
                cs.find(|c| c.channels() == own.channels() && c.sample_format() == own.sample_format() && c.min_sample_rate() <= r && r <= c.max_sample_rate())
            })
            .map(|c| c.with_sample_rate(r))
            .unwrap_or(own),
        None => own,
    };
    let format = supported.sample_format();
    let range = supported.buffer_size().clone();
    let mut config: StreamConfig = supported.into();
    if let Some(b) = cfg.buffer {
        let fits = match range {
            // Windows shared mode takes any length (it sizes the device buffer; blocks stay one device period).
            SupportedBufferSize::Range { min, max } => cfg!(windows) || (min..=max).contains(&b),
            SupportedBufferSize::Unknown => true,
        };
        if fits {
            config.buffer_size = BufferSize::Fixed(b);
        }
    }
    Ok(Output { name: device_name(&device), device, format, config })
}

/// The sample rate streams open at now (what hosted plugins are activated at).
pub fn rate() -> Option<u32> {
    output(&AudioCfg::current()).ok().map(|o| o.config.sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_text_round_trips() {
        let c = AudioCfg { device: Some("Speakers (USB Audio)".into()), rate: Some(48000), buffer: Some(256) };
        assert_eq!(AudioCfg::parse(&c.text()), c);
        assert_eq!(AudioCfg::parse(""), AudioCfg::default());
        // Unknown keys and nonsense values are skipped.
        let d = AudioCfg::parse("colour=red\nrate=12\nbuffer=0\ndevice=\n");
        assert_eq!(d, AudioCfg::default());
        assert_eq!(AudioCfg::default().with(Set::Buffer(Some(512))).buffer, Some(512));
        // The device's own buffer length is kept as chosen, also where the default is another.
        let own = AudioCfg::default().with(Set::Buffer(None));
        assert_eq!(AudioCfg::parse(&own.text()), own);
    }

    /// Linux starts at 512 frames (the devices' own lengths popped); elsewhere at the device's own.
    #[test]
    fn default_buffer_length() {
        let want = if cfg!(target_os = "linux") { Some(512) } else { None };
        assert_eq!(AudioCfg::default().buffer, want);
        assert_eq!(AudioCfg::parse("rate=48000\n").buffer, want, "a settings file without a buffer length");
    }
}
