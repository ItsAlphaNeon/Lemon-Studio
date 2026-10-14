// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The host-side COM objects a VST3 plug-in talks to: host application (and the messages / attribute lists it creates
//! for the plug-in), component handler, plug frame, memory stream, event list and parameter changes.
//!
//! Each object implements one interface (its first vtable), so a pointer to the object is that interface's pointer.
// spec: plugins/vst3-hosting.md (§4 setup, §5 processing, §6 editor, §7 parameters, §8 state)
// The binding's COM macros end statements with `;` inside expressions.
#![allow(semicolon_in_expressions_from_non_local_macros)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use vst3_sys as vst3_com;
use vst3_sys::base::{kInvalidArgument, kNoInterface, kResultFalse, kResultOk, tresult, FIDString, IBStream, IUnknown};
use vst3_sys::gui::{IPlugFrame, IPlugView, ViewRect};
#[cfg(target_os = "linux")]
use vst3_sys::gui::linux::{FileDescriptor, IEventHandler, IRunLoop, ITimerHandler, TimerInterval};
use vst3_sys::utils::{SharedVstPtr, StaticVstPtr};
use vst3_sys::vst::{
    Event, IAttributeList, IComponentHandler, IEventList, IHostApplication, IMessage, IParamValueQueue, IParameterChanges,
};
use vst3_sys::{c_void, ComInterface, IID, VST3};

/// One reference to a host object we created: released when dropped.
pub struct Com<T: IUnknown> {
    ptr: *mut T,
}

impl<T: IUnknown> Com<T> {
    /// Takes the object `allocate` returned (reference count 1).
    pub fn new(b: Box<T>) -> Com<T> {
        Com { ptr: Box::into_raw(b) }
    }

    pub fn get(&self) -> &T {
        // SAFETY: we hold a reference, so the object lives.
        unsafe { &*self.ptr }
    }

    /// The object as a pointer to its (first) interface.
    pub fn raw(&self) -> *mut c_void {
        self.ptr as *mut c_void
    }

    /// The object as interface `I` for a method parameter (shared: the callee adds its own reference).
    pub fn shared<I: ComInterface + ?Sized>(&self) -> SharedVstPtr<I> {
        // SAFETY: `SharedVstPtr` is a transparent wrapper of the interface pointer; our objects implement their one
        // interface first.
        unsafe { std::mem::transmute_copy::<*mut c_void, SharedVstPtr<I>>(&self.raw()) }
    }

    /// The object as interface `I` for a struct field (no reference handed over).
    pub fn static_ptr<I: ComInterface + ?Sized>(&self) -> StaticVstPtr<I> {
        // SAFETY: as `shared`.
        unsafe { std::mem::transmute_copy::<*mut c_void, StaticVstPtr<I>>(&self.raw()) }
    }
}

impl<T: IUnknown> Drop for Com<T> {
    fn drop(&mut self) {
        unsafe { (*self.ptr).release() };
    }
}

/// A null interface parameter.
pub fn null_shared<I: ComInterface + ?Sized>() -> SharedVstPtr<I> {
    // SAFETY: a transparent wrapper of a (here null) pointer.
    unsafe { std::mem::transmute_copy::<*mut c_void, SharedVstPtr<I>>(&std::ptr::null_mut()) }
}

/// A null interface field.
pub fn null_static<I: ComInterface + ?Sized>() -> StaticVstPtr<I> {
    // SAFETY: as `null_shared`.
    unsafe { std::mem::transmute_copy::<*mut c_void, StaticVstPtr<I>>(&std::ptr::null_mut()) }
}

/// Writes `s` into a `String128` (UTF-16, NUL terminated, cut to fit).
pub unsafe fn write_str128(dst: *mut u16, s: &str) {
    let mut n = 0;
    for c in s.encode_utf16().take(127) {
        unsafe { *dst.add(n) = c };
        n += 1;
    }
    unsafe { *dst.add(n) = 0 };
}

/// A UTF-16 string up to its NUL.
pub fn from_utf16(s: &[i16]) -> String {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end].iter().map(|&c| c as u16).collect::<Vec<u16>>())
}

/// What the plug-in told the host through the component handler and the plug frame (shared with the instance).
#[derive(Default)]
pub struct Shared {
    /// Edits of the plug-in's editor, for the app (`os_host::Plugin::take_param_events`).
    pub events: Mutex<Vec<os_host::ParamEvent>>,
    /// Values for the processor's next block (the plug-in's own edits and the host's).
    pub to_processor: Mutex<Vec<(u32, f64)>>,
    /// `restartComponent` flags not handled yet.
    pub restart: AtomicI32,
    /// Size the editor asked for (`resizeView`).
    pub size: Mutex<Option<(i32, i32)>>,
    /// Raised with everything above and when the audio thread leaves work for the UI thread.
    pub attention: Arc<os_host::Attention>,
}

/// Most plug-in edits kept until the app takes them.
const MAX_EVENTS: usize = 4096;

impl Shared {
    pub fn push_value(&self, id: u32, v: f64) {
        if let Ok(mut q) = self.to_processor.lock() {
            match q.iter_mut().find(|e| e.0 == id) {
                Some(e) => e.1 = v,
                None => q.push((id, v)),
            }
        }
        self.attention.raise();
    }

    fn event(&self, e: os_host::ParamEvent) {
        if let Ok(mut v) = self.events.lock() {
            if v.len() < MAX_EVENTS {
                v.push(e);
            }
        }
        self.attention.raise();
    }
}

// ---- host application ----

#[VST3(implements(IHostApplication))]
pub struct HostApp {}

impl HostApp {
    pub fn create() -> Com<HostApp> {
        Com::new(HostApp::allocate())
    }
}

/// Hands `obj` (just created, one reference) out as interface `iid`, dropping our reference.
unsafe fn hand_out<T: IUnknown>(obj: Box<T>, iid: *const IID, out: *mut *mut c_void) -> tresult {
    let c = Com::new(obj);
    unsafe { c.get().query_interface(iid, out) }
}

impl IHostApplication for HostApp {
    unsafe fn get_name(&self, name: *mut u16) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        unsafe { write_str128(name, "Lemon Studio") };
        kResultOk
    }

    unsafe fn create_instance(&self, cid: *const IID, iid: *const IID, obj: *mut *mut c_void) -> tresult {
        if cid.is_null() || iid.is_null() || obj.is_null() {
            return kInvalidArgument;
        }
        unsafe {
            *obj = std::ptr::null_mut();
            if *cid == <dyn IMessage as ComInterface>::IID {
                return hand_out(Message::allocate(RefCell::new(c"".to_owned()), AttributeList::create()), iid, obj);
            }
            if *cid == <dyn IAttributeList as ComInterface>::IID {
                return hand_out(AttributeList::allocate(RefCell::new(HashMap::new())), iid, obj);
            }
        }
        kNoInterface
    }
}

// ---- messages between a component and its separate controller ----

#[VST3(implements(IMessage))]
pub struct Message {
    id: RefCell<std::ffi::CString>,
    attrs: Com<AttributeList>,
}

impl IMessage for Message {
    unsafe fn get_message_id(&self) -> FIDString {
        self.id.borrow().as_ptr()
    }

    unsafe fn set_message_id(&self, id: FIDString) {
        if !id.is_null() {
            *self.id.borrow_mut() = unsafe { CStr::from_ptr(id) }.to_owned();
        }
    }

    unsafe fn get_attributes(&self) -> StaticVstPtr<dyn IAttributeList> {
        self.attrs.static_ptr()
    }
}

enum Attr {
    Int(i64),
    Float(f64),
    Str(Vec<u16>),
    Bin(Vec<u8>),
}

#[VST3(implements(IAttributeList))]
pub struct AttributeList {
    map: RefCell<HashMap<Vec<u8>, Attr>>,
}

impl AttributeList {
    fn create() -> Com<AttributeList> {
        Com::new(AttributeList::allocate(RefCell::new(HashMap::new())))
    }
}

unsafe fn key(id: *const std::ffi::c_char) -> Option<Vec<u8>> {
    (!id.is_null()).then(|| unsafe { CStr::from_ptr(id) }.to_bytes().to_vec())
}

impl IAttributeList for AttributeList {
    unsafe fn set_int(&self, id: *const std::ffi::c_char, value: i64) -> tresult {
        let Some(k) = (unsafe { key(id) }) else { return kInvalidArgument };
        self.map.borrow_mut().insert(k, Attr::Int(value));
        kResultOk
    }

    unsafe fn get_int(&self, id: *const std::ffi::c_char, value: *mut i64) -> tresult {
        match (unsafe { key(id) }).and_then(|k| match self.map.borrow().get(&k) {
            Some(Attr::Int(v)) => Some(*v),
            _ => None,
        }) {
            Some(v) if !value.is_null() => {
                unsafe { *value = v };
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn set_float(&self, id: *const std::ffi::c_char, value: f64) -> tresult {
        let Some(k) = (unsafe { key(id) }) else { return kInvalidArgument };
        self.map.borrow_mut().insert(k, Attr::Float(value));
        kResultOk
    }

    unsafe fn get_float(&self, id: *const std::ffi::c_char, value: *mut f64) -> tresult {
        match (unsafe { key(id) }).and_then(|k| match self.map.borrow().get(&k) {
            Some(Attr::Float(v)) => Some(*v),
            _ => None,
        }) {
            Some(v) if !value.is_null() => {
                unsafe { *value = v };
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    /// The documented signature has no size (the string is NUL terminated); the binding's extra argument is ignored.
    unsafe fn set_string(&self, id: *const std::ffi::c_char, value: *const i16, _size: u32) -> tresult {
        let Some(k) = (unsafe { key(id) }) else { return kInvalidArgument };
        if value.is_null() {
            return kInvalidArgument;
        }
        let mut s = Vec::new();
        let mut i = 0;
        loop {
            let c = unsafe { *value.add(i) } as u16;
            if c == 0 || i > 1 << 20 {
                break;
            }
            s.push(c);
            i += 1;
        }
        self.map.borrow_mut().insert(k, Attr::Str(s));
        kResultOk
    }

    /// `size` is the buffer size in bytes.
    unsafe fn get_string(&self, id: *const std::ffi::c_char, value: *mut i16, size: u32) -> tresult {
        let map = self.map.borrow();
        let Some(Attr::Str(s)) = (unsafe { key(id) }).and_then(|k| map.get(&k)) else { return kResultFalse };
        let cap = (size as usize / 2).saturating_sub(1);
        if value.is_null() || size < 2 {
            return kInvalidArgument;
        }
        let n = s.len().min(cap);
        for (i, &c) in s[..n].iter().enumerate() {
            unsafe { *value.add(i) = c as i16 };
        }
        unsafe { *value.add(n) = 0 };
        kResultOk
    }

    unsafe fn set_binary(&self, id: *const std::ffi::c_char, ptr: *const c_void, size: u32) -> tresult {
        let Some(k) = (unsafe { key(id) }) else { return kInvalidArgument };
        let data = if ptr.is_null() || size == 0 { Vec::new() } else { unsafe { std::slice::from_raw_parts(ptr as *const u8, size as usize) }.to_vec() };
        self.map.borrow_mut().insert(k, Attr::Bin(data));
        kResultOk
    }

    /// The data stays owned by the list (valid while the attribute is unchanged).
    unsafe fn get_binary(&self, id: *const std::ffi::c_char, ptr: *const *mut c_void, size: *mut u32) -> tresult {
        let map = self.map.borrow();
        let Some(Attr::Bin(b)) = (unsafe { key(id) }).and_then(|k| map.get(&k)) else { return kResultFalse };
        if ptr.is_null() || size.is_null() {
            return kInvalidArgument;
        }
        unsafe {
            *(ptr as *mut *const u8) = b.as_ptr();
            *size = b.len() as u32;
        }
        kResultOk
    }
}

// ---- component handler ----

#[VST3(implements(IComponentHandler))]
pub struct Handler {
    shared: Arc<Shared>,
}

impl Handler {
    pub fn create(shared: Arc<Shared>) -> Com<Handler> {
        Com::new(Handler::allocate(shared))
    }
}

impl IComponentHandler for Handler {
    unsafe fn begin_edit(&self, id: u32) -> tresult {
        self.shared.event(os_host::ParamEvent::Begin(id));
        kResultOk
    }

    unsafe fn perform_edit(&self, id: u32, value: f64) -> tresult {
        self.shared.event(os_host::ParamEvent::Value(id, value));
        // The processor learns every edit of the controller from the host.
        self.shared.push_value(id, value);
        os_host::wake();
        kResultOk
    }

    unsafe fn end_edit(&self, id: u32) -> tresult {
        self.shared.event(os_host::ParamEvent::End(id));
        kResultOk
    }

    unsafe fn restart_component(&self, flags: i32) -> tresult {
        self.shared.restart.fetch_or(flags, Ordering::AcqRel);
        self.shared.attention.raise();
        os_host::wake();
        kResultOk
    }
}

// ---- plug frame (editor resizes; on Linux the editor's run loop) ----

#[cfg_attr(target_os = "linux", VST3(implements(IPlugFrame, IRunLoop)))]
#[cfg_attr(not(target_os = "linux"), VST3(implements(IPlugFrame)))]
pub struct Frame {
    shared: Arc<Shared>,
    /// What the editor registered with the run loop (Linux): its handler or timer object and the run loop's token.
    run: RefCell<Vec<(usize, u64)>>,
}

impl Frame {
    pub fn create(shared: Arc<Shared>) -> Com<Frame> {
        Com::new(Frame::allocate(shared, RefCell::new(Vec::new())))
    }

    /// Drops what a closed editor left registered with the run loop (it should have unregistered in `removed`).
    pub fn clear_run_loop(&self) {
        for (_, token) in self.run.borrow_mut().drain(..) {
            #[cfg(target_os = "linux")]
            os_host::runloop::remove(token);
            #[cfg(not(target_os = "linux"))]
            let _ = token;
        }
    }

    /// Removes the run loop entries of the handler or timer object at `key`.
    #[cfg(target_os = "linux")]
    fn unregister(&self, key: usize) {
        self.run.borrow_mut().retain(|&(k, token)| {
            if k == key {
                os_host::runloop::remove(token);
            }
            k != key
        });
    }
}

/// Linux: plug-in editors have no message loop of their own; they hand the host their X11 connection's descriptor and
/// their timers (spec §6), and get the callbacks on the UI thread (`os_host::runloop`).
#[cfg(target_os = "linux")]
impl IRunLoop for Frame {
    unsafe fn register_event_handler(&self, h: SharedVstPtr<dyn IEventHandler>, fd: FileDescriptor) -> tresult {
        let Some(handler) = h.upgrade() else { return kInvalidArgument };
        let key = { h }.as_ptr() as usize;
        let token = os_host::runloop::add_fd(fd, os_host::runloop::READ, move |fd, _| unsafe { handler.on_fd_is_set(fd) });
        self.run.borrow_mut().push((key, token));
        kResultOk
    }

    unsafe fn unregister_event_handler(&self, h: SharedVstPtr<dyn IEventHandler>) -> tresult {
        self.unregister({ h }.as_ptr() as usize);
        kResultOk
    }

    unsafe fn register_timer(&self, t: SharedVstPtr<dyn ITimerHandler>, ms: TimerInterval) -> tresult {
        let Some(timer) = t.upgrade() else { return kInvalidArgument };
        let key = { t }.as_ptr() as usize;
        let token = os_host::runloop::add_timer(std::time::Duration::from_millis(ms), move || unsafe { timer.on_timer() });
        self.run.borrow_mut().push((key, token));
        kResultOk
    }

    unsafe fn unregister_timer(&self, t: SharedVstPtr<dyn ITimerHandler>) -> tresult {
        self.unregister({ t }.as_ptr() as usize);
        kResultOk
    }
}

impl IPlugFrame for Frame {
    unsafe fn resize_view(&self, view: SharedVstPtr<dyn IPlugView>, new_size: *mut ViewRect) -> tresult {
        if new_size.is_null() {
            return kInvalidArgument;
        }
        let r = unsafe { *new_size };
        if let Ok(mut s) = self.shared.size.lock() {
            *s = Some((r.right - r.left, r.bottom - r.top));
        }
        self.shared.attention.raise();
        // The host frame follows on its next tick; the view hears its new size now.
        if let Some(v) = view.upgrade() {
            unsafe { v.on_size(new_size) };
        }
        os_host::wake();
        kResultOk
    }
}

// ---- memory stream (state) ----

#[VST3(implements(IBStream))]
pub struct Stream {
    data: RefCell<Vec<u8>>,
    pos: Cell<usize>,
}

impl Stream {
    pub fn create(data: Vec<u8>) -> Com<Stream> {
        Com::new(Stream::allocate(RefCell::new(data), Cell::new(0)))
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.data.borrow().clone()
    }

    pub fn rewind(&self) {
        self.pos.set(0);
    }
}

impl IBStream for Stream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: i32, num_bytes_read: *mut i32) -> tresult {
        if buffer.is_null() || num_bytes < 0 {
            return kInvalidArgument;
        }
        let data = self.data.borrow();
        let pos = self.pos.get().min(data.len());
        let n = (num_bytes as usize).min(data.len() - pos);
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr().add(pos), buffer as *mut u8, n) };
        self.pos.set(pos + n);
        if !num_bytes_read.is_null() {
            unsafe { *num_bytes_read = n as i32 };
        }
        kResultOk
    }

    unsafe fn write(&self, buffer: *const c_void, num_bytes: i32, num_bytes_written: *mut i32) -> tresult {
        if buffer.is_null() || num_bytes < 0 {
            return kInvalidArgument;
        }
        let mut data = self.data.borrow_mut();
        let (pos, n) = (self.pos.get(), num_bytes as usize);
        if data.len() < pos + n {
            data.resize(pos + n, 0);
        }
        unsafe { std::ptr::copy_nonoverlapping(buffer as *const u8, data.as_mut_ptr().add(pos), n) };
        self.pos.set(pos + n);
        if !num_bytes_written.is_null() {
            unsafe { *num_bytes_written = n as i32 };
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: i64, mode: i32, result: *mut i64) -> tresult {
        let len = self.data.borrow().len() as i64;
        let base = match mode {
            0 => 0,
            1 => self.pos.get() as i64,
            2 => len,
            _ => return kInvalidArgument,
        };
        let to = (base + pos).max(0);
        self.pos.set(to as usize);
        if !result.is_null() {
            unsafe { *result = to };
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut i64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        unsafe { *pos = self.pos.get() as i64 };
        kResultOk
    }
}

// ---- event list (notes into the processor) ----

#[VST3(implements(IEventList))]
pub struct EventList {
    events: RefCell<Vec<Event>>,
}

impl EventList {
    pub fn create() -> Com<EventList> {
        Com::new(EventList::allocate(RefCell::new(Vec::with_capacity(256))))
    }

    pub fn clear(&self) {
        self.events.borrow_mut().clear();
    }

    pub fn push(&self, e: Event) {
        self.events.borrow_mut().push(e);
    }
}

impl IEventList for EventList {
    unsafe fn get_event_count(&self) -> i32 {
        self.events.borrow().len() as i32
    }

    unsafe fn get_event(&self, index: i32, e: *mut Event) -> tresult {
        match self.events.borrow().get(index as usize) {
            Some(ev) if !e.is_null() => {
                unsafe { *e = *ev };
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn add_event(&self, e: *mut Event) -> tresult {
        if e.is_null() {
            return kInvalidArgument;
        }
        self.events.borrow_mut().push(unsafe { *e });
        kResultOk
    }
}

// ---- parameter changes (host -> processor, processor -> host) ----

#[VST3(implements(IParamValueQueue))]
pub struct ParamQueue {
    id: Cell<u32>,
    points: RefCell<Vec<(i32, f64)>>,
}

impl IParamValueQueue for ParamQueue {
    unsafe fn get_parameter_id(&self) -> u32 {
        self.id.get()
    }

    unsafe fn get_point_count(&self) -> i32 {
        self.points.borrow().len() as i32
    }

    unsafe fn get_point(&self, index: i32, sample_offset: *mut i32, value: *mut f64) -> tresult {
        match self.points.borrow().get(index as usize) {
            Some(&(o, v)) if !sample_offset.is_null() && !value.is_null() => {
                unsafe {
                    *sample_offset = o;
                    *value = v;
                }
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn add_point(&self, sample_offset: i32, value: f64, index: *mut i32) -> tresult {
        let mut p = self.points.borrow_mut();
        // Points are kept in time order; a second point at the same offset replaces the first.
        let i = match p.iter().position(|q| q.0 >= sample_offset) {
            Some(i) if p[i].0 == sample_offset => {
                p[i].1 = value;
                i
            }
            Some(i) => {
                p.insert(i, (sample_offset, value));
                i
            }
            None => {
                p.push((sample_offset, value));
                p.len() - 1
            }
        };
        if !index.is_null() {
            unsafe { *index = i as i32 };
        }
        kResultOk
    }
}

#[VST3(implements(IParameterChanges))]
pub struct ParamChanges {
    /// Queue objects, reused block after block; the first `used` are this block's.
    queues: RefCell<Vec<Com<ParamQueue>>>,
    used: Cell<usize>,
}

impl ParamChanges {
    pub fn create() -> Com<ParamChanges> {
        Com::new(ParamChanges::allocate(RefCell::new(Vec::new()), Cell::new(0)))
    }

    pub fn clear(&self) {
        for q in self.queues.borrow().iter().take(self.used.get()) {
            q.get().points.borrow_mut().clear();
        }
        self.used.set(0);
    }

    /// Adds a point (host side).
    pub fn add(&self, id: u32, offset: i32, value: f64) {
        let mut index = 0;
        let q = unsafe { self.add_parameter_data(&id, &mut index) };
        if let Some(q) = q.upgrade() {
            let mut i = 0;
            unsafe { q.add_point(offset, value, &mut i) };
        }
    }

    /// Each changed parameter's last value (what the processor reported), without allocating.
    pub fn for_each_last(&self, mut f: impl FnMut(u32, f64)) {
        let qs = self.queues.borrow();
        for q in qs.iter().take(self.used.get()) {
            if let Some(&(_, v)) = q.get().points.borrow().last() {
                f(q.get().id.get(), v);
            }
        }
    }
}

impl IParameterChanges for ParamChanges {
    unsafe fn get_parameter_count(&self) -> i32 {
        self.used.get() as i32
    }

    unsafe fn get_parameter_data(&self, index: i32) -> StaticVstPtr<dyn IParamValueQueue> {
        let qs = self.queues.borrow();
        match qs.get(index as usize).filter(|_| (index as usize) < self.used.get()) {
            Some(q) => q.static_ptr(),
            None => null_static(),
        }
    }

    unsafe fn add_parameter_data(&self, id: *const u32, index: *mut i32) -> StaticVstPtr<dyn IParamValueQueue> {
        if id.is_null() {
            return null_static();
        }
        let id = unsafe { *id };
        let used = self.used.get();
        let mut qs = self.queues.borrow_mut();
        let i = match qs.iter().take(used).position(|q| q.get().id.get() == id) {
            Some(i) => i,
            None => {
                if qs.len() == used {
                    // Room for an automation point every 64 frames of a full block.
                    qs.push(Com::new(ParamQueue::allocate(Cell::new(0), RefCell::new(Vec::with_capacity(72)))));
                }
                qs[used].get().id.set(id);
                qs[used].get().points.borrow_mut().clear();
                self.used.set(used + 1);
                used
            }
        };
        if !index.is_null() {
            unsafe { *index = i as i32 };
        }
        qs[i].static_ptr()
    }
}
