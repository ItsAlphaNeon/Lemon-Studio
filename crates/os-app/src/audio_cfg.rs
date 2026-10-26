// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Audio device settings (Settings > Audio): the output device, sample rate and buffer length. They are kept in the
//! user folder (`audio.txt`) and every stream the transport opens uses them; a device that is gone, a rate it does not
//! offer or a buffer length it refuses falls back to the system's default device and that device's own rate and buffer.
//! The device is a Windows (ALSA, Core Audio) one or, on Windows, an ASIO driver (`asio`): for a driver, `None` and a
//! rate or buffer length it does not take mean the driver's own (its buffer length is set in its own panel). A Windows
//! device plays in shared mode, or in exclusive mode (`exclusive`, `device::wasapi`) with the device to itself. On
//! Linux shared mode plays through the sound server (PulseAudio, or PipeWire through pipewire-pulse) as other programs
//! do, and exclusive mode opens an ALSA device directly, the sound server's hold on it or not (`host`).
// spec: ui/settings.md "Audio", audio/wasapi-low-latency.md

use std::sync::Mutex;

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{BufferSize, SampleFormat, StreamConfig, SupportedBufferSize};

/// The chosen output; `None` = the device's (or the system's) own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioCfg {
    /// `device` is an ASIO driver (`device::asio_drivers`), not a Windows device.
    pub asio: bool,
    /// Output device by name.
    pub device: Option<String>,
    /// Sample rate, Hz.
    pub rate: Option<u32>,
    /// Buffer length, frames (`SMALLEST`: the device's smallest period, in exclusive mode).
    pub buffer: Option<u32>,
    /// A Windows device in exclusive mode (Lemon Studio alone on it), on Linux an ALSA device opened directly; ignored
    /// for an ASIO driver and on macOS.
    pub exclusive: bool,
}

/// `AudioCfg::buffer` for "Smallest": the shortest period the device takes in exclusive mode, at whatever rate it opens
/// (elsewhere the device's own length).
pub const SMALLEST: u32 = 0;

/// The buffer length until one is chosen. Linux: 512 frames, since devices' own lengths there are often short enough
/// to pop; elsewhere the device's own.
pub const DEFAULT_BUFFER: Option<u32> = if cfg!(target_os = "linux") { Some(512) } else { None };

impl Default for AudioCfg {
    fn default() -> AudioCfg {
        AudioCfg { asio: false, device: None, rate: None, buffer: DEFAULT_BUFFER, exclusive: false }
    }
}

/// One setting changed from the Audio page's menus (`Cmd::Audio`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Set {
    /// A Windows device (`None`: the system's default).
    Device(Option<String>),
    /// An ASIO driver.
    Asio(String),
    Rate(Option<u32>),
    Buffer(Option<u32>),
    /// Exclusive (`true`) or shared mode for Windows devices (Linux: ALSA, or the sound server).
    Mode(bool),
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
            Set::Device(d) => (self.asio, self.device) = (false, d),
            Set::Asio(d) => (self.asio, self.device) = (true, Some(d)),
            Set::Rate(r) => self.rate = r,
            Set::Buffer(b) => self.buffer = b,
            Set::Mode(on) => {
                // Linux: the sound server's devices are not ALSA's; the new mode starts on its default.
                if cfg!(target_os = "linux") && on != self.exclusive {
                    self.device = None;
                }
                self.exclusive = on;
            }
        }
        self
    }

    /// A Windows device chosen in exclusive mode, or on Linux an ALSA device (not an ASIO driver; never on macOS).
    pub fn exclusive(&self) -> bool {
        self.exclusive && !self.asio && (cfg!(windows) || cfg!(target_os = "linux"))
    }

    /// The ASIO driver chosen (Windows; elsewhere the name is looked for among the devices and not found).
    pub fn asio_driver(&self) -> Option<&str> {
        self.device.as_deref().filter(|_| self.asio && cfg!(windows))
    }

    /// `key=value` lines: `host` (`asio`: the device is an ASIO driver), `mode` (`exclusive`; else shared), `device`,
    /// `rate`, `buffer` (`device`: the device's own; `smallest`: `SMALLEST`); anything else is ignored.
    pub fn parse(text: &str) -> AudioCfg {
        let mut c = AudioCfg::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "host" => c.asio = v.eq_ignore_ascii_case("asio"),
                "mode" => c.exclusive = v.eq_ignore_ascii_case("exclusive"),
                "device" if !v.is_empty() => c.device = Some(v.to_string()),
                "rate" => c.rate = v.parse().ok().filter(|&r| r >= 8000),
                "buffer" if v == "device" => c.buffer = None,
                "buffer" if v == "smallest" => c.buffer = Some(SMALLEST),
                "buffer" => c.buffer = v.parse().ok().filter(|&b| b > 0).or(c.buffer),
                _ => {}
            }
        }
        c
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        if self.asio {
            s += "host=asio\n";
        }
        if self.exclusive {
            s += "mode=exclusive\n";
        }
        if let Some(d) = &self.device {
            s += &format!("device={d}\n");
        }
        if let Some(r) = self.rate {
            s += &format!("rate={r}\n");
        }
        match self.buffer {
            Some(SMALLEST) => s += "buffer=smallest\n",
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

/// The audio system `cfg` plays on. Linux: in shared mode the sound server (cpal's PulseAudio host, which PipeWire
/// serves too), else ALSA (also when no sound server is running); elsewhere the system's own.
pub fn host(cfg: &AudioCfg) -> cpal::Host {
    #[cfg(target_os = "linux")]
    {
        if server(cfg) {
            if let Ok(h) = cpal::host_from_id(cpal::HostId::PulseAudio) {
                return h;
            }
        }
        if let Ok(h) = cpal::host_from_id(cpal::HostId::Alsa) {
            return h;
        }
    }
    cpal::default_host()
}

/// Linux, shared mode, with a sound server to play through (it is not asked: only whether its socket is there).
#[cfg(target_os = "linux")]
fn server(cfg: &AudioCfg) -> bool {
    !cfg.exclusive() && cpal::available_hosts().contains(&cpal::HostId::PulseAudio)
}

/// The name of `host(cfg)`, without connecting to it.
pub fn host_name(cfg: &AudioCfg) -> &'static str {
    #[cfg(target_os = "linux")]
    return if server(cfg) { cpal::HostId::PulseAudio.name() } else { cpal::HostId::Alsa.name() };
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cfg;
        cpal::default_host().id().name()
    }
}

/// A device's name as the settings show and keep it. ALSA names one card's many PCMs alike (`default:`, `front:`,
/// `hw:`, `plughw:`, ...): there the PCM follows, "HDA Intel PCH, ALC257 Analog (hw:CARD=0,DEV=0)".
pub fn device_name(d: &cpal::Device) -> String {
    let name = plain_name(d);
    match alsa_pcm(d) {
        Some(pcm) if pcm != "default" => named(&name, &pcm),
        _ => name,
    }
}

/// The device's own description, as names were kept before ALSA's had their PCM (a name saved then still finds its
/// device: `find`).
fn plain_name(d: &cpal::Device) -> String {
    d.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "(unknown device)".into())
}

fn named(name: &str, pcm: &str) -> String {
    format!("{name} ({pcm})")
}

/// An ALSA device's PCM name ("hw:CARD=0,DEV=0"); `None` on another host.
fn alsa_pcm(d: &cpal::Device) -> Option<String> {
    let id = d.id().ok()?;
    (id.host().name() == "ALSA").then(|| id.id().to_string())
}

/// ALSA PCMs that are no device to play on or record from: plugins that only convert (rates, channels, Speex's
/// processing), throw the sound away (`null`) or stream raw USB (`usbstream`).
const NOT_DEVICES: [&str; 9] = ["null", "lavrate", "samplerate", "speexrate", "speex", "upmix", "vdownmix", "usbstream", "a52"];

/// A device the settings list (every one off ALSA).
pub fn listed(d: &cpal::Device) -> bool {
    alsa_pcm(d).is_none_or(|pcm| !NOT_DEVICES.contains(&pcm.split(':').next().unwrap_or("")))
}

/// The device named `want` among `devices`: by its name, else (a name saved before ALSA's had their PCM) the first
/// whose own description it is.
pub fn find(devices: impl Iterator<Item = cpal::Device>, want: &str) -> Option<cpal::Device> {
    let devices: Vec<cpal::Device> = devices.collect();
    let i = devices.iter().position(|d| device_name(d) == want).or_else(|| devices.iter().position(|d| plain_name(d) == want))?;
    devices.into_iter().nth(i)
}

/// The output devices' names for `cfg` (on Linux the sound server's or ALSA's, by mode), and the default's.
pub fn devices(cfg: &AudioCfg) -> (Vec<String>, Option<String>) {
    let host = host(cfg);
    let list = host.output_devices().map(|ds| ds.filter(listed).map(|d| device_name(&d)).collect()).unwrap_or_default();
    (list, host.default_output_device().map(|d| device_name(&d)))
}

/// What a stream opens.
pub struct Output {
    /// The audio system the device is on (cpal's name for it).
    pub host: &'static str,
    pub device: cpal::Device,
    pub name: String,
    pub format: SampleFormat,
    pub config: StreamConfig,
    /// The buffer lengths the device says it takes, frames.
    pub buffers: Option<(u32, u32)>,
}

/// The output for `cfg`: its device (else the system default), its rate when the device offers it (else the
/// device's own) and its buffer length when the device takes it (else the device's default).
pub fn output(cfg: &AudioCfg) -> Result<Output, String> {
    let host = host(cfg);
    let chosen = cfg.device.as_ref().and_then(|want| find(host.output_devices().ok()?, want));
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
    let buffers = match range {
        SupportedBufferSize::Range { min, max } => Some((min, max)),
        SupportedBufferSize::Unknown => None,
    };
    let mut config: StreamConfig = supported.into();
    #[cfg(target_os = "linux")]
    let through_server = server(cfg);
    #[cfg(not(target_os = "linux"))]
    let through_server = false;
    if let Some(b) = wanted_buffer(cfg.buffer, through_server) {
        let fits = match range {
            // Windows shared mode takes any length (it sizes the device buffer; blocks stay one device period).
            SupportedBufferSize::Range { min, max } => cfg!(windows) || (min..=max).contains(&b),
            SupportedBufferSize::Unknown => true,
        };
        if fits {
            config.buffer_size = BufferSize::Fixed(b);
        }
    }
    Ok(Output { host: host.id().name(), name: device_name(&device), device, format, config, buffers })
}

/// The buffer length a stream asks for, `chosen` being the setting. "Smallest" is exclusive mode's; elsewhere it is the
/// device's own, as is no length. Through the sound server (Linux shared mode) a stream always asks for one: asking
/// for none gets the server's default buffering, about two seconds (PulseAudio's default target length, which
/// pipewire-pulse keeps), so the device's own is `DEFAULT_BUFFER` there.
fn wanted_buffer(chosen: Option<u32>, through_server: bool) -> Option<u32> {
    chosen.filter(|&b| b != SMALLEST).or(if through_server { DEFAULT_BUFFER.or(Some(512)) } else { None })
}

/// The sample rate the settings' device would open at now, asking the device (`device::rate` knows it once the device
/// opened). An ASIO driver is not asked (that would load it): the rate chosen, else unknown until it opens. A Windows
/// device in exclusive mode runs at the rate chosen when it takes it, else (and until one is chosen) at its own.
pub fn rate() -> Option<u32> {
    let cfg = AudioCfg::current();
    if cfg.asio_driver().is_some() {
        return cfg.rate;
    }
    if let Some(r) = cfg.rate.filter(|_| cfg!(windows) && cfg.exclusive()) {
        return Some(r);
    }
    output(&cfg).ok().map(|o| o.config.sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream through the sound server never asks for no length (the server would buffer about two seconds); elsewhere
    /// "Smallest" and the device's own ask for none, and a chosen length is asked for.
    #[test]
    fn a_stream_through_the_sound_server_asks_for_a_length() {
        let default = DEFAULT_BUFFER.or(Some(512));
        assert_eq!(wanted_buffer(None, true), default);
        assert_eq!(wanted_buffer(Some(SMALLEST), true), default);
        assert_eq!(wanted_buffer(Some(256), true), Some(256));
        assert_eq!(wanted_buffer(None, false), None);
        assert_eq!(wanted_buffer(Some(SMALLEST), false), None);
        assert_eq!(wanted_buffer(Some(256), false), Some(256));
    }

    #[test]
    fn settings_text_round_trips() {
        let c = AudioCfg { asio: false, device: Some("Speakers (USB Audio)".into()), rate: Some(48000), buffer: Some(256), exclusive: false };
        assert_eq!(AudioCfg::parse(&c.text()), c);
        // Exclusive mode and its smallest period; a file without `mode` is shared.
        let x = c.clone().with(Set::Mode(true)).with(Set::Device(c.device.clone())).with(Set::Buffer(Some(SMALLEST)));
        assert_eq!(x.text(), "mode=exclusive\ndevice=Speakers (USB Audio)\nrate=48000\nbuffer=smallest\n");
        assert_eq!(AudioCfg::parse(&x.text()), x);
        assert_eq!(x.exclusive(), cfg!(windows) || cfg!(target_os = "linux"));
        assert!(!AudioCfg::parse("mode=shared\n").exclusive);
        assert!(!x.clone().with(Set::Asio("FL Studio ASIO".into())).exclusive(), "not for an ASIO driver");
        // An ASIO driver; a file from before there were drivers (no `host`) is a Windows device.
        let a = AudioCfg::default().with(Set::Asio("FL Studio ASIO".into()));
        assert!(a.text().starts_with("host=asio\ndevice=FL Studio ASIO\n"), "{}", a.text());
        assert_eq!(AudioCfg::parse(&a.text()), a);
        assert!(!AudioCfg::parse("device=FL Studio ASIO\n").asio);
        assert_eq!(a.clone().with(Set::Device(None)), AudioCfg::default(), "a Windows device again");
        assert_eq!(a.asio_driver(), cfg!(windows).then_some("FL Studio ASIO"));
        assert_eq!(AudioCfg::parse(""), AudioCfg::default());
        // Unknown keys and nonsense values are skipped.
        let d = AudioCfg::parse("colour=red\nrate=12\nbuffer=0\ndevice=\n");
        assert_eq!(d, AudioCfg::default());
        assert_eq!(AudioCfg::default().with(Set::Buffer(Some(512))).buffer, Some(512));
        // The device's own buffer length is kept as chosen, also where the default is another.
        let own = AudioCfg::default().with(Set::Buffer(None));
        assert_eq!(AudioCfg::parse(&own.text()), own);
    }

    /// On Linux the sound server's devices and ALSA's are not the same: changing mode starts on the default device
    /// (elsewhere the device stays). Shared mode is the default, on the sound server when one runs.
    #[test]
    fn changing_mode_keeps_the_device_where_it_is_the_same() {
        let c = AudioCfg { device: Some("Speakers".into()), ..AudioCfg::default() };
        let x = c.clone().with(Set::Mode(true));
        assert!(x.exclusive);
        assert_eq!(x.device.is_none(), cfg!(target_os = "linux"));
        assert_eq!(c.clone().with(Set::Mode(false)).device, c.device, "the same mode again");
        assert!(!AudioCfg::default().exclusive);
        #[cfg(target_os = "linux")]
        assert_eq!(host_name(&x), "ALSA");
    }

    /// ALSA's PCMs of one card are told apart by their PCM; converting plugins are not listed.
    #[test]
    fn alsa_names_say_the_pcm() {
        let name = "HDA Intel PCH, ALC257 Analog";
        assert_eq!(named(name, "hw:CARD=0,DEV=0"), "HDA Intel PCH, ALC257 Analog (hw:CARD=0,DEV=0)");
        assert_ne!(named(name, "hw:CARD=0,DEV=0"), named(name, "front:CARD=PCH,DEV=0"));
        assert!(NOT_DEVICES.contains(&"usbstream:CARD=PCH".split(':').next().unwrap()));
    }

    /// The ALSA devices here have a name each, and a name saved before they had their PCM still finds a device. Needs
    /// an audio card.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn alsa_devices_here_are_told_apart() {
        use cpal::traits::HostTrait;
        let alsa = cpal::host_from_id(cpal::HostId::Alsa).expect("ALSA");
        let devices: Vec<cpal::Device> = alsa.output_devices().expect("devices").filter(listed).collect();
        let names: Vec<String> = devices.iter().map(device_name).collect();
        println!("{names:#?}");
        let unique: std::collections::HashSet<&String> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "no two alike");
        assert!(!names.iter().any(|n| n.ends_with("(null)") || n.contains("(samplerate)")), "no converting plugins");
        let card = devices.iter().find(|d| alsa_pcm(d).is_some_and(|p| p.starts_with("hw:"))).expect("a card");
        let found = find(alsa.output_devices().expect("devices"), &plain_name(card)).expect("an old name finds a device");
        assert_eq!(plain_name(&found), plain_name(card));
        let exact = find(alsa.output_devices().expect("devices"), &device_name(card)).expect("its name finds it");
        assert_eq!(alsa_pcm(&exact), alsa_pcm(card));
    }

    /// Linux starts at 512 frames (the devices' own lengths popped); elsewhere at the device's own.
    #[test]
    fn default_buffer_length() {
        let want = if cfg!(target_os = "linux") { Some(512) } else { None };
        assert_eq!(AudioCfg::default().buffer, want);
        assert_eq!(AudioCfg::parse("rate=48000\n").buffer, want, "a settings file without a buffer length");
    }
}
