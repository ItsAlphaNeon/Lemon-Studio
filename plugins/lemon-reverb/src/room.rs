// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The room visual: a wireframe prism turning in a tinted panel. The diffusion sets its number of sides, the room size
//! its shape and the camera's height, the wet level its glow; the panel's tint follows the filters, the decay, the high
//! damping and the bass multiplier. All laws are measured from the original's panel (160 px square); the panel is drawn
//! at any size by scaling that frame.
//!
// spec: plugins/lemon-reverb.md §2 (the room visual)

use os_engine::effects::reverb::{BASS, DAMPING, DECAY, DIFFUSION, HIGH_CUT, LOW_CUT, PARAMS, ROOM, WET};

/// The model's frame: the original's panel in px.
pub const FRAME: f32 = 160.0;
/// Seconds per turn (counter-clockwise seen from above).
pub const TURN_SECS: f64 = 34.97;

/// Camera: distance from its aim, focal scale and the image of the aim (the panel's centre), in frame px.
const CAM_DIST: f32 = 3.0;
const FOCAL: f32 = 170.5;
const CENTRE: (f32, f32) = (80.0, 80.0);

/// The camera over the room size (fit per size; straight lines between): elevation in degrees above the horizon (90 =
/// straight down on the smallest room) and the prism's height, in units of a third of the camera's distance.
const CAMERA: [(i32, f32, f32); 20] = [
    (1, 90.0, 2.435),
    (2, 88.21, 2.328),
    (3, 86.51, 2.243),
    (5, 83.05, 2.084),
    (7, 79.78, 1.945),
    (10, 75.05, 1.775),
    (13, 70.36, 1.629),
    (16, 66.26, 1.52),
    (20, 61.1, 1.403),
    (25, 55.4, 1.295),
    (30, 50.32, 1.201),
    (35, 45.82, 1.13),
    (40, 41.74, 1.068),
    (45, 38.25, 1.018),
    (50, 35.07, 0.975),
    (60, 29.88, 0.906),
    (70, 25.81, 0.853),
    (80, 22.68, 0.814),
    (90, 20.22, 0.779),
    (100, 18.34, 0.751),
];

/// The prism's number of sides for diffusion 0..100.
pub const SIDES: [u8; 101] = [
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, //
    4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, //
    6, 6, 6, 6, 6, 6, 6, 7, 7, 7, 7, 7, 7, 8, 8, 8, 8, 8, 9, 9, //
    9, 9, 10, 10, 10, 10, 11, 11, 11, 11, 12, 12, 12, 13, 13, 14, 14, 14, 15, 15, //
    16, 16, 16, 17, 17, 18, 18, 19, 20, 20, 21, 21, 22, 23, 23, 24, 25, 26, 26, 27, //
    28,
];

/// Panel tint: the decay's strength over the decay knob (0..1).
const DECAY_STRENGTH: [(i32, f32); 15] = [
    (1, 0.09),
    (2, 0.12),
    (5, 0.152),
    (10, 0.212),
    (15, 0.27),
    (20, 0.333),
    (30, 0.45),
    (40, 0.515),
    (60, 0.66),
    (80, 0.758),
    (100, 0.82),
    (120, 0.879),
    (130, 0.9),
    (160, 0.94),
    (200, 1.0),
];

/// Line colour and width; the lines' opacity at the prism's axis and its gain towards the camera (per unit of a point's
/// horizontal distance in front of the axis, times the cosine of the elevation).
const LINE: [f32; 3] = [255.0, 255.0, 255.0];
const LINE_WIDTH: f32 = 1.15;
const SHADE_MID: f32 = 0.707;
const SHADE_GAIN: f32 = 0.342;
/// Glow per wet step (of the blurred, shaded line coverage) and its blur radius in frame px.
const GLOW_PER_WET: f32 = 0.0206;
const GLOW_SIGMA: f32 = 8.3;

fn interp(table: &[(i32, f32)], x: f32) -> f32 {
    let (first, last) = (table[0], table[table.len() - 1]);
    if x <= first.0 as f32 {
        return first.1;
    }
    for w in table.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x <= x1 as f32 {
            return y0 + (y1 - y0) * (x - x0 as f32) / (x1 - x0) as f32;
        }
    }
    last.1
}

/// The camera's elevation (degrees) and the prism's height at room size `size`, straight between the table's rows.
fn camera(size: f32) -> (f32, f32) {
    let i = CAMERA.iter().position(|r| size <= r.0 as f32).unwrap_or(CAMERA.len() - 1).max(1);
    let ((x0, e0, h0), (x1, e1, h1)) = (CAMERA[i - 1], CAMERA[i]);
    let t = ((size - x0 as f32) / (x1 - x0) as f32).clamp(0.0, 1.0);
    (e0 + (e1 - e0) * t, h0 + (h1 - h0) * t)
}

/// The hue wheel at `h` degrees: red, yellow, green, cyan, blue, magenta, each channel 0..1 and linear between.
fn wheel(h: f32) -> [f32; 3] {
    let s = h.rem_euclid(360.0) / 60.0;
    let f = s.fract();
    match s as u32 % 6 {
        0 => [1.0, f, 0.0],
        1 => [1.0 - f, 1.0, 0.0],
        2 => [0.0, 1.0, f],
        3 => [0.0, 1.0 - f, 1.0],
        4 => [f, 0.0, 1.0],
        _ => [1.0, 0.0, 1.0 - f],
    }
}

/// What the visual shows for a setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub sides: u32,
    /// Prism radius and height, and how far its centre sits above the camera's aim.
    pub radius: f32,
    pub height: f32,
    pub lift: f32,
    /// Camera elevation in radians.
    pub elevation: f32,
    /// Panel colour, 0..255 per channel.
    pub tint: [f32; 3],
    /// Glow gain.
    pub glow: f32,
}

/// The panel colour: a grey whose brightness follows the bass multiplier, plus two hue-wheel colours (the low cut's,
/// and the high cut's scaled by the high damping) whose strength grows with the decay.
pub fn tint(k: &[i32; PARAMS]) -> [f32; 3] {
    let h1 = -0.0722 * (k[LOW_CUT].max(10) - 10) as f32;
    let h2 = 140.0 + k[HIGH_CUT] as f32;
    let damp = 4.6 + 0.0833 * (k[DAMPING] - 5) as f32;
    let s = interp(&DECAY_STRENGTH, k[DECAY] as f32);
    let grey = 83.5 - 0.085 * (k[BASS] - 100) as f32;
    let amp = (grey / 83.5).max(0.0).powf(0.75) * s;
    let (a, b) = (wheel(h1), wheel(h2));
    std::array::from_fn(|c| (grey + amp * (42.0 * (a[c] - 0.5) + damp * (b[c] - 0.5))).clamp(0.0, 255.0))
}

pub fn look(k: &[i32; PARAMS]) -> Look {
    let size = k[ROOM].clamp(1, 100) as f32;
    let (elevation, height) = camera(size);
    Look {
        sides: u32::from(SIDES[k[DIFFUSION].clamp(0, 100) as usize]),
        radius: 0.1655 + 0.00974 * size,
        height,
        lift: 0.0405 + 0.00109 * size,
        elevation: elevation.to_radians(),
        tint: tint(k),
        glow: GLOW_PER_WET * k[WET].max(0) as f32,
    }
}

/// The prism's edges in frame px with each end's line opacity, at turn angle `angle` (radians, counter-clockwise from
/// above; 0 = a corner towards the camera): the top rim, the bottom rim, the uprights.
pub fn edges(l: &Look, angle: f32) -> Vec<[(f32, f32, f32); 2]> {
    let n = l.sides.max(3);
    let (ce, se) = (l.elevation.cos(), l.elevation.sin());
    // Camera at distance D towards -y, raised by the elevation, looking at the origin; screen x = world x.
    let cam = [0.0, -CAM_DIST * ce, CAM_DIST * se];
    let fwd = [0.0, ce, -se];
    let up = [0.0, se, ce];
    let project = |p: [f32; 3]| {
        let v = [p[0] - cam[0], p[1] - cam[1], p[2] - cam[2]];
        let depth = (v[1] * fwd[1] + v[2] * fwd[2]).max(1e-3);
        let (x, y) = (v[0], v[1] * up[1] + v[2] * up[2]);
        // Lines fade with the horizontal distance behind the axis (the same at both ends of an upright).
        let shade = (SHADE_MID - SHADE_GAIN * p[1] * ce).clamp(0.0, 1.0);
        (CENTRE.0 + FOCAL * x / depth, CENTRE.1 - FOCAL * y / depth, shade)
    };
    let corner = |i: u32, z: f32| {
        let t = angle - std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * i as f32 / n as f32;
        project([l.radius * t.cos(), l.radius * t.sin(), l.lift + z])
    };
    let top: Vec<_> = (0..n).map(|i| corner(i, 0.5 * l.height)).collect();
    let bot: Vec<_> = (0..n).map(|i| corner(i, -0.5 * l.height)).collect();
    let mut e = Vec::with_capacity(3 * n as usize);
    for i in 0..n as usize {
        let j = (i + 1) % n as usize;
        e.push([top[i], top[j]]);
        e.push([bot[i], bot[j]]);
        e.push([top[i], bot[i]]);
    }
    e
}

/// The turn angle `secs` after the editor opened.
pub fn angle_at(secs: f64) -> f32 {
    (std::f64::consts::TAU * (secs / TURN_SECS).fract()) as f32
}

/// A Gaussian blur of a `w` x `h` plane (separable, edges read as 0).
fn blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let r = (3.0 * sigma).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / sum).collect();
    let mut tmp = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut a = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let xx = x as isize + i as isize - r;
                if xx >= 0 && (xx as usize) < w {
                    a += kv * src[y * w + xx as usize];
                }
            }
            tmp[y * w + x] = a;
        }
    }
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut a = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let yy = y as isize + i as isize - r;
                if yy >= 0 && (yy as usize) < h {
                    a += kv * tmp[yy as usize * w + x];
                }
            }
            out[y * w + x] = a;
        }
    }
    out
}

/// The panel as `side` x `side` pixels `0x00RRGGBB`: the tint, the lines, the glow (screened over).
pub fn render(l: &Look, angle: f32, side: usize) -> Vec<u32> {
    let side = side.max(1);
    let k = side as f32 / FRAME;
    // Line coverage.
    let mut cov = vec![0.0f32; side * side];
    if let Some(mut pm) = tiny_skia::Pixmap::new(side as u32, side as u32) {
        let stroke = tiny_skia::Stroke { width: LINE_WIDTH * k, ..Default::default() };
        let white = |a: f32| tiny_skia::Color::from_rgba(1.0, 1.0, 1.0, a).unwrap_or(tiny_skia::Color::WHITE);
        for [(x0, y0, a0), (x1, y1, a1)] in edges(l, angle) {
            let mut pb = tiny_skia::PathBuilder::new();
            pb.move_to(x0 * k, y0 * k);
            pb.line_to(x1 * k, y1 * k);
            let Some(path) = pb.finish() else { continue };
            let mut paint = tiny_skia::Paint { anti_alias: true, ..Default::default() };
            // The opacity runs along the edge; edges drawn over each other add up as layers.
            paint.shader = tiny_skia::LinearGradient::new(
                tiny_skia::Point::from_xy(x0 * k, y0 * k),
                tiny_skia::Point::from_xy(x1 * k, y1 * k),
                vec![tiny_skia::GradientStop::new(0.0, white(a0)), tiny_skia::GradientStop::new(1.0, white(a1))],
                tiny_skia::SpreadMode::Pad,
                tiny_skia::Transform::identity(),
            )
            .unwrap_or(tiny_skia::Shader::SolidColor(white(0.5 * (a0 + a1))));
            pm.stroke_path(&path, &paint, &stroke, tiny_skia::Transform::identity(), None);
        }
        for (c, p) in cov.iter_mut().zip(pm.pixels()) {
            *c = f32::from(p.alpha()) / 255.0;
        }
    }
    // Glow: the coverage averaged over cells of about two frame px, blurred there, read back bilinearly.
    let cell = (2.0 * k).round().max(1.0) as usize;
    let g = side.div_ceil(cell);
    let glow = if l.glow > 0.0 {
        let mut small = vec![0.0f32; g * g];
        for y in 0..side {
            for x in 0..side {
                small[(y / cell) * g + x / cell] += cov[y * side + x];
            }
        }
        let norm = 1.0 / (cell * cell) as f32;
        small.iter_mut().for_each(|v| *v *= norm);
        Some(blur(&small, g, g, GLOW_SIGMA * k / cell as f32))
    } else {
        None
    };
    let sample = |b: &[f32], x: usize, y: usize| {
        let fx = ((x as f32 + 0.5) / cell as f32 - 0.5).clamp(0.0, (g - 1) as f32);
        let fy = ((y as f32 + 0.5) / cell as f32 - 0.5).clamp(0.0, (g - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(g - 1), (y0 + 1).min(g - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let top = b[y0 * g + x0] * (1.0 - tx) + b[y0 * g + x1] * tx;
        let bot = b[y1 * g + x0] * (1.0 - tx) + b[y1 * g + x1] * tx;
        top * (1.0 - ty) + bot * ty
    };
    let mut out = vec![0u32; side * side];
    for y in 0..side {
        for x in 0..side {
            let a = cov[y * side + x];
            let gl = glow.as_ref().map_or(0.0, |b| (l.glow * sample(b, x, y)).min(1.0));
            let mut px = 0u32;
            for c in 0..3 {
                let base = l.tint[c] + (LINE[c] - l.tint[c]) * a;
                let v = 255.0 - (255.0 - base) * (1.0 - gl);
                px = (px << 8) | v.round().clamp(0.0, 255.0) as u32;
            }
            out[y * side + x] = px;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_engine::effects::reverb::DEFAULTS;

    #[test]
    fn default_tint_is_the_original_panel() {
        // The original's panel at its defaults: (88, 79, 79).
        let t = tint(&DEFAULTS);
        for (c, want) in t.iter().zip([88.0, 79.0, 79.0]) {
            assert!((c - want).abs() < 1.0, "{t:?}");
        }
    }

    #[test]
    fn sides_follow_the_diffusion_and_the_room_its_size() {
        let mut k = DEFAULTS;
        assert_eq!(look(&k).sides, 28);
        k[DIFFUSION] = 0;
        assert_eq!(look(&k).sides, 3);
        assert_eq!(edges(&look(&k), 0.0).len(), 9);
        // The smallest room is seen from straight above, the largest from low down.
        k[ROOM] = 1;
        assert!(look(&k).elevation > 1.5);
        k[ROOM] = 100;
        assert!(look(&k).elevation < 0.35);
        assert!(look(&k).radius > look(&k).height);
        assert!(SIDES.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn a_corner_faces_the_camera_and_the_turn_is_counter_clockwise() {
        let mut k = DEFAULTS;
        k[DIFFUSION] = 0;
        let l = look(&k);
        // Edge 2 = the first upright: at angle 0 straight below the centre.
        let x0 = edges(&l, 0.0)[2][0].0;
        assert!((x0 - CENTRE.0).abs() < 0.01);
        // Counter-clockwise seen from above: the front corner moves right.
        assert!(edges(&l, 0.1)[2][0].0 > x0);
        assert!((angle_at(TURN_SECS) - 0.0).abs() < 1e-4);
    }

    #[test]
    fn render_tints_lines_and_glows() {
        let l = look(&DEFAULTS);
        let px = render(&l, 0.3, 160);
        assert_eq!(px.len(), 160 * 160);
        // The corner pixel is the tint; something reaches near-white; the glow brightens the tint near the prism.
        let ch = |p: u32, s: u32| ((p >> s) & 0xff) as i32;
        assert!([(16, 88), (8, 79), (0, 79)].iter().all(|&(s, want)| (ch(px[0], s) - want).abs() <= 1), "{:06x}", px[0]);
        assert!(px.iter().any(|p| (p >> 16) & 0xff > 220));
        let dry = render(&Look { glow: 0.0, ..l }, 0.3, 160);
        let lit = |v: &[u32]| v.iter().map(|p| (p >> 16) & 0xff).sum::<u32>();
        assert!(lit(&px) > lit(&dry));
    }
}

#[cfg(test)]
mod pictures {
    use super::*;
    use os_engine::effects::reverb::DEFAULTS;

    /// `ROOM_PNG=<dir> cargo test -p lemon-reverb pictures -- --ignored`: the panel alone at the original's size.
    #[test]
    #[ignore]
    fn render_panels() {
        let Some(dir) = std::env::var_os("ROOM_PNG") else { return };
        for (name, wet, angle) in [("room_default", 128, 0.0f32), ("room_dry", 0, 0.0), ("room_default_b", 128, 0.05)] {
            let mut k = DEFAULTS;
            k[WET] = wet;
            let px = render(&look(&k), angle, 160);
            let mut pm = tiny_skia::Pixmap::new(160, 160).unwrap();
            for (o, p) in pm.data_mut().chunks_mut(4).zip(&px) {
                o.copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
            }
            pm.save_png(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
        }
    }

    /// `ROOM_STATE=v0,..,v14 ROOM_PNG=<dir> cargo test -p lemon-reverb turns -- --ignored`: one setting at 72 turn
    /// angles (`turn_<i>.png`, i * 5 degrees), to line up with a capture of the original.
    #[test]
    #[ignore]
    fn render_turns() {
        let (Some(dir), Ok(state)) = (std::env::var_os("ROOM_PNG"), std::env::var("ROOM_STATE")) else { return };
        let mut k = DEFAULTS;
        for (i, v) in state.split(',').enumerate().take(k.len()) {
            k[i] = v.trim().parse().unwrap();
        }
        for i in 0..72 {
            let px = render(&look(&k), (i as f32 * 5.0).to_radians(), 160);
            let mut pm = tiny_skia::Pixmap::new(160, 160).unwrap();
            for (o, p) in pm.data_mut().chunks_mut(4).zip(&px) {
                o.copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
            }
            pm.save_png(std::path::Path::new(&dir).join(format!("turn_{i}.png"))).unwrap();
        }
    }
}
