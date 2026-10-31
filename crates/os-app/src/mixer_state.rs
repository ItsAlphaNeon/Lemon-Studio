// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Mixer track states (Lemon Studio's design; spec: ui/mixer-track-commands.md §7): a track's sound, its effects with
//! their settings and its own settings, kept in a file of the user folder's "Mixer states" to put on a track again
//! (File ▸ Open mixer track state ▸) or on a new one (Insert from state ▸). The original keeps its states in its own
//! preset format, which was not looked at; Lemon Studio's are JSON text.

use std::path::{Path, PathBuf};

use os_model::{MixerTrack, PluginCpu, Rgb, SlotPlugin};
use serde_json::{json, Value};

/// The files' extension.
pub const EXT: &str = "lemontrack";

/// What a state holds: the track's name and colour, its levels, switches and EQ, and its slots (plugin, state, switch,
/// mix level, CPU switches).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackState {
    pub name: Option<String>,
    pub color: Option<Rgb>,
    pub volume: Option<f64>,
    pub pan: Option<f64>,
    pub separation: Option<f64>,
    pub polarity: bool,
    pub swap: bool,
    pub slots_enabled: bool,
    pub eq: [[i32; 3]; 3],
    pub slots: [Option<(SlotPlugin, bool, f64, PluginCpu)>; 10],
}

impl TrackState {
    /// Track `m`'s state (its slots' plugin states as `m` holds them).
    pub fn of(m: &MixerTrack) -> TrackState {
        TrackState {
            name: m.name.clone(),
            color: m.color,
            volume: m.volume,
            pan: m.pan,
            separation: m.stereo_separation,
            polarity: m.polarity,
            swap: m.swap,
            slots_enabled: m.slots_enabled,
            eq: m.eq,
            slots: std::array::from_fn(|k| m.slots[k].clone().map(|p| (p, m.slot_on[k], m.slot_mix[k], m.slot_cpu[k]))),
        }
    }

    /// Puts the state on track `m`: its sound, effects and settings; with `named` also its name and colour. Routing,
    /// audio inputs and outputs, docking and the track's place stay.
    pub fn apply(&self, m: &mut MixerTrack, named: bool) {
        if named {
            (m.name, m.color) = (self.name.clone(), self.color);
        }
        (m.volume, m.pan, m.stereo_separation) = (self.volume, self.pan, self.separation);
        (m.polarity, m.swap, m.slots_enabled, m.eq) = (self.polarity, self.swap, self.slots_enabled, self.eq);
        for (k, s) in self.slots.iter().enumerate() {
            let (p, on, mix, cpu) = match s {
                Some((p, on, mix, cpu)) => (Some(SlotPlugin { id: os_model::Id::NONE, ..p.clone() }), *on, *mix, *cpu),
                None => (None, true, 1.0, PluginCpu::default()),
            };
            (m.slots[k], m.slot_on[k], m.slot_mix[k], m.slot_cpu[k]) = (p, on, mix, cpu);
        }
    }

    pub fn to_json(&self) -> String {
        let slots: Vec<Value> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(k, s)| {
                let (p, on, mix, cpu) = s.as_ref()?;
                let hex: String = p.state.iter().map(|b| format!("{b:02x}")).collect();
                Some(json!({"slot": k, "plugin": p.name, "state": hex, "on": on, "mix": mix, "threaded": cpu.threaded, "auto_bypass": cpu.smart_disable}))
            })
            .collect();
        let v = json!({
            "lemon_mixer_track_state": 1,
            "name": self.name,
            "color": self.color.map(|c| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)),
            "volume": self.volume,
            "pan": self.pan,
            "separation": self.separation,
            "polarity": self.polarity,
            "swap": self.swap,
            "slots_enabled": self.slots_enabled,
            "eq": self.eq,
            "slots": slots,
        });
        serde_json::to_string_pretty(&v).unwrap_or_default()
    }

    /// Reads a state written by `to_json`; `None` for anything else.
    pub fn from_json(text: &str) -> Option<TrackState> {
        let v: Value = serde_json::from_str(text).ok()?;
        v.get("lemon_mixer_track_state")?.as_u64()?;
        let num = |k: &str| v.get(k).and_then(Value::as_f64);
        let flag = |k: &str, d: bool| v.get(k).and_then(Value::as_bool).unwrap_or(d);
        let color = v.get("color").and_then(Value::as_str).and_then(|c| {
            let n = u32::from_str_radix(c.strip_prefix('#')?, 16).ok()?;
            Some(Rgb::new((n >> 16) as u8, (n >> 8) as u8, n as u8))
        });
        let mut eq = [[0; 3]; 3];
        for (b, band) in v.get("eq").and_then(Value::as_array).into_iter().flatten().take(3).enumerate() {
            for (i, x) in band.as_array().into_iter().flatten().take(3).enumerate() {
                eq[b][i] = x.as_i64().unwrap_or(0) as i32;
            }
        }
        let mut slots: [Option<(SlotPlugin, bool, f64, PluginCpu)>; 10] = Default::default();
        for s in v.get("slots").and_then(Value::as_array).into_iter().flatten() {
            let k = s.get("slot").and_then(Value::as_u64).filter(|&k| k < 10)? as usize;
            let name = s.get("plugin").and_then(Value::as_str)?.to_owned();
            let hex = s.get("state").and_then(Value::as_str).unwrap_or("");
            let state = (0..hex.len() / 2).map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()).collect::<Option<Vec<u8>>>()?;
            let b = |k: &str, d: bool| s.get(k).and_then(Value::as_bool).unwrap_or(d);
            let mix = s.get("mix").and_then(Value::as_f64).unwrap_or(1.0);
            let cpu = PluginCpu { threaded: b("threaded", true), smart_disable: b("auto_bypass", false) };
            slots[k] = Some((SlotPlugin { id: os_model::Id::NONE, name, state }, b("on", true), mix, cpu));
        }
        Some(TrackState {
            name: v.get("name").and_then(Value::as_str).map(str::to_owned),
            color,
            volume: num("volume"),
            pan: num("pan"),
            separation: num("separation"),
            polarity: flag("polarity", false),
            swap: flag("swap", false),
            slots_enabled: flag("slots_enabled", true),
            eq,
            slots,
        })
    }
}

/// The user folder's "Mixer states" (tests: a temporary folder of the test's own thread).
pub fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        let thread = format!("{:?}", std::thread::current().id()).chars().filter(char::is_ascii_digit).collect::<String>();
        return Some(std::env::temp_dir().join(format!("lemon-mixer-states-{}-{thread}", std::process::id())));
    }
    os_paths::user_dir().map(|d| d.join("Mixer states"))
}

/// The saved states, by name.
pub fn list() -> Vec<(String, PathBuf)> {
    let Some(d) = dir() else { return Vec::new() };
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXT)))
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), p)))
        .collect();
    v.sort_by_key(|(n, _)| n.to_lowercase());
    v
}

/// Writes `st` as "<name>.lemontrack", with a number after the name when one of that name exists.
pub fn save(st: &TrackState, name: &str) -> Result<PathBuf, String> {
    let d = dir().ok_or("no user folder")?;
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let clean: String = name.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c }).collect();
    let free = |p: &Path| !p.exists();
    let path = (1..).map(|i| d.join(if i == 1 { format!("{clean}.{EXT}") } else { format!("{clean} {i}.{EXT}") })).find(|p| free(p)).unwrap();
    std::fs::write(&path, st.to_json()).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Reads the state in `path`.
pub fn load(path: &Path) -> Option<TrackState> {
    TrackState::from_json(&std::fs::read_to_string(path).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A track's state goes through the file unchanged.
    #[test]
    fn a_state_round_trips() {
        let mut m = MixerTrack::new(3);
        (m.name, m.color, m.volume, m.pan, m.polarity) = (Some("Drums".into()), Some(Rgb::new(0xc8, 0x50, 0x4a)), Some(0.5), Some(-0.25), true);
        m.eq[1] = [100, 2000, 30];
        m.slots[2] = Some(SlotPlugin { id: os_model::Id::NONE, name: "Fruity Limiter".into(), state: vec![0, 1, 0xfe, 0xff] });
        (m.slot_on[2], m.slot_mix[2], m.slot_cpu[2].smart_disable) = (false, 0.75, true);
        let st = TrackState::of(&m);
        assert_eq!(TrackState::from_json(&st.to_json()), Some(st.clone()));
        assert_eq!(TrackState::from_json("{}"), None);
        let mut other = MixerTrack::new(5);
        st.apply(&mut other, false);
        assert_eq!((other.name, other.volume, other.slot_mix[2], other.slot_cpu[2].smart_disable), (None, Some(0.5), 0.75, true));
        assert_eq!(other.slots[2].as_ref().map(|p| p.state.clone()), Some(vec![0, 1, 0xfe, 0xff]));
        assert_eq!(other.index, 5, "the track keeps its place");
    }
}
