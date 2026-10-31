// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The keyboard's part in the startup logo (`crate::splash`): a snake of the logo's lemon runs round the pads with the
//! logo's groove light, growing as the logo's lemon fills, until the ring is whole; it pops with the logo's mark and
//! fades with the logo. On a thread of its own, as the logo's, since the UI thread is busy loading then.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::launchkey::{self as lk, Colour, Led, Model};
use crate::splash::{Follow, Moment};

/// The pads round the ring, clockwise from the top row's left.
const RING: [u8; 16] = [8, 9, 10, 11, 12, 13, 14, 15, 7, 6, 5, 4, 3, 2, 1, 0];
/// The snake's length in pads at the start of the logo's gauge.
const SHORTEST: f32 = 3.0;
/// The ring's level until the logo's mark pops.
const WHOLE: f32 = 0.7;
const FRAME: Duration = Duration::from_millis(33);

/// The level of each pad of `RING`, 0..=1.
fn levels(m: &Moment) -> [f32; 16] {
    let n = RING.len() as f32;
    let head = (m.angle / 360.0).rem_euclid(1.0) * n;
    let long = SHORTEST + (n - SHORTEST) * m.progress;
    let whole = WHOLE + (1.0 - WHOLE) * m.pop;
    std::array::from_fn(|i| {
        // How far behind the head this pad is, round the ring: the snake brightest at its head.
        let behind = (head - i as f32).rem_euclid(n);
        let tail = (1.0 - behind / long).max(0.0);
        let snake = tail * tail;
        // As the logo's light goes out, the snake becomes the whole ring.
        (snake * m.light + whole * (1.0 - m.light)) * m.alpha
    })
}

fn colour(level: f32) -> Colour {
    let l = crate::splash::lemon();
    let of = |c: u8| (f32::from(c) * level.clamp(0.0, 1.0)).round() as u8;
    Colour::of(os_model::Rgb::new(of(l.r), of(l.g), of(l.b)))
}

/// The snake, while it runs: the pads are its, not the drive's. Dropping it ends it.
pub struct Intro {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Intro {
    /// Starts it on `output`.
    pub fn start(mut follow: Follow, output: Arc<Mutex<midir::MidiOutputConnection>>, model: Model, wake: fn()) -> Option<Intro> {
        let (stop, done) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (s, d) = (stop.clone(), done.clone());
        let thread = std::thread::Builder::new()
            .name("keyboard intro".into())
            .spawn(move || {
                let mut lights = lk::Lights::default();
                let mut send = |lights: &mut lk::Lights| {
                    if let (Some(m), Ok(mut o)) = (lights.flush(model), output.lock()) {
                        let _ = o.send(&m);
                    }
                };
                while !s.load(Ordering::Relaxed) {
                    let Some(m) = follow.moment() else { break };
                    for (i, l) in levels(&m).into_iter().enumerate() {
                        lights.set(Led::Pad(RING[i]), colour(l), false);
                    }
                    send(&mut lights);
                    std::thread::sleep(FRAME);
                }
                lights.all_off();
                send(&mut lights);
                d.store(true, Ordering::Release);
                wake();
            })
            .ok()?;
        Some(Intro { stop, done, thread: Some(thread) })
    }

    /// It has ended: the pads are the drive's again.
    pub fn done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
}

impl Drop for Intro {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(angle: f32, progress: f32, light: f32, pop: f32, alpha: f32) -> [f32; 16] {
        levels(&Moment { alpha, pop, progress, light, angle })
    }

    #[test]
    fn the_snake_runs_with_the_light_and_grows_with_the_gauge() {
        // The head on the top row's fifth pad, a quarter of the way round.
        let short = at(90.0, 0.0, 1.0, 0.0, 1.0);
        assert_eq!(short[4], 1.0);
        assert!(short[3] > short[2] && short[2] > 0.0, "{short:?}");
        assert_eq!(short[5], 0.0, "nothing ahead of the head");
        assert_eq!(short[0], 0.0, "too far behind");
        let long = at(90.0, 0.9, 1.0, 0.0, 1.0);
        assert!(long[0] > 0.0 && long[12] > 0.0, "{long:?}");
        // It wraps round the ring's end.
        let wrapped = at(0.0, 0.0, 1.0, 0.0, 1.0);
        assert_eq!(wrapped[0], 1.0);
        assert!(wrapped[15] > 0.0, "{wrapped:?}");
    }

    #[test]
    fn the_ring_is_whole_as_the_light_goes_out_pops_and_fades() {
        assert!(at(90.0, 1.0, 0.0, 0.0, 1.0).iter().all(|&l| l == WHOLE));
        assert!(at(90.0, 1.0, 0.0, 1.0, 1.0).iter().all(|&l| l == 1.0));
        assert!(at(90.0, 1.0, 0.0, 1.0, 0.0).iter().all(|&l| l == 0.0));
    }

    #[test]
    fn the_ring_goes_clockwise_over_every_pad() {
        let mut pads = RING.to_vec();
        pads.sort_unstable();
        assert_eq!(pads, (0..16).collect::<Vec<u8>>());
        assert_eq!(colour(0.0), Colour::Rgb(0, 0, 0));
    }
}
