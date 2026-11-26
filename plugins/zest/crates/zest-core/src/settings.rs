// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The flat settings record: the parameter values read once when they change (`pages/index.md`, Parameters: when they apply).
//! Stepped values are rounded, ties away from zero; a switch is on at >= 0.5; a modulation source is rounded and clamped.

use crate::params::{id, COUNT};

#[inline]
fn step(x: f32) -> i32 {
    x.round() as i32
}

#[inline]
fn on(x: f32) -> bool {
    x >= 0.5
}

#[inline]
fn source(x: f32) -> usize {
    step(x).clamp(0, 23) as usize
}

const PB_TABLE: [f32; 28] =
    [0., 1., 2., 3., 4., 5., 6., 7., 8., 9., 10., 11., 12., 13., 14., 15., 16., 17., 18., 19., 20., 21., 22., 23., 24., 36., 48., 48.];
const VOICE_COUNT: [i32; 8] = [2, 3, 4, 5, 6, 8, 12, 16];

fn bend_range(x: f32) -> f32 {
    PB_TABLE[step(x).clamp(0, 27) as usize]
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EnvSet {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub velocity: f32,
    pub model: i32,
    pub quantise: bool,
    pub curve: bool,
    pub release_on: bool,
    pub key_follow: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LfoSet {
    pub sync: i32,
    pub restart: i32,
    pub wave: i32,
    pub phase: f32,
    pub delay: f32,
    pub depth_src: usize,
    pub depth_amt: f32,
    pub rate: f32,
    pub freq_src: usize,
    pub freq_amt: f32,
    pub positive: bool,
}

/// One slot of the matrix (Zest's own), only those that act.
#[derive(Clone, Copy, Debug, Default)]
pub struct MatrixSlot {
    pub src: usize,
    pub dst: i32,
    pub amt: f32,
}

#[derive(Clone, Copy)]
pub struct Settings {
    /// The plain values, as given.
    pub raw: [f32; COUNT],

    pub output: f32,
    pub fx_on: [bool; 2],
    pub fx_module: [i32; 2],

    pub voices: i32,
    pub stack: i32,
    pub mode: i32,
    pub priority: i32,
    pub glide_rate_mode: bool,
    pub glide: f32,
    pub glide2: f32,
    pub glide_range: f32,
    pub bend_up: f32,
    pub bend_down: f32,
    pub transpose: i32,
    pub mtun_on: bool,
    pub tuning_mode: bool,
    pub arp_on: bool,
    pub fine: f32,
    pub vc: [f32; 8],

    pub accuracy: i32,
    pub offline_best: bool,
    pub tune_slop: f32,
    pub trims: [[f32; 8]; 3],
    pub voice_map: [f32; 8],
    pub trans_m: i32,
    pub slop_knobs: [f32; 4],

    pub env: [EnvSet; 2],
    pub lfo: [LfoSet; 2],

    pub quant: f32,
    pub slew: f32,
    pub rect_src: usize,
    pub inv_src: usize,
    pub quant_src: usize,
    pub lag_src: usize,
    pub add_src: [usize; 2],
    pub mul_src: [usize; 2],

    pub matrix: [MatrixSlot; 4],
    pub matrix_n: usize,

    pub osc_model: i32,
    pub osc: OscSet,
    pub hpf: HpfSet,
    pub vcf: VcfSet,
    pub vca: VcaSet,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OscSet {
    pub tune: [f32; 3],
    pub shape: [f32; 3],
    pub volume: [f32; 3],
    pub fm: f32,
    pub fm_src: usize,
    pub fm_depth: f32,
    pub sync: [bool; 2],
    pub noise_vol: f32,
    pub noise_src: usize,
    pub noise_depth: f32,
    pub noise_color: bool,
    pub bp_noise: f32,
    pub vibrato: f32,
    pub tm1_src: usize,
    pub tm1_depth: f32,
    pub tm2_src: usize,
    pub tm2_depth: f32,
    pub tune_mod: [f32; 3],
    pub shape_src: usize,
    pub shape_depth: f32,
    pub shape_mod: [f32; 3],
    pub drift: f32,
    pub rst_ph: [f32; 3],
    pub pulse_width: f32,
    pub pw_src: usize,
    pub pw_depth: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HpfSet {
    pub model: i32,
    pub frequency: f32,
    pub resonance: f32,
    pub revision: i32,
    pub key_follow: f32,
    pub freq_src: usize,
    pub freq_depth: f32,
    pub post_freq: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct VcfSet {
    pub model: i32,
    pub frequency: f32,
    pub resonance: f32,
    pub fm1_src: usize,
    pub fm1_depth: f32,
    pub fm2_src: usize,
    pub fm2_depth: f32,
    pub key_follow: f32,
    pub filter_fm: f32,
    pub ladder_12db: bool,
    pub rough: bool,
    pub sk_revision: i32,
    pub svf_mode: i32,
    pub feedback: f32,
    pub res_src: usize,
    pub res_depth: f32,
    pub ffm_src: usize,
    pub ffm_depth: f32,
    pub fb_src: usize,
    pub fb_depth: f32,
    pub shape_mix: f32,
    pub shape_src: usize,
    pub shape_depth: f32,
    pub uhbie_bp: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct VcaSet {
    pub pan: f32,
    pub volume: f32,
    pub env1: bool,
    pub mod_src: usize,
    pub mod_depth: f32,
    pub pan_src: usize,
    pub pan_depth: f32,
    pub mode: i32,
    pub offset: f32,
}

macro_rules! env_set {
    ($v:ident, $a:ident, $d:ident, $s:ident, $r:ident, $vel:ident, $m:ident, $q:ident, $c:ident, $ro:ident, $k:ident) => {
        EnvSet {
            attack: $v[id::$a],
            decay: $v[id::$d],
            sustain: $v[id::$s],
            release: $v[id::$r],
            velocity: $v[id::$vel],
            model: step($v[id::$m]).clamp(0, 2),
            quantise: on($v[id::$q]),
            curve: on($v[id::$c]),
            release_on: on($v[id::$ro]),
            key_follow: $v[id::$k],
        }
    };
}

macro_rules! lfo_set {
    ($v:ident, $sy:ident, $re:ident, $w:ident, $ph:ident, $dl:ident, $ds:ident, $da:ident, $ra:ident, $fs:ident, $fa:ident, $po:ident) => {
        LfoSet {
            sync: step($v[id::$sy]),
            restart: step($v[id::$re]),
            wave: step($v[id::$w]),
            phase: $v[id::$ph],
            delay: $v[id::$dl],
            depth_src: source($v[id::$ds]),
            depth_amt: $v[id::$da],
            rate: $v[id::$ra],
            freq_src: source($v[id::$fs]),
            freq_amt: $v[id::$fa],
            positive: on($v[id::$po]),
        }
    };
}

impl Settings {
    pub fn read(v: &[f32; COUNT]) -> Settings {
        let mut trims = [[0.0; 8]; 3];
        let mut voice_map = [0.0; 8];
        let mut vc = [0.0; 8];
        for m in 0..8 {
            trims[0][m] = v[id::OPT_V1T1 + m];
            trims[1][m] = v[id::OPT_V1T2 + m];
            trims[2][m] = v[id::OPT_V1T3 + m];
            voice_map[m] = v[id::OPT_V1MOD + m];
            vc[m] = v[id::VCC_VC1 + m];
        }
        let mut matrix = [MatrixSlot::default(); 4];
        let mut matrix_n = 0;
        for (src, dst, amt) in [
            (id::MTX_SRC1, id::MTX_DST1, id::MTX_AMT1),
            (id::MTX_SRC2, id::MTX_DST2, id::MTX_AMT2),
            (id::MTX_SRC3, id::MTX_DST3, id::MTX_AMT3),
            (id::MTX_SRC4, id::MTX_DST4, id::MTX_AMT4),
        ] {
            let s = source(v[src]);
            let d = step(v[dst]);
            if s != 0 && d != 0 && v[amt] != 0.0 {
                matrix[matrix_n] = MatrixSlot { src: s, dst: d, amt: v[amt] };
                matrix_n += 1;
            }
        }
        Settings {
            raw: *v,
            output: v[id::MAIN_OUTPUT],
            fx_on: [on(v[id::MAIN_ACTIVE_FX1]), on(v[id::MAIN_ACTIVE_FX2])],
            fx_module: [step(v[id::FX1_MODULE]).clamp(0, 4), step(v[id::FX2_MODULE]).clamp(0, 4)],
            voices: VOICE_COUNT[step(v[id::VCC_VOICES]).clamp(0, 7) as usize],
            stack: step(v[id::VCC_VOICE_STACK]).clamp(1, 6),
            mode: step(v[id::VCC_MODE]).clamp(0, 4),
            priority: step(v[id::VCC_NOTE_PRIORITY]).clamp(0, 2),
            glide_rate_mode: step(v[id::VCC_GLIDEMODE]) == 1,
            glide: v[id::VCC_GLIDE],
            glide2: v[id::VCC_GLIDE2],
            glide_range: v[id::VCC_GLIDERANGE],
            bend_up: bend_range(v[id::VCC_PITCHBEND_UP]),
            bend_down: bend_range(v[id::VCC_PITCHBEND_DOWN]),
            transpose: step(v[id::VCC_TRANSPOSE]),
            mtun_on: step(v[id::VCC_MTUNS]) == 1,
            tuning_mode: on(v[id::VCC_TUNINGMODE]),
            arp_on: on(v[id::ARP_ONOFF]),
            fine: v[id::VCC_FINETUNECENTS],
            vc,
            accuracy: step(v[id::OPT_ACCURACY]).clamp(0, 3),
            offline_best: step(v[id::OPT_OFFLINEACC]) == 1,
            tune_slop: v[id::OPT_TUNESLOP],
            trims,
            voice_map,
            trans_m: step(v[id::OPT_TRANSM]),
            slop_knobs: [v[id::OPT_CUTOFFSLOP], v[id::OPT_GLIDESLOP], v[id::OPT_PWSLOP], v[id::OPT_ENVRATESLOP]],
            env: [
                env_set!(v, ENV1_ATTACK, ENV1_DECAY, ENV1_SUSTAIN, ENV1_RELEASE, ENV1_VELOCITY, ENV1_MODEL, ENV1_QUANTISE, ENV1_CURVE, ENV1_RELEASE_ON, ENV1_KEYFOLLOW),
                env_set!(v, ENV2_ATTACK, ENV2_DECAY, ENV2_SUSTAIN, ENV2_RELEASE, ENV2_VELOCITY, ENV2_MODEL, ENV2_QUANTISE, ENV2_CURVE, ENV2_RELEASE_ON, ENV2_KEYFOLLOW),
            ],
            lfo: [
                lfo_set!(v, LFO1_SYNC, LFO1_RESTART, LFO1_WAVEFORM, LFO1_PHASE, LFO1_DELAY, LFO1_DEPTHMOD_SRC1, LFO1_DEPTHMOD_DPT1, LFO1_RATE, LFO1_FREQMOD_SRC1, LFO1_FREQMOD_DPT, LFO1_POLARITY),
                lfo_set!(v, LFO2_SYNC, LFO2_RESTART, LFO2_WAVEFORM, LFO2_PHASE, LFO2_DELAY, LFO2_DEPTHMOD_SRC1, LFO2_DEPTHMOD_DPT1, LFO2_RATE, LFO2_FREQMOD_SRC1, LFO2_FREQMOD_DPT, LFO2_POLARITY),
            ],
            quant: v[id::MOD_QUANTISE],
            slew: v[id::MOD_SLEW_RATE],
            rect_src: source(v[id::MOD_RECTIFYSOURCE]),
            inv_src: source(v[id::MOD_INVERTSOURCE]),
            quant_src: source(v[id::MOD_QUANTISESOURCE]),
            lag_src: source(v[id::MOD_LAGSOURCE]),
            add_src: [source(v[id::MOD_ADDSOURCE1]), source(v[id::MOD_ADDSOURCE2])],
            mul_src: [source(v[id::MOD_MULSOURCE1]), source(v[id::MOD_MULSOURCE2])],
            matrix,
            matrix_n,
            osc_model: step(v[id::OSC_MODEL]).clamp(0, 4),
            osc: OscSet {
                tune: [v[id::OSC_TUNE1], v[id::OSC_TUNE2], v[id::OSC_TUNE3]],
                shape: [v[id::OSC_SHAPE1], v[id::OSC_SHAPE2], v[id::OSC_SHAPE3]],
                volume: [v[id::OSC_VOLUME1], v[id::OSC_VOLUME2], v[id::OSC_VOLUME3]],
                fm: v[id::OSC_FM],
                fm_src: source(v[id::OSC_FMMODSRC]),
                fm_depth: v[id::OSC_FMMODDEPTH],
                sync: [on(v[id::OSC_SYNC2]), on(v[id::OSC_SYNC3])],
                noise_vol: v[id::OSC_NOISEVOL],
                noise_src: source(v[id::OSC_NOISEVOLMODSRC]),
                noise_depth: v[id::OSC_NOISEVOLMODDEPTH],
                noise_color: step(v[id::OSC_NOISECOLOR]) == 1,
                bp_noise: if on(v[id::OSC_BPNOISE]) { 1.0 } else { 0.0 },
                vibrato: v[id::OSC_VIBRATO],
                tm1_src: source(v[id::OSC_TUNE1MODSRC]),
                tm1_depth: v[id::OSC_TUNE1MODDEPTH],
                tm2_src: source(v[id::OSC_TUNE2MODSRC]),
                tm2_depth: v[id::OSC_TUNE2MODDEPTH],
                tune_mod: [
                    if on(v[id::OSC_TUNEMODOSC1]) { 1.0 } else { 0.0 },
                    if on(v[id::OSC_TUNEMODOSC2]) { 1.0 } else { 0.0 },
                    if on(v[id::OSC_TUNEMODOSC3]) { 1.0 } else { 0.0 },
                ],
                shape_src: source(v[id::OSC_SHAPESRC]),
                shape_depth: v[id::OSC_SHAPEDEPTH],
                shape_mod: [
                    if on(v[id::OSC_SHAPEMODOSC1]) { 1.0 } else { 0.0 },
                    if on(v[id::OSC_SHAPEMODOSC2]) { 1.0 } else { 0.0 },
                    if on(v[id::OSC_SHAPEMODOSC3]) { 1.0 } else { 0.0 },
                ],
                drift: v[id::OSC_DRIFT],
                rst_ph: [v[id::OSC_RSTPH1], v[id::OSC_RSTPH2], v[id::OSC_RSTPH3]],
                pulse_width: v[id::OSC_PULSEWIDTH],
                pw_src: source(v[id::OSC_PWMODSRC]),
                pw_depth: v[id::OSC_PWMODDEPTH],
            },
            hpf: HpfSet {
                model: step(v[id::HPF_MODEL]).clamp(0, 3),
                frequency: v[id::HPF_FREQUENCY],
                resonance: v[id::HPF_RESONANCE],
                revision: step(v[id::HPF_REVISION]),
                key_follow: v[id::HPF_KEYFOLLOW],
                freq_src: source(v[id::HPF_FREQMODSRC]),
                freq_depth: v[id::HPF_FREQMODDEPTH],
                post_freq: step(v[id::HPF_POST_HPF_FREQ]),
            },
            vcf: VcfSet {
                model: step(v[id::VCF1_MODEL]).clamp(0, 4),
                frequency: v[id::VCF1_FREQUENCY],
                resonance: v[id::VCF1_RESONANCE],
                fm1_src: source(v[id::VCF1_FREQMODSRC]),
                fm1_depth: v[id::VCF1_FREQMODDEPTH],
                fm2_src: source(v[id::VCF1_FREQMOD2SRC]),
                fm2_depth: v[id::VCF1_FREQMOD2DEPTH],
                key_follow: v[id::VCF1_KEYFOLLOW],
                filter_fm: v[id::VCF1_FILTERFM],
                ladder_12db: step(v[id::VCF1_LADDERMODE]) == 1,
                rough: step(v[id::VCF1_LADDERCOLOR]) == 1,
                sk_revision: step(v[id::VCF1_SLNKYREVISION]),
                svf_mode: step(v[id::VCF1_SVFMODE]).clamp(0, 3),
                feedback: v[id::VCF1_FEEDBACK],
                res_src: source(v[id::VCF1_RESMODSRC]),
                res_depth: v[id::VCF1_RESMODDEPTH],
                ffm_src: source(v[id::VCF1_FMAMOUNTMODSRC]),
                ffm_depth: v[id::VCF1_FMAMOUNTMODDEPTH],
                fb_src: source(v[id::VCF1_FEEDBACKMODSRC]),
                fb_depth: v[id::VCF1_FEEDBACKMODDEPTH],
                shape_mix: v[id::VCF1_SHAPEMIX],
                shape_src: source(v[id::VCF1_SHAPEMODSRC]),
                shape_depth: v[id::VCF1_SHAPEMODDEPTH],
                uhbie_bp: step(v[id::VCF1_UHBIEBANDPASS]) == 1,
            },
            vca: VcaSet {
                pan: v[id::VCA1_PAN],
                volume: v[id::VCA1_VOLUME],
                env1: v[id::VCA1_VCA] != 0.0,
                mod_src: source(v[id::VCA1_MODULATION]),
                mod_depth: v[id::VCA1_MODDEPTH],
                pan_src: source(v[id::VCA1_PANMODULATION]),
                pan_depth: v[id::VCA1_PANMODDEPTH],
                mode: step(v[id::VCA1_MODE]).clamp(0, 2),
                offset: v[id::VCA1_OFFSET],
            },
        }
    }

    /// The feedback gate of `hpf.md`: `VCF1/Feedback` acts when `OSC/Model` = 0 or `HPF/Model` = 0.
    #[inline]
    pub fn feedback_gate(&self) -> bool {
        self.osc_model == 0 || self.hpf.model == 0
    }

    /// Effective accuracy (`index.md`, Rendering a span, step 3).
    #[inline]
    pub fn effective_accuracy(&self, offline: bool) -> i32 {
        if offline && self.offline_best {
            3
        } else {
            self.accuracy
        }
    }
}
