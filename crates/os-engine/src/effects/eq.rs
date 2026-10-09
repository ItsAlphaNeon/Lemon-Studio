//! OpenStudio's 7-band parametric EQ, configured from Parametric EQ 2 saved state.
//!
// spec: plugins/parametric-eq-2.md

use super::{field, Biquad, Effect};

const BANDS: usize = 7;

pub struct ParametricEq {
    sections: Vec<Biquad>,
    main_gain: f32,
}

impl ParametricEq {
    pub fn from_state(state: &[u8], sample_rate: u32) -> Option<Self> {
        let sr = f64::from(sample_rate);
        let param = |k: usize| field(state, k + 1);
        let mut sections = Vec::new();
        for b in 0..BANDS {
            let db = f64::from(param(b)?) / 100.0;
            let n = f64::from(param(7 + b)?) / 65536.0;
            let w = (f64::from(param(14 + b)?) / 65536.0).max(0.01);
            let kind = param(21 + b)?;
            let order = param(28 + b)?;
            let f0 = 20.0 * 1000f64.powf(n);
            let q = 0.36 / w;
            // Pass filters: identical resonant sections (spec: fitted Q per order setting).
            let (sections_n, pass_q) = match order {
                -1 => (2, 0.96),  // Steep 4
                1 => (2, 0.82),   // Gentle 4
                3 => (4, 0.76),   // Gentle 8
                _ => (4, 0.94),   // Steep 8 (−3) and the template default 0
            };
            match kind {
                1 => sections.extend((0..sections_n).map(|_| Biquad::low_pass(f0, pass_q, sr))),
                2 => sections.push(Biquad::band_pass(f0, q, sr)),
                3 => sections.extend((0..sections_n).map(|_| Biquad::high_pass(f0, pass_q, sr))),
                4 => sections.push(Biquad::notch(f0, q, sr)),
                5 if db != 0.0 => sections.push(Biquad::low_shelf(f0, 0.575 / w, db, sr)),
                6 if db != 0.0 => sections.push(Biquad::peaking(f0, q, db, sr)),
                7 if db != 0.0 => sections.push(Biquad::high_shelf(f0, 0.575 / w, db, sr)),
                _ => {} // disabled, or a shelf/peak at 0 dB (transparent)
            }
        }
        let main_db = f64::from(param(35)?) / 100.0;
        Some(ParametricEq { sections, main_gain: 10f64.powf(main_db / 20.0) as f32 })
    }
}

impl Effect for ParametricEq {
    fn process(&mut self, buf: &mut [f32]) {
        for s in &mut self.sections {
            s.process(buf);
        }
        if self.main_gain != 1.0 {
            for x in buf.iter_mut() {
                *x *= self.main_gain;
            }
        }
    }
}
