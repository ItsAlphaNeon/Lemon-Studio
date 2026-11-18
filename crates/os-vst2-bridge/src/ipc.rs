// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A channel between the host and a helper: one shared mapping (a header and a payload) and two auto-reset events, one
//! each way. Messages alternate: a side writes a request and signals; the other answers with a reply, or first with
//! requests of its own (a plugin calling back while the host's call is running), each answered in turn. So one buffer
//! serves the whole exchange, and a side reads a message's payload before it writes anything else.
//!
// spec: plugins/vst2-bridge.md §2

use std::ffi::c_void;
use std::sync::Mutex;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
    WaitForMultipleObjects,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_QS_SENDMESSAGE,
    PM_REMOVE, PeekMessageW, QS_ALLINPUT, QS_SENDMESSAGE, TranslateMessage,
};

use crate::proto::{DATA, HEADER_SPACE, Header, MAPPING, kind};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Which end of a channel this process is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Host,
    Helper,
}

/// What a wait lets through meanwhile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pump {
    /// Nothing (the audio thread).
    None,
    /// Messages other threads and processes send to this thread's windows, which they wait on: a plugin editor in one
    /// process and its parent window in the other send each other such messages.
    Sent,
    /// Every message (the helper's window thread while idle: the plugin's editor runs on it).
    All,
}

/// Why a wait ended without a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gone {
    /// The other process ended.
    Peer,
    Timeout,
}

/// A payload the sender keeps in a mapping of its own because it is larger than the channel's.
struct Big {
    map: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl Drop for Big {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
            CloseHandle(self.map);
        }
    }
}

pub struct Channel {
    name: String,
    side: Side,
    map: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    to_host: HANDLE,
    to_helper: HANDLE,
    /// The large payload last sent, kept until the next message is written.
    big: Mutex<Option<Big>>,
}

// SAFETY: the handles and the view are process-wide; who writes the shared memory when is the protocol's (one message
// at a time, each side's threads behind the channel's lock).
unsafe impl Send for Channel {}
unsafe impl Sync for Channel {}

impl Channel {
    /// The host makes the channel `name` (mapping and events).
    pub fn create(name: &str) -> Result<Channel, String> {
        unsafe {
            let map = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                MAPPING as u32,
                wide(name).as_ptr(),
            );
            if map.is_null() {
                return Err(format!(
                    "cannot make shared memory {name}: error {}",
                    GetLastError()
                ));
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(map);
                return Err(format!("shared memory {name} already exists"));
            }
            let ev = |suffix: &str| {
                CreateEventW(
                    std::ptr::null(),
                    0,
                    0,
                    wide(&format!("{name}.{suffix}")).as_ptr(),
                )
            };
            Self::finish(name, Side::Host, map, ev("to_host"), ev("to_helper"))
        }
    }

    /// The helper opens the channel `name` the host made.
    pub fn open(name: &str) -> Result<Channel, String> {
        unsafe {
            let map = OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, wide(name).as_ptr());
            if map.is_null() {
                return Err(format!(
                    "cannot open shared memory {name}: error {}",
                    GetLastError()
                ));
            }
            let ev = |suffix: &str| {
                OpenEventW(
                    EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
                    0,
                    wide(&format!("{name}.{suffix}")).as_ptr(),
                )
            };
            Self::finish(name, Side::Helper, map, ev("to_host"), ev("to_helper"))
        }
    }

    unsafe fn finish(
        name: &str,
        side: Side,
        map: HANDLE,
        to_host: HANDLE,
        to_helper: HANDLE,
    ) -> Result<Channel, String> {
        unsafe {
            if to_host.is_null() || to_helper.is_null() {
                for h in [map, to_host, to_helper] {
                    if !h.is_null() {
                        CloseHandle(h);
                    }
                }
                return Err(format!("cannot make or open the events of {name}"));
            }
            let view = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, MAPPING);
            if view.Value.is_null() {
                CloseHandle(map);
                CloseHandle(to_host);
                CloseHandle(to_helper);
                return Err(format!("cannot map {name}"));
            }
            Ok(Channel {
                name: name.to_string(),
                side,
                map,
                view,
                to_host,
                to_helper,
                big: Mutex::new(None),
            })
        }
    }

    fn header(&self) -> *mut Header {
        self.view.Value as *mut Header
    }

    fn data(&self) -> *mut u8 {
        unsafe { (self.view.Value as *mut u8).add(HEADER_SPACE) }
    }

    /// The event this side waits on.
    pub fn mine(&self) -> HANDLE {
        if self.side == Side::Host {
            self.to_host
        } else {
            self.to_helper
        }
    }

    fn signal_peer(&self) {
        unsafe {
            SetEvent(if self.side == Side::Host {
                self.to_helper
            } else {
                self.to_host
            })
        };
    }

    fn big_name(&self, side: Side) -> String {
        format!(
            "{}.big.{}",
            self.name,
            if side == Side::Host { "host" } else { "helper" }
        )
    }

    /// Writes message `h` with `payload` and signals the other side.
    pub fn send(&self, mut h: Header, payload: &[u8]) {
        let mut big = self.big.lock().unwrap_or_else(|e| e.into_inner());
        *big = None;
        h.len = payload.len() as u64;
        h.big = 0;
        unsafe {
            if payload.len() <= DATA {
                std::ptr::copy_nonoverlapping(payload.as_ptr(), self.data(), payload.len());
            } else {
                let name = wide(&self.big_name(self.side));
                let size = payload.len() as u64;
                let map = CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    std::ptr::null(),
                    PAGE_READWRITE,
                    (size >> 32) as u32,
                    size as u32,
                    name.as_ptr(),
                );
                let view = if map.is_null() {
                    MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: std::ptr::null_mut(),
                    }
                } else {
                    MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, payload.len())
                };
                if view.Value.is_null() {
                    if !map.is_null() {
                        CloseHandle(map);
                    }
                    // Nothing to send it in: the other side gets an empty payload.
                    h.len = 0;
                } else {
                    std::ptr::copy_nonoverlapping(
                        payload.as_ptr(),
                        view.Value as *mut u8,
                        payload.len(),
                    );
                    h.big = 1;
                    *big = Some(Big { map, view });
                }
            }
            self.header().write_volatile(h);
        }
        drop(big);
        self.signal_peer();
    }

    /// The message the other side wrote (after a signal): its header and payload.
    pub fn read(&self) -> (Header, Vec<u8>) {
        let h = unsafe { self.header().read_volatile() };
        let len = h.len as usize;
        let payload = if h.big == 1 {
            let other = if self.side == Side::Host {
                Side::Helper
            } else {
                Side::Host
            };
            unsafe {
                let map =
                    OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, wide(&self.big_name(other)).as_ptr());
                if map.is_null() {
                    Vec::new()
                } else {
                    let view = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, len);
                    let v = if view.Value.is_null() {
                        Vec::new()
                    } else {
                        std::slice::from_raw_parts(view.Value as *const u8, len).to_vec()
                    };
                    if !view.Value.is_null() {
                        UnmapViewOfFile(view);
                    }
                    CloseHandle(map);
                    v
                }
            }
        } else {
            let len = len.min(DATA);
            unsafe { std::slice::from_raw_parts(self.data(), len).to_vec() }
        };
        (h, payload)
    }

    /// Waits for the other side's next message (`peer`: its process, whose end ends the wait).
    pub fn wait(&self, peer: HANDLE, pump: Pump, timeout: Option<Duration>) -> Result<(), Gone> {
        wait_any(&[self.mine(), peer], pump, timeout)
            .and_then(|i| if i == 0 { Ok(()) } else { Err(Gone::Peer) })
    }

    /// Sends request `h` with `payload` and waits for its reply, answering the other side's requests in the meantime
    /// with `serve` (which may itself call through this channel: those calls nest).
    pub fn call(
        &self,
        h: Header,
        payload: &[u8],
        peer: HANDLE,
        pump: Pump,
        timeout: Option<Duration>,
        serve: &mut dyn FnMut(&Header, Vec<u8>) -> (Header, Vec<u8>),
    ) -> Result<(Header, Vec<u8>), Gone> {
        self.call_with(h, payload, peer, pump, timeout, true, serve)
    }

    /// [`Channel::call`], with the reply's payload read only when `reply_payload` (else an empty one).
    #[allow(clippy::too_many_arguments)]
    pub fn call_with(
        &self,
        mut h: Header,
        payload: &[u8],
        peer: HANDLE,
        pump: Pump,
        timeout: Option<Duration>,
        reply_payload: bool,
        serve: &mut dyn FnMut(&Header, Vec<u8>) -> (Header, Vec<u8>),
    ) -> Result<(Header, Vec<u8>), Gone> {
        h.kind = kind::REQUEST;
        self.send(h, payload);
        loop {
            self.wait(peer, pump, timeout)?;
            let got = unsafe { self.header().read_volatile() };
            if got.kind == kind::REPLY {
                return Ok(if reply_payload {
                    self.read()
                } else {
                    (got, Vec::new())
                });
            }
            let (got, data) = self.read();
            let (mut r, out) = serve(&got, data);
            r.kind = kind::REPLY;
            self.send(r, &out);
        }
    }

    /// Answers the request just read with `h` and `payload`.
    pub fn reply(&self, mut h: Header, payload: &[u8]) {
        h.kind = kind::REPLY;
        self.send(h, payload);
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        *self.big.get_mut().unwrap_or_else(|e| e.into_inner()) = None;
        unsafe {
            UnmapViewOfFile(self.view);
            CloseHandle(self.map);
            CloseHandle(self.to_host);
            CloseHandle(self.to_helper);
        }
    }
}

/// Waits for one of `handles` (null ones skipped), letting `pump`'s messages through; the index of the one signalled.
pub fn wait_any(handles: &[HANDLE], pump: Pump, timeout: Option<Duration>) -> Result<usize, Gone> {
    let hs: Vec<HANDLE> = handles.iter().copied().filter(|h| !h.is_null()).collect();
    let map: Vec<usize> = handles
        .iter()
        .enumerate()
        .filter(|(_, h)| !h.is_null())
        .map(|(i, _)| i)
        .collect();
    let deadline = timeout.map(|t| std::time::Instant::now() + t);
    loop {
        let ms = match deadline {
            None => u32::MAX,
            Some(d) => d
                .saturating_duration_since(std::time::Instant::now())
                .as_millis()
                .min(u32::MAX as u128 - 1) as u32,
        };
        let r = unsafe {
            match pump {
                Pump::None => WaitForMultipleObjects(hs.len() as u32, hs.as_ptr(), 0, ms),
                Pump::Sent => MsgWaitForMultipleObjectsEx(
                    hs.len() as u32,
                    hs.as_ptr(),
                    ms,
                    QS_SENDMESSAGE,
                    MWMO_INPUTAVAILABLE,
                ),
                Pump::All => MsgWaitForMultipleObjectsEx(
                    hs.len() as u32,
                    hs.as_ptr(),
                    ms,
                    QS_ALLINPUT,
                    MWMO_INPUTAVAILABLE,
                ),
            }
        };
        if r == WAIT_TIMEOUT {
            return Err(Gone::Timeout);
        }
        let i = r.wrapping_sub(WAIT_OBJECT_0) as usize;
        if i < hs.len() {
            return Ok(map[i]);
        }
        if i == hs.len() {
            pump_messages(pump);
            continue;
        }
        // An abandoned or failed wait: the handles are gone.
        return Err(Gone::Peer);
    }
}

/// Handles this thread's waiting messages: the sent ones only (`Pump::Sent`), or all.
pub fn pump_messages(pump: Pump) {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        match pump {
            Pump::None => {}
            // Peeking for sent messages delivers them; nothing is removed from the queue.
            Pump::Sent => {
                PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_QS_SENDMESSAGE);
            }
            Pump::All => {
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

/// An empty pointer value of the right type for unused fields.
pub fn null() -> *mut c_void {
    std::ptr::null_mut()
}
