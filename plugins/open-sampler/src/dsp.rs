//! Open Sampler sound: one-shot sample voices with pitch, gain, the Volume / Pitch envelopes, precomputed effects
//! and a simple low-pass.
//!
// spec: plugins/open-sampler.md (Behaviour), audio/sampler-playback.md (pitch, velocity, pan, resampling),
// spec: audio/channel-envelopes.md (envelope laws)

use std::sync::Arc;

use os_engine::{envelope, laws, Sample};

use crate::params::FxKey;

/// Builds the played sample from the loaded one and the precomputed-effect settings.
pub fn precompute(raw: &Sample, k: FxKey) -> Sample {
    let n = raw.frames();
    let s0 = ((k.start.clamp(0.0, 1.0) * n as f32) as usize).min(n);
    let len = ((k.length.clamp(0.0, 1.0) * (n - s0) as f32).ceil() as usize).clamp(usize::from(n > s0), n - s0);
    let mut l = raw.left[s0..s0 + len].to_vec();
    let mut r = raw.right[s0..s0 + len].to_vec();
    if k.dc && len > 0 {
        for ch in [&mut l, &mut r] {
            let mean = ch.iter().map(|&v| f64::from(v)).sum::<f64>() / len as f64;
            ch.iter_mut().for_each(|v| *v -= mean as f32);
        }
    }
    if k.reverse {
        l.reverse();
        r.reverse();
    }
    if k.polarity {
        l.iter_mut().chain(r.iter_mut()).for_each(|v| *v = -*v);
    }
    if k.swap {
        std::mem::swap(&mut l, &mut r);
    }
    if k.fade_stereo {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let m = 0.5 * (*a + *b);
            (*a, *b) = (m, m);
        }
    }
    let fin = (k.fade_in.clamp(0.0, 1.0) * len as f32) as usize;
    let fout = (k.fade_out.clamp(0.0, 1.0) * len as f32) as usize;
    for i in 0..fin {
        let g = i as f32 / fin as f32;
        l[i] *= g;
        r[i] *= g;
    }
    for i in 0..fout {
        let g = i as f32 / fout as f32;
        l[len - 1 - i] *= g;
        r[len - 1 - i] *= g;
    }
    if k.normalize {
        let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
        if peak > 1e-9 {
            l.iter_mut().chain(r.iter_mut()).for_each(|v| *v /= peak);
        }
    }
    Sample { rate: raw.rate, left: l, right: r }
}

/// Settings read once per block.
#[derive(Clone, Copy)]
pub struct Block {
    pub on: bool,
    pub root: i32,
    /// Fine tune plus the pitch knob, in cents.
    pub cents: f64,
    pub vol_env: Option<envelope::Params>,
    pub pitch_env: Option<envelope::Params>,
    pub start_offset: f32,
    /// Low-pass cutoff 0..1 (1 = open) and resonance 0..1.
    pub cutoff: f32,
    pub resonance: f32,
}

#[derive(Clone, Copy)]
struct Voice {
    key: u8,
    vel: f32,
    pos: f64,
    /// Key rate times the sample / host rate ratio (without pitch envelope and knob).
    key_step: f64,
    age: u64,
    off: Option<u64>,
}

pub struct Engine {
    pub sample: Option<Arc<Sample>>,
    rate: f32,
    voices: Vec<Voice>,
    /// State-variable filter memory per channel (ic1, ic2).
    svf: [(f32, f32); 2],
}

pub const MAX_VOICES: usize = 128;

impl Engine {
    pub fn new(rate: f32) -> Self {
        Engine { sample: None, rate, voices: Vec::with_capacity(MAX_VOICES), svf: [(0.0, 0.0); 2] }
    }

    pub fn set_rate(&mut self, rate: f32) {
        self.rate = rate;
    }

    pub fn reset(&mut self) {
        self.voices.clear();
        self.svf = [(0.0, 0.0); 2];
    }

    /// Keys currently sounding.
    pub fn sounding(&self) -> impl Iterator<Item = u8> + '_ {
        self.voices.iter().map(|v| v.key)
    }

    pub fn note_on(&mut self, key: u8, velocity: f32, b: &Block) {
        let Some(s) = &self.sample else { return };
        if self.voices.len() >= MAX_VOICES {
            self.voices.remove(0);
        }
        let ratio = f64::from(s.rate) / f64::from(self.rate);
        let key_step = 2f64.powf((f64::from(key) - f64::from(b.root)) / 12.0) * ratio;
        // FL velocity byte = plugin velocity × 128; 100 = unity (spec: audio/sampler-playback.md).
        let vel = laws::velocity_gain((velocity.clamp(0.0, 1.0) * 128.0).round() as u8) as f32;
        let pos = (f64::from(b.start_offset) * s.frames() as f64).floor();
        self.voices.push(Voice { key, vel, pos, key_step, age: 0, off: None });
    }

    pub fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.key == key && v.off.is_none()) {
            v.off = Some(v.age);
        }
    }

    /// Adds the voices into `out_l` / `out_r` (frames `0..len`), with per-frame gains from `gain`.
    pub fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32], b: &Block, gains: &[(f32, f32)]) {
        let len = out_l.len();
        let Some(s) = self.sample.clone() else {
            self.voices.clear();
            return;
        };
        let n = s.frames();
        let frame_s = 1.0 / f64::from(self.rate);
        let knob = 2f64.powf(b.cents / 1200.0);
        let mut i = 0;
        while i < self.voices.len() {
            let v = &mut self.voices[i];
            let mut alive = true;
            for f in 0..len {
                if v.pos >= n as f64 {
                    alive = false;
                    break;
                }
                let t = v.age as f64 * frame_s;
                let off = v.off.map(|o| o as f64 * frame_s);
                let env = match &b.vol_env {
                    None => 1.0,
                    Some(p) => match p.level(t, off) {
                        Some(l) => l as f32,
                        None => {
                            alive = false;
                            break;
                        }
                    },
                };
                let mut step = v.key_step * knob;
                if let Some(p) = &b.pitch_env {
                    step *= 2f64.powf(p.level(t, off).unwrap_or(0.0) * p.amount / 1200.0);
                }
                let (l, r) = read(&s, v.pos, step);
                let (gl, gr) = gains[f];
                let g = v.vel * env;
                out_l[f] += l * gl * g;
                out_r[f] += r * gr * g;
                v.pos += step;
                v.age += 1;
            }
            if alive && v.pos < n as f64 {
                i += 1;
            } else {
                self.voices.swap_remove(i);
            }
        }
        if !b.on {
            out_l.fill(0.0);
            out_r.fill(0.0);
        }
        if b.cutoff < 0.999 {
            self.lowpass(out_l, out_r, b);
        }
    }

    /// 2-pole state-variable low-pass (topology-preserving form), 20 Hz..20 kHz on an exponential knob
    /// (OpenStudio approximation of the Fast LP type, not measured).
    fn lowpass(&mut self, out_l: &mut [f32], out_r: &mut [f32], b: &Block) {
        let fc = (20.0 * 1000f32.powf(b.cutoff.clamp(0.0, 1.0))).min(self.rate * 0.45);
        let g = (std::f32::consts::PI * fc / self.rate).tan();
        let k = 2.0 - 1.95 * b.resonance.clamp(0.0, 1.0);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        for (ch, buf) in [out_l, out_r].into_iter().enumerate() {
            let (mut ic1, mut ic2) = self.svf[ch];
            for x in buf.iter_mut() {
                let v3 = *x - ic2;
                let v1 = a1 * ic1 + a2 * v3;
                let v2 = ic2 + a2 * ic1 + a3 * v3;
                ic1 = 2.0 * v1 - ic1;
                ic2 = 2.0 * v2 - ic2;
                *x = v2;
            }
            self.svf[ch] = (ic1, ic2);
        }
    }
}

const SINC_W: f64 = 13.5;

/// One stereo frame at a fractional source position: exact copy at rate 1, else a Blackman-windowed sinc
/// with a 13.5-sample window and the cutoff lowered to the output Nyquist when pitching up.
// spec: audio/sampler-playback.md (resampling)
fn read(s: &Sample, pos: f64, step: f64) -> (f32, f32) {
    let n = s.frames();
    if step == 1.0 && pos.fract() == 0.0 {
        let i = pos as usize;
        return (s.left[i], s.right[i]);
    }
    let scale = step.max(1.0);
    let i0 = (pos - SINC_W).ceil().max(0.0) as usize;
    let i1 = ((pos + SINC_W).floor() as usize).min(n - 1);
    let (mut l, mut r) = (0.0f64, 0.0f64);
    for i in i0..=i1 {
        let t = i as f64 - pos;
        let w = sinc(t / scale) / scale * blackman(t);
        l += f64::from(s.left[i]) * w;
        r += f64::from(s.right[i]) * w;
    }
    (l as f32, r as f32)
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

fn blackman(t: f64) -> f64 {
    if t.abs() >= SINC_W {
        return 0.0;
    }
    let x = std::f64::consts::PI * t / SINC_W;
    0.42 + 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Sample {
        let left: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
        Sample { rate: 48000, right: left.clone(), left }
    }

    fn block() -> Block {
        Block { on: true, root: 60, cents: 0.0, vol_env: None, pitch_env: None, start_offset: 0.0, cutoff: 1.0, resonance: 0.0 }
    }

    fn fx() -> FxKey {
        FxKey { dc: false, normalize: false, reverse: false, polarity: false, fade_stereo: false, swap: false, start: 0.0, length: 1.0, fade_in: 0.0, fade_out: 0.0 }
    }

    #[test]
    fn root_key_plays_the_sample_exactly() {
        let s = ramp(100);
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(s.clone()));
        e.note_on(60, 100.0 / 128.0, &block());
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        e.render(&mut l, &mut r, &block(), &[(1.0, 1.0); 64]);
        assert_eq!(&l[..], &s.left[..64]);
        // The sample ends: the voice is gone after its last frame.
        let (mut l2, mut r2) = (vec![0.0; 64], vec![0.0; 64]);
        e.render(&mut l2, &mut r2, &block(), &[(1.0, 1.0); 64]);
        assert_eq!(l2[35], s.left[99]);
        assert_eq!(l2[36], 0.0);
        assert_eq!(e.sounding().count(), 0);
    }

    #[test]
    fn octave_up_plays_twice_as_fast() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(ramp(1000)));
        e.note_on(72, 100.0 / 128.0, &block());
        let (mut l, mut r) = (vec![0.0; 600], vec![0.0; 600]);
        e.render(&mut l, &mut r, &block(), &[(1.0, 1.0); 600]);
        assert!((l[200] - 0.4).abs() < 0.01, "{}", l[200]);
        assert_eq!(l[550], 0.0);
    }

    #[test]
    fn precomputed_effects() {
        let s = ramp(10);
        let k = FxKey { reverse: true, ..fx() };
        assert_eq!(precompute(&s, k).left[0], 0.9);
        let k = FxKey { normalize: true, ..fx() };
        assert_eq!(precompute(&s, k).left[9], 1.0);
        let k = FxKey { start: 0.5, length: 0.4, ..fx() };
        assert_eq!(precompute(&s, k).left, vec![0.5, 0.6]);
        let k = FxKey { fade_in: 0.5, ..fx() };
        assert_eq!(precompute(&s, k).left[0], 0.0);
    }
}
