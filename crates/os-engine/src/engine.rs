//! Block-based playback engine: schedules notes from patterns / playlist clips, plays sampler
//! and audio-clip channels, and sums them through the mixer routing graph.
//!
// spec: audio/sampler-playback.md, audio/gain-laws.md, flp/events/playlist.md (clips),
//       flp/events/patterns.md (notes), flp/events/mixer.md (routing)

use std::sync::Arc;

use os_model::{ChannelKind, ClipSource, Project};

use crate::laws;
use crate::sample::{Sample, SampleLibrary};

/// What to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    /// The first arrangement's playlist. Falls back to the current pattern if it has no clips.
    Song,
    /// One pattern (1-based), from its start.
    Pattern(u16),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Trigger {
    frame: u64,
    channel: usize,
    rate: f64,
    /// Output frame at which the voice is cut (end of an audio clip), if any.
    cut: Option<u64>,
}

#[derive(Debug, Clone)]
struct ChannelState {
    sample: Option<Arc<Sample>>,
    gain: (f32, f32),
    insert: usize,
    /// Sample-rate conversion × channel pitch.
    base_rate: f64,
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    channel: usize,
    pos: f64,
    step: f64,
    /// Output frames left before the voice is cut; `None` = play the whole sample.
    remaining: Option<u64>,
}

#[derive(Debug, Clone)]
struct MixTrack {
    /// Fader × pan as `[[L←L, L←R], [R←L, R←R]]`.
    matrix: [[f32; 2]; 2],
    /// (destination, send gain). Sidechain-only routes (level 0) are dropped.
    routes: Vec<(usize, f32)>,
}

/// Things the engine skipped, so callers can report them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EngineReport {
    pub plugin_channels_silent: usize,
    pub automation_clips_ignored: usize,
    pub effects_bypassed: usize,
    pub missing_samples: Vec<String>,
}

pub struct Engine {
    rate: u32,
    channels: Vec<ChannelState>,
    tracks: Vec<MixTrack>,
    /// Mixer tracks in processing order: every track before the tracks it sends to.
    order: Vec<usize>,
    triggers: Vec<Trigger>,
    next_trigger: usize,
    voices: Vec<Voice>,
    frame: u64,
    /// Frame where the scheduled material ends (voices may ring past it).
    end_frame: u64,
    bus: Vec<Vec<f32>>,
    pub report: EngineReport,
}

impl Engine {
    pub fn new(project: &Project, library: &mut SampleLibrary, rate: u32, mode: PlayMode) -> Self {
        let mut report = EngineReport::default();
        let tempo = project.tempo.unwrap_or(140.0);
        let ppq = project.ppq.max(1);
        let n_tracks = project.mixer.len().max(1);

        let channels: Vec<ChannelState> = project
            .channels
            .iter()
            .map(|ch| {
                let plays_sample = matches!(ch.kind, ChannelKind::Sampler | ChannelKind::AudioClip);
                if ch.kind == ChannelKind::Plugin {
                    report.plugin_channels_silent += 1;
                }
                let sample = if plays_sample {
                    ch.sample_path.as_deref().and_then(|p| {
                        let s = library.get(p);
                        if s.is_none() {
                            report.missing_samples.push(p.to_string());
                        }
                        s
                    })
                } else {
                    None
                };
                let g = if ch.enabled { laws::channel_gain(ch.volume) } else { 0.0 };
                let (pl, pr) = laws::channel_pan(ch.pan);
                let base_rate = sample.as_ref().map_or(1.0, |s| f64::from(s.rate) / f64::from(rate))
                    * laws::playback_rate(60, ch.pitch_cents);
                let insert = usize::from(ch.mixer_track).min(n_tracks - 1);
                ChannelState { sample, gain: ((g * pl) as f32, (g * pr) as f32), insert, base_rate }
            })
            .collect();

        let tracks: Vec<MixTrack> = if project.mixer.is_empty() {
            vec![MixTrack { matrix: [[1.0, 0.0], [0.0, 1.0]], routes: vec![] }]
        } else {
            project
                .mixer
                .iter()
                .map(|m| {
                    report.effects_bypassed += m.slots.iter().flatten().count();
                    let g = if m.enabled { m.volume.map_or(1.0, laws::fader_gain) } else { 0.0 };
                    let pan = laws::mixer_pan(m.pan.unwrap_or(0.0));
                    let matrix = pan.map(|row| row.map(|x| (x * g) as f32));
                    let routes = if m.index == 0 {
                        vec![]
                    } else {
                        m.routes
                            .iter()
                            .filter(|&&r| usize::from(r) < n_tracks && r != m.index)
                            .map(|&r| {
                                let level = m.send_levels.iter().find(|(d, _)| *d == r).map(|&(_, l)| l);
                                (usize::from(r), level.map_or(1.0, laws::fader_gain) as f32)
                            })
                            .filter(|&(_, g)| g > 0.0)
                            .collect()
                    };
                    MixTrack { matrix, routes }
                })
                .collect()
        };
        let order = processing_order(&tracks);

        let (mut triggers, end_tick) = schedule(project, mode, &mut report);
        triggers.sort_by(|a, b| a.0.total_cmp(&b.0));
        let triggers = triggers
            .into_iter()
            .filter_map(|(tick, ch, key, cut)| {
                let state = channels.get(ch)?;
                state.sample.as_ref()?;
                Some(Trigger {
                    frame: laws::tick_to_frame(tick, ppq, tempo, rate),
                    channel: ch,
                    rate: state.base_rate * laws::playback_rate(key, 0),
                    cut: cut.map(|c: f64| laws::tick_to_frame(c, ppq, tempo, rate)),
                })
            })
            .collect();

        Engine {
            rate,
            channels,
            bus: vec![Vec::new(); tracks.len()],
            tracks,
            order,
            triggers,
            next_trigger: 0,
            voices: Vec::new(),
            frame: 0,
            end_frame: laws::tick_to_frame(end_tick, ppq, tempo, rate),
            report,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// True once all scheduled material has played and every voice has finished.
    pub fn finished(&self) -> bool {
        self.frame >= self.end_frame && self.next_trigger >= self.triggers.len() && self.voices.is_empty()
    }

    /// Renders the next `out.len() / 2` stereo frames (interleaved L/R) into `out`.
    pub fn process(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        for b in &mut self.bus {
            b.clear();
            b.resize(frames * 2, 0.0);
        }
        let block_end = self.frame + frames as u64;

        // Continue voices started in earlier blocks.
        let channels = &self.channels;
        let bus = &mut self.bus;
        self.voices.retain_mut(|v| render_voice(v, channels, bus, 0, frames));
        // Start voices whose trigger falls in this block, each from its exact offset.
        while let Some(t) = self.triggers.get(self.next_trigger).copied() {
            if t.frame >= block_end {
                break;
            }
            self.next_trigger += 1;
            let offset = t.frame.saturating_sub(self.frame) as usize;
            let remaining = t.cut.map(|c| c.saturating_sub(t.frame));
            let mut v = Voice { channel: t.channel, pos: 0.0, step: t.rate, remaining };
            if render_voice(&mut v, &self.channels, &mut self.bus, offset, frames) {
                self.voices.push(v);
            }
        }

        // Mixer: apply each track's gain/pan, then add into its destinations.
        for &t in &self.order {
            let [[ll, lr], [rl, rr]] = self.tracks[t].matrix;
            let mut buf = std::mem::take(&mut self.bus[t]);
            for f in buf.chunks_exact_mut(2) {
                let (l, r) = (f[0], f[1]);
                f[0] = ll * l + lr * r;
                f[1] = rl * l + rr * r;
            }
            for &(dest, g) in &self.tracks[t].routes {
                for (d, s) in self.bus[dest].iter_mut().zip(&buf) {
                    *d += *s * g;
                }
            }
            self.bus[t] = buf;
        }
        out.copy_from_slice(&self.bus[0][..frames * 2]);
        self.frame = block_end;
    }

    /// Renders everything to an interleaved stereo buffer.
    pub fn render_all(&mut self, block: usize) -> Vec<f32> {
        let mut out = Vec::new();
        let mut buf = vec![0.0f32; block * 2];
        while !self.finished() {
            self.process(&mut buf);
            out.extend_from_slice(&buf);
        }
        // Trim to the last non-silent frame, but never shorter than the scheduled length.
        let min_len = (self.end_frame as usize) * 2;
        let last = out.iter().rposition(|s| s.abs() > 0.0).map_or(0, |i| (i / 2 + 1) * 2);
        out.truncate(last.max(min_len).min(out.len()));
        out
    }
}

/// Adds one voice into its channel's insert bus from `offset` to the block end.
/// Returns false when the voice has finished.
fn render_voice(v: &mut Voice, channels: &[ChannelState], bus: &mut [Vec<f32>], offset: usize, frames: usize) -> bool {
    let ch = &channels[v.channel];
    let Some(s) = ch.sample.as_ref() else { return false };
    let n = s.frames();
    let (gl, gr) = ch.gain;
    let out = &mut bus[ch.insert];
    let mut pos = v.pos;
    for f in offset..frames {
        let i = pos as usize;
        if v.remaining == Some(0) {
            return false;
        }
        if let Some(r) = v.remaining.as_mut() {
            *r -= 1;
        }
        if i >= n {
            v.pos = pos;
            return false;
        }
        let frac = (pos - i as f64) as f32;
        let (l, r) = if i + 1 < n {
            (
                s.left[i] + (s.left[i + 1] - s.left[i]) * frac,
                s.right[i] + (s.right[i + 1] - s.right[i]) * frac,
            )
        } else {
            (s.left[i], s.right[i])
        };
        out[f * 2] += l * gl;
        out[f * 2 + 1] += r * gr;
        pos += v.step;
    }
    v.pos = pos;
    (pos as usize) < n
}

/// Orders tracks so that every track is processed before any track it routes to.
fn processing_order(tracks: &[MixTrack]) -> Vec<usize> {
    fn visit(t: usize, tracks: &[MixTrack], state: &mut [u8], order: &mut Vec<usize>) {
        if state[t] != 0 {
            return; // done, or a cycle (ignored)
        }
        state[t] = 1;
        for &(d, _) in &tracks[t].routes {
            visit(d, tracks, state, order);
        }
        state[t] = 2;
        order.push(t);
    }
    let mut state = vec![0u8; tracks.len()];
    let mut order = Vec::with_capacity(tracks.len());
    for t in 0..tracks.len() {
        visit(t, tracks, &mut state, &mut order);
    }
    order.reverse(); // sources first, destinations (Master) last
    order
}

/// A scheduled note: (start tick, channel, key, optional cut tick).
type Sched = (f64, usize, u16, Option<f64>);

/// Note triggers plus the end tick of the scheduled material.
fn schedule(project: &Project, mode: PlayMode, report: &mut EngineReport) -> (Vec<Sched>, f64) {
    let bar = f64::from(project.ppq.max(1)) * 4.0;
    let pattern_notes = |n: u16| project.patterns.iter().find(|p| p.number == n).map(|p| p.notes.as_slice()).unwrap_or(&[]);
    let pattern_end = |n: u16| {
        let last = pattern_notes(n).iter().map(|x| f64::from(x.position) + f64::from(x.length)).fold(0.0, f64::max);
        ((last / bar).floor() + 1.0).max(1.0) * bar
    };
    let one_pattern = |n: u16| {
        let t: Vec<Sched> = pattern_notes(n).iter().map(|x| (f64::from(x.position), usize::from(x.channel), x.key, None)).collect();
        (t, pattern_end(n))
    };
    match mode {
        PlayMode::Pattern(n) => one_pattern(n),
        PlayMode::Song => {
            let clips = project.arrangements.first().map(|a| a.clips.as_slice()).unwrap_or(&[]);
            if clips.is_empty() {
                return one_pattern(project.current_pattern.unwrap_or(1));
            }
            let mut out = Vec::new();
            let mut end: f64 = 0.0;
            for c in clips {
                let start = f64::from(c.position);
                let len = f64::from(c.length);
                end = end.max(start + len);
                match c.source {
                    ClipSource::Pattern(n) => {
                        for note in pattern_notes(n) {
                            if f64::from(note.position) < len {
                                out.push((start + f64::from(note.position), usize::from(note.channel), note.key, None));
                            }
                        }
                    }
                    ClipSource::Channel(ch) => match project.channels.get(usize::from(ch)).map(|c| c.kind) {
                        // An audio clip plays its channel's sample from the start, cut at the clip end.
                        Some(ChannelKind::AudioClip | ChannelKind::Sampler) => {
                            out.push((start, usize::from(ch), 60, Some(start + len)));
                        }
                        _ => report.automation_clips_ignored += 1,
                    },
                }
            }
            (out, end)
        }
    }
}
