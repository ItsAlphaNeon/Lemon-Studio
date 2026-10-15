// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The UI thread's run loop for plugin editors on Linux.
//!
//! There a plugin's editor has no message loop of its own: it gives the host file descriptors to watch (its X11
//! connection) and timers to run, and expects the callbacks on the UI thread. VST3 asks through `IRunLoop`, CLAP through
//! `timer-support` and `posix-fd-support`; both backends register here, and the app calls [`dispatch`] on every UI tick.
//!
//! Everything lives on the UI thread (registration happens from plugin calls made there), so the callbacks need not be
//! `Send`. A callback may register or remove entries, its own included.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Identifies a registered descriptor or timer.
pub type Token = u64;

/// Descriptor conditions (the same bits as CLAP's `clap_posix_fd_flags`).
pub const READ: u32 = 1;
pub const WRITE: u32 = 2;
pub const ERROR: u32 = 4;

type FdCall = Rc<RefCell<dyn FnMut(i32, u32)>>;
type TimerCall = Rc<RefCell<dyn FnMut()>>;

struct Fd {
    token: Token,
    fd: i32,
    flags: u32,
    call: FdCall,
}

struct Timer {
    token: Token,
    period: Duration,
    due: Instant,
    call: TimerCall,
}

#[derive(Default)]
struct Loop {
    fds: Vec<Fd>,
    timers: Vec<Timer>,
}

thread_local! {
    static LOOP: RefCell<Loop> = RefCell::new(Loop::default());
    static NEXT: Cell<Token> = const { Cell::new(1) };
}

fn token() -> Token {
    NEXT.with(|n| {
        let t = n.get();
        n.set(t + 1);
        t
    })
}

/// Watches `fd` for `flags` (`READ`, `WRITE`, `ERROR`); `call(fd, ready flags)` runs when it is ready.
pub fn add_fd(fd: i32, flags: u32, call: impl FnMut(i32, u32) + 'static) -> Token {
    let t = token();
    LOOP.with(|l| l.borrow_mut().fds.push(Fd { token: t, fd, flags, call: Rc::new(RefCell::new(call)) }));
    t
}

/// Changes what a registered descriptor is watched for.
pub fn modify_fd(token: Token, flags: u32) -> bool {
    LOOP.with(|l| l.borrow_mut().fds.iter_mut().find(|f| f.token == token).map(|f| f.flags = flags).is_some())
}

/// Runs `call` every `period` (at the UI tick's resolution, never more than once per tick).
pub fn add_timer(period: Duration, call: impl FnMut() + 'static) -> Token {
    let t = token();
    let period = period.max(Duration::from_millis(1));
    LOOP.with(|l| l.borrow_mut().timers.push(Timer { token: t, period, due: Instant::now() + period, call: Rc::new(RefCell::new(call)) }));
    t
}

/// Removes a descriptor or timer; whether it was registered.
pub fn remove(token: Token) -> bool {
    LOOP.with(|l| {
        let mut l = l.borrow_mut();
        let before = l.fds.len() + l.timers.len();
        l.fds.retain(|f| f.token != token);
        l.timers.retain(|t| t.token != token);
        before != l.fds.len() + l.timers.len()
    })
}

/// Anything registered: the app ticks at frame rate while it is, so editors stay responsive.
pub fn active() -> bool {
    LOOP.with(|l| {
        let l = l.borrow();
        !l.fds.is_empty() || !l.timers.is_empty()
    })
}

fn registered(token: Token) -> bool {
    LOOP.with(|l| {
        let l = l.borrow();
        l.fds.iter().any(|f| f.token == token) || l.timers.iter().any(|t| t.token == token)
    })
}

/// UI thread: runs the callbacks of ready descriptors and due timers. Never blocks.
pub fn dispatch() {
    let watched: Vec<(Token, i32, u32, FdCall)> =
        LOOP.with(|l| l.borrow().fds.iter().map(|f| (f.token, f.fd, f.flags, f.call.clone())).collect());
    if !watched.is_empty() {
        for (token, fd, ready, call) in ready(&watched) {
            // An earlier callback may have removed this one.
            if registered(token) {
                (call.borrow_mut())(fd, ready);
            }
        }
    }
    let now = Instant::now();
    let due: Vec<(Token, TimerCall)> = LOOP.with(|l| {
        let mut l = l.borrow_mut();
        l.timers
            .iter_mut()
            .filter(|t| t.due <= now)
            .map(|t| {
                // Missed periods are dropped, not caught up on.
                t.due = (t.due + t.period).max(now);
                (t.token, t.call.clone())
            })
            .collect()
    });
    for (token, call) in due {
        if registered(token) {
            (call.borrow_mut())();
        }
    }
}

/// The descriptors among `watched` that are ready now, with what they are ready for.
#[cfg(unix)]
fn ready(watched: &[(Token, i32, u32, FdCall)]) -> Vec<(Token, i32, u32, FdCall)> {
    let mut polls: Vec<libc::pollfd> = watched
        .iter()
        .map(|&(_, fd, flags, _)| {
            let mut events = 0;
            if flags & READ != 0 {
                events |= libc::POLLIN;
            }
            if flags & WRITE != 0 {
                events |= libc::POLLOUT;
            }
            libc::pollfd { fd, events, revents: 0 }
        })
        .collect();
    // SAFETY: `polls` is a valid array of `pollfd` for its length; a zero timeout never blocks.
    let n = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, 0) };
    if n <= 0 {
        return Vec::new();
    }
    watched
        .iter()
        .zip(&polls)
        .filter_map(|((token, fd, flags, call), p)| {
            let mut got = 0;
            if p.revents & libc::POLLIN != 0 {
                got |= READ;
            }
            if p.revents & libc::POLLOUT != 0 {
                got |= WRITE;
            }
            if p.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                got |= ERROR;
            }
            // Errors are always reported; read / write only when asked for.
            let got = got & (flags | ERROR);
            (got != 0).then(|| (*token, *fd, got, call.clone()))
        })
        .collect()
}

#[cfg(not(unix))]
fn ready(_: &[(Token, i32, u32, FdCall)]) -> Vec<(Token, i32, u32, FdCall)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_run_when_due_and_stop_when_removed() {
        let n = Rc::new(Cell::new(0));
        let c = n.clone();
        let t = add_timer(Duration::from_millis(30), move || c.set(c.get() + 1));
        assert!(active());
        dispatch();
        assert_eq!(n.get(), 0, "not due yet");
        std::thread::sleep(Duration::from_millis(40));
        dispatch();
        assert_eq!(n.get(), 1);
        assert!(remove(t));
        std::thread::sleep(Duration::from_millis(40));
        dispatch();
        assert_eq!(n.get(), 1);
        assert!(!remove(t));
    }

    #[cfg(unix)]
    #[test]
    fn readable_descriptors_are_reported_and_may_remove_themselves() {
        let mut fds = [0i32; 2];
        // SAFETY: `fds` holds room for the two descriptors pipe() writes.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (r, w) = (fds[0], fds[1]);
        let seen = Rc::new(Cell::new(0u32));
        let slot: Rc<Cell<Token>> = Rc::new(Cell::new(0));
        let (s, me) = (seen.clone(), slot.clone());
        slot.set(add_fd(r, READ, move |_, got| {
            s.set(s.get() | got);
            remove(me.get());
        }));
        dispatch();
        assert_eq!(seen.get(), 0, "nothing written yet");
        // SAFETY: writes one byte from a valid buffer to the pipe's write end.
        assert_eq!(unsafe { libc::write(w, b"x".as_ptr().cast(), 1) }, 1);
        dispatch();
        assert_eq!(seen.get(), READ);
        assert!(!remove(slot.get()), "the callback removed itself");
        // SAFETY: closing the pipe's own descriptors.
        unsafe {
            libc::close(r);
            libc::close(w);
        }
    }
}
