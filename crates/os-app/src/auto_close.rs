// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Audio ▸ Input / output ▸ Auto close: while another program is in front, Lemon Studio lets go of its audio output.
//!
//! With the check on, once no Lemon Studio window (the main window, a detached plugin window, a file dialog) has been
//! in front for `GRACE`, a playing transport pauses where it is and the output streams close
//! (`Transport::close_device`). When one of Lemon Studio's windows is in front again, or the check is turned off, the
//! streams reopen (`Transport::open_device`) and a playback that Auto close paused goes on from the same place. MIDI
//! input and the UI keep running meanwhile; what they would play finds no stream and isn't heard.
// spec: ui/settings-choices.md "Input / output: Auto close; CPU and Mixer: Smart disable, Align tick lengths,
// Resampling quality (2026-10-16)"

use std::time::{Duration, Instant};

use crate::app::App;

/// How long no Lemon Studio window must be in front before the output closes: a click from the main window into a
/// plugin window can pass through a moment with no window in front.
pub const GRACE: Duration = Duration::from_millis(300);

/// What `AutoClose::poll` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Close the output; `pause`: pause the playing transport first.
    Close { pause: bool },
    /// Open the output again; `resume`: go on playing (Auto close paused it).
    Open { resume: bool },
}

/// Where Auto close is.
#[derive(Debug, Clone)]
pub struct AutoClose {
    /// The main window has the keyboard focus (winit's `Focused`): where the system can't tell which program is in
    /// front, Lemon Studio is while this is so (`lemon_in_front`).
    pub main_focused: bool,
    /// Since when no Lemon Studio window has been in front (while the output is open and the check on).
    away: Option<Instant>,
    /// The output is closed by Auto close: `Some(resume)`, `resume` = it paused a playback.
    closed: Option<bool>,
}

impl Default for AutoClose {
    fn default() -> AutoClose {
        AutoClose { main_focused: true, away: None, closed: None }
    }
}

impl AutoClose {
    /// The output is closed by Auto close.
    pub fn closed(&self) -> bool {
        self.closed.is_some()
    }

    /// What to do at `now`, with the check `on`, `ours` = one of Lemon Studio's windows is in front and `playing` = the
    /// transport plays (not paused). Called periodically:
    /// - open: closes once no Lemon Studio window has been in front for `GRACE` (each poll that finds one in front
    ///   starts the wait over), pausing the transport if it plays;
    /// - closed: opens when one of Lemon Studio's windows is in front again, or the check is off, and resumes the
    ///   playback it paused.
    pub fn poll(&mut self, now: Instant, on: bool, ours: bool, playing: bool) -> Option<Step> {
        if let Some(resume) = self.closed {
            if on && !ours {
                return None;
            }
            self.closed = None;
            self.away = None;
            return Some(Step::Open { resume });
        }
        if !on || ours {
            self.away = None;
            return None;
        }
        let since = *self.away.get_or_insert(now);
        if now.saturating_duration_since(since) < GRACE {
            return None;
        }
        self.away = None;
        self.closed = Some(playing);
        Some(Step::Close { pause: playing })
    }
}

/// Whether one of Lemon Studio's windows is in front. Windows: the foreground window belongs to this process (the main
/// window, a detached plugin window, a file dialog). Elsewhere: the main window has the focus (`main_focused`), so a
/// detached plugin window in front counts as another program there.
pub fn lemon_in_front(main_focused: bool) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::GetCurrentProcessId;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
        let _ = main_focused;
        // SAFETY: plain queries; a null window (none in front, or the focus moving) is nobody's.
        let h = unsafe { GetForegroundWindow() };
        if h.is_null() {
            return false;
        }
        let mut pid = 0u32;
        // SAFETY: as above.
        unsafe { GetWindowThreadProcessId(h, &mut pid) };
        // SAFETY: as above.
        pid == unsafe { GetCurrentProcessId() }
    }
    #[cfg(not(windows))]
    main_focused
}

/// The app's side of Auto close.
impl App {
    /// Periodic (`App::tick`): Auto close with the window in front now.
    pub(crate) fn auto_close_tick(&mut self, now: Instant) {
        let on = crate::prefs::check(&crate::prefs::AUTO_CLOSE);
        // Only asked while it matters (the check on, or the output closed by it).
        let ours = (on || self.auto_close.closed()) && lemon_in_front(self.auto_close.main_focused);
        self.auto_close_at(now, ours);
    }

    /// Auto close at `now` with `ours` = one of Lemon Studio's windows is in front: closes or opens the output, pausing
    /// or resuming the transport (`AutoClose::poll`).
    pub(crate) fn auto_close_at(&mut self, now: Instant, ours: bool) {
        let on = crate::prefs::check(&crate::prefs::AUTO_CLOSE);
        let playing = self.transport.state() == crate::transport::State::Playing;
        match self.auto_close.poll(now, on, ours, playing) {
            Some(Step::Close { pause }) => {
                if pause {
                    self.transport.pause();
                }
                self.transport.close_device();
                self.invalidate();
            }
            Some(Step::Open { resume }) => {
                self.transport.open_device();
                if resume {
                    self.transport.resume();
                }
                self.invalidate();
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    // spec: ui/settings-choices.md "Input / output: Auto close; CPU and Mixer: Smart disable, Align tick lengths,
    // Resampling quality (2026-10-16)"
    /// Focus lost to another program closes the output once it has stayed away for the grace, pausing a playing
    /// transport; the focus back reopens it and resumes the playback.
    #[test]
    fn focus_lost_to_another_program_closes_and_its_return_reopens() {
        let t0 = Instant::now();
        let mut a = AutoClose::default();
        assert_eq!(a.poll(t0, true, true, true), None, "in front: nothing");
        assert_eq!(a.poll(t0 + 100 * MS, true, false, true), None, "away: the grace first");
        assert_eq!(a.poll(t0 + 300 * MS, true, false, true), None);
        assert_eq!(a.poll(t0 + 400 * MS, true, false, true), Some(Step::Close { pause: true }), "300 ms away");
        assert!(a.closed());
        assert_eq!(a.poll(t0 + 2000 * MS, true, false, false), None, "closed while away");
        assert_eq!(a.poll(t0 + 3000 * MS, true, true, false), Some(Step::Open { resume: true }), "back: goes on playing");
        assert!(!a.closed());
        assert_eq!(a.poll(t0 + 3100 * MS, true, true, true), None);
    }

    /// Stopped or paused when the focus went: the return reopens the output without playing.
    #[test]
    fn the_return_resumes_only_what_was_playing() {
        let t0 = Instant::now();
        let mut a = AutoClose::default();
        a.poll(t0, true, false, false);
        assert_eq!(a.poll(t0 + GRACE, true, false, false), Some(Step::Close { pause: false }));
        assert_eq!(a.poll(t0 + GRACE + MS, true, true, true), Some(Step::Open { resume: false }));
    }

    /// The focus going to one of Lemon Studio's own windows (a plugin window, a dialog) closes nothing; a short trip
    /// away and back starts the wait over.
    #[test]
    fn focus_to_a_lemon_window_keeps_the_output() {
        let t0 = Instant::now();
        let mut a = AutoClose::default();
        for i in 0..20 {
            assert_eq!(a.poll(t0 + i * 100 * MS, true, true, true), None);
        }
        assert_eq!(a.poll(t0 + 2000 * MS, true, false, true), None);
        assert_eq!(a.poll(t0 + 2200 * MS, true, true, true), None, "back within the grace");
        assert_eq!(a.poll(t0 + 2400 * MS, true, false, true), None, "away again: a new wait");
        assert_eq!(a.poll(t0 + 2600 * MS, true, false, true), None);
        assert_eq!(a.poll(t0 + 2700 * MS, true, false, true), Some(Step::Close { pause: true }));
    }

    /// Off, nothing closes; turned off while closed, the output reopens (and the playback goes on) at once.
    #[test]
    fn off_keeps_the_output_and_turning_it_off_reopens() {
        let t0 = Instant::now();
        let mut a = AutoClose::default();
        for i in 0..20 {
            assert_eq!(a.poll(t0 + i * 100 * MS, false, false, true), None);
        }
        a.poll(t0, true, false, true);
        assert_eq!(a.poll(t0 + GRACE, true, false, true), Some(Step::Close { pause: true }));
        assert_eq!(a.poll(t0 + GRACE + MS, false, false, false), Some(Step::Open { resume: true }), "off: reopens");
        assert_eq!(a.poll(t0 + 2 * GRACE, false, false, false), None);
    }

    /// The app sends the transport its steps; with the check off (the default) the focus changes nothing.
    #[test]
    fn the_app_follows_the_focus() {
        let t0 = Instant::now();
        let mut app = App::new(None);
        app.auto_close_at(t0, false);
        app.auto_close_at(t0 + GRACE, false);
        assert!(!app.auto_close.closed(), "off by default");
        crate::prefs::set_check(&crate::prefs::AUTO_CLOSE, true);
        app.auto_close_at(t0 + 2 * GRACE, false);
        app.auto_close_at(t0 + 3 * GRACE, false);
        assert!(app.auto_close.closed());
        assert_eq!(app.transport.state(), crate::transport::State::Stopped, "nothing was playing");
        app.auto_close_at(t0 + 4 * GRACE, true);
        assert!(!app.auto_close.closed(), "back in front");
        app.auto_close_at(t0 + 5 * GRACE, false);
        app.auto_close_at(t0 + 6 * GRACE, false);
        assert!(app.auto_close.closed());
        crate::prefs::set_check(&crate::prefs::AUTO_CLOSE, false);
        app.auto_close_at(t0 + 7 * GRACE, false);
        assert!(!app.auto_close.closed(), "turned off");
    }
}
