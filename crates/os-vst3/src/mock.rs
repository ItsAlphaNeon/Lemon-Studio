// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! A test plug-in built into the test binary: a gain effect whose edit controller is a separate class, the layout most
//! VST 3 plug-ins use (Lemon Studio's own bundles are single-component). It covers what those bundles do not reach:
//! a separate controller (connection points, setComponentState, its own state), messages created by the host,
//! parameter output of the processor, restart requests and the teardown order.
// spec: plugins/vst3-hosting.md
// The binding's COM macros end statements with `;` inside expressions.
#![allow(semicolon_in_expressions_from_non_local_macros)]

use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use vst3_sys as vst3_com;
use vst3_sys::base::{
    kInvalidArgument, kNoInterface, kNotImplemented, kResultFalse, kResultOk, tresult, FIDString, IBStream, IPluginBase, IPluginFactory,
    IPluginFactory2, IUnknown, PClassInfo, PClassInfo2, PFactoryInfo, TBool,
};
use vst3_sys::utils::SharedVstPtr;
use vst3_sys::vst::{
    BusDirection, BusInfo, IAttributeList, IAudioProcessor, IComponent, IComponentHandler, IConnectionPoint, IEditController, IHostApplication,
    IMessage, IParamValueQueue, IParameterChanges, IoMode, MediaType, ParameterInfo, ProcessData, ProcessSetup, RoutingInfo, SpeakerArrangement, TChar,
};
use vst3_sys::{c_void, ComInterface, VstPtr, IID, VST3};

pub const COMPONENT_CID: IID = IID { data: *b"OSTestGainComp\0\x01" };
pub const CONTROLLER_CID: IID = IID { data: *b"OSTestGainCtrl\0\x02" };

/// Parameters: gain (-24..+24 dB), mode (three choices), peak (read only, reported by the processor), and an extra
/// one that appears after a "titles changed" restart.
pub const GAIN: u32 = 7;
pub const MODE: u32 = 8;
pub const PEAK: u32 = 9;
pub const EXTRA: u32 = 10;

/// What the test plug-in saw, shared with the test.
#[derive(Default)]
pub struct Probe {
    /// Calls in order ("comp.initialize", "ctrl.set_component_state 0.5", ...).
    pub log: Mutex<Vec<String>>,
    /// Plug-in objects alive.
    pub live: AtomicI32,
    pub latency: AtomicU32,
    /// The controller has the extra parameter.
    pub extra: AtomicBool,
    /// Gain points the processor received (offset, value), in order.
    pub gain_points: Mutex<Vec<(i32, f64)>>,
    /// The controller object, for simulating edits in its editor.
    pub controller: AtomicUsize,
    /// Each process call's context: (state, tempo, project time in quarter notes, in samples, bar start, cycle end).
    pub contexts: Mutex<Vec<(u32, f64, f64, i64, f64, f64)>>,
}

impl Probe {
    fn log(&self, s: impl Into<String>) {
        self.log.lock().unwrap().push(s.into());
    }

    pub fn logged(&self, prefix: &str) -> Vec<String> {
        self.log.lock().unwrap().iter().filter(|l| l.starts_with(prefix)).cloned().collect()
    }

    /// The controller (valid while the instance lives).
    pub fn controller(&self) -> &Controller {
        let p = self.controller.load(Ordering::Acquire);
        assert_ne!(p, 0, "no controller yet");
        // SAFETY: set when the controller was created; tests use it only while the instance holds it.
        unsafe { &*(p as *const Controller) }
    }
}

/// Counts a plug-in object as alive until it is freed.
pub struct Live(Arc<Probe>);

impl Live { // This code is a part of L e m o n S t u d i o. Please don't upload it to Art Craft!
    fn new(p: &Arc<Probe>) -> Live {
        p.live.fetch_add(1, Ordering::AcqRel);
        Live(p.clone())
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::AcqRel);
    }
}

fn copy_c(dst: &mut [std::ffi::c_char], s: &str) {
    for (d, b) in dst.iter_mut().zip(s.bytes().chain(std::iter::once(0))) {
        *d = b as std::ffi::c_char;
    }
}

fn copy_w(dst: &mut [TChar], s: &str) {
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().chain(std::iter::once(0))) {
        *d = c as TChar;
    }
}

unsafe fn read_f64(s: &SharedVstPtr<dyn IBStream>) -> Option<f64> {
    let s = s.upgrade()?;
    let mut b = [0u8; 8];
    let mut n = 0;
    (unsafe { s.read(b.as_mut_ptr() as *mut c_void, 8, &mut n) } == kResultOk && n == 8).then(|| f64::from_le_bytes(b))
}

unsafe fn write_bytes(s: &SharedVstPtr<dyn IBStream>, b: &[u8]) -> tresult {
    let Some(s) = s.upgrade() else { return kInvalidArgument };
    let mut n = 0;
    unsafe { s.write(b.as_ptr() as *const c_void, b.len() as i32, &mut n) }
}

/// Hands a new object out as interface `iid`, dropping our reference.
unsafe fn hand_out<T: IUnknown>(obj: Box<T>, iid: *const IID, out: *mut *mut c_void) -> tresult {
    let raw = Box::into_raw(obj);
    unsafe {
        let r = (*raw).query_interface(iid, out);
        (*raw).release();
        if r == kResultOk { kResultOk } else { kNoInterface }
    }
}

// ---- factory ----

#[VST3(implements(IPluginFactory2, IPluginFactory))]
pub struct Factory {
    probe: Arc<Probe>,
}

impl Factory {
    /// A factory pointer as `GetPluginFactory` returns it (one reference for the caller).
    pub fn create(probe: &Arc<Probe>) -> *mut c_void {
        Box::into_raw(Factory::allocate(probe.clone())) as *mut c_void
    }
}

impl IPluginFactory for Factory {
    unsafe fn get_factory_info(&self, info: *mut PFactoryInfo) -> tresult {
        let info = unsafe { &mut *info };
        copy_c(&mut info.vendor, "Test Vendor");
        kResultOk
    }

    unsafe fn count_classes(&self) -> i32 {
        2
    }

    unsafe fn get_class_info(&self, index: i32, info: *mut PClassInfo) -> tresult {
        let info = unsafe { &mut *info };
        let (cid, cat, name) = match index {
            0 => (COMPONENT_CID, "Audio Module Class", "Test Gain"),
            1 => (CONTROLLER_CID, "Component Controller Class", "Test Gain Controller"),
            _ => return kInvalidArgument,
        };
        info.cid = cid;
        info.cardinality = 0x7FFF_FFFF;
        copy_c(&mut info.category, cat);
        copy_c(&mut info.name, name);
        kResultOk
    }

    unsafe fn create_instance(&self, cid: *const IID, iid: *const IID, obj: *mut *mut c_void) -> tresult {
        unsafe {
            *obj = std::ptr::null_mut();
            if *cid == COMPONENT_CID {
                return hand_out(Component::new(&self.probe), iid, obj);
            }
            if *cid == CONTROLLER_CID {
                let c = Controller::new(&self.probe);
                self.probe.controller.store(&*c as *const Controller as usize, Ordering::Release);
                return hand_out(c, iid, obj);
            }
        }
        kNoInterface
    }
}

impl IPluginFactory2 for Factory {
    unsafe fn get_class_info2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        let info = unsafe { &mut *info };
        let (cid, cat, name, sub) = match index {
            0 => (COMPONENT_CID, "Audio Module Class", "Test Gain", "Fx|Dynamics"),
            1 => (CONTROLLER_CID, "Component Controller Class", "Test Gain Controller", ""),
            _ => return kInvalidArgument,
        };
        info.cid = cid;
        info.cardinality = 0x7FFF_FFFF;
        copy_c(&mut info.category, cat);
        copy_c(&mut info.name, name);
        copy_c(&mut info.subcategories, sub);
        copy_c(&mut info.version, "1.2.3");
        kResultOk
    }
}

// ---- component (processor) ----

#[VST3(implements(IComponent, IAudioProcessor, IConnectionPoint))]
pub struct Component {
    probe: Arc<Probe>,
    host: RefCell<Option<VstPtr<dyn IHostApplication>>>,
    peer: RefCell<Option<VstPtr<dyn IConnectionPoint>>>,
    /// Normalized gain: 0.5 = 0 dB, output = input * 2 * gain.
    gain: Cell<f64>,
    _live: Live,
}

impl Component {
    fn new(probe: &Arc<Probe>) -> Box<Component> {
        Component::allocate(probe.clone(), RefCell::new(None), RefCell::new(None), Cell::new(0.5), Live::new(probe))
    }

    /// Tells the controller something through a message the host creates.
    unsafe fn send(&self, id: &CStr, latency: i64) {
        let (Some(host), Some(peer)) = (self.host.borrow().clone(), self.peer.borrow().clone()) else { return };
        let mut obj: *mut c_void = std::ptr::null_mut();
        unsafe {
            if host.create_instance(&<dyn IMessage as ComInterface>::IID, &<dyn IMessage as ComInterface>::IID, &mut obj) != kResultOk {
                return;
            }
            let Some(msg) = VstPtr::<dyn IMessage>::owned(obj as *mut *mut _) else { return };
            msg.set_message_id(id.as_ptr());
            if let Some(a) = msg.get_attributes().upgrade() {
                a.set_int(c"latency".as_ptr(), latency);
            }
            peer.notify(std::mem::transmute_copy(&msg.as_ptr()));
        }
    }
}

impl IPluginBase for Component {
    unsafe fn initialize(&self, context: *mut c_void) -> tresult {
        let host = unsafe { VstPtr::<dyn IUnknown>::shared(context as *mut *mut _) }.and_then(|u| u.cast::<dyn IHostApplication>());
        self.probe.log(format!("comp.initialize host={}", host.is_some()));
        *self.host.borrow_mut() = host;
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        self.probe.log("comp.terminate");
        *self.host.borrow_mut() = None;
        kResultOk
    }
}

impl IComponent for Component {
    unsafe fn get_controller_class_id(&self, tuid: *mut IID) -> tresult {
        unsafe { *tuid = CONTROLLER_CID };
        kResultOk
    }

    unsafe fn set_io_mode(&self, _mode: IoMode) -> tresult {
        kNotImplemented
    }

    unsafe fn get_bus_count(&self, type_: MediaType, _dir: BusDirection) -> i32 {
        // One stereo audio input and output, no event buses.
        i32::from(type_ == 0)
    }

    unsafe fn get_bus_info(&self, type_: MediaType, dir: BusDirection, index: i32, info: *mut BusInfo) -> tresult {
        if type_ != 0 || index != 0 {
            return kInvalidArgument;
        }
        let info = unsafe { &mut *info };
        info.media_type = 0;
        info.direction = dir;
        info.channel_count = 2;
        copy_w(&mut info.name, if dir == 0 { "In" } else { "Out" });
        info.bus_type = 0;
        info.flags = 1;
        kResultOk
    }

    unsafe fn get_routing_info(&self, _in_info: *mut RoutingInfo, _out_info: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }

    unsafe fn activate_bus(&self, type_: MediaType, dir: BusDirection, index: i32, state: TBool) -> tresult {
        self.probe.log(format!("comp.activate_bus {type_} {dir} {index} {state}"));
        kResultOk
    }

    unsafe fn set_active(&self, state: TBool) -> tresult {
        self.probe.log(format!("comp.set_active {state}"));
        if state != 0 {
            unsafe { self.send(c"activated", i64::from(self.probe.latency.load(Ordering::Acquire))) };
        }
        kResultOk
    }

    unsafe fn set_state(&self, state: SharedVstPtr<dyn IBStream>) -> tresult {
        match unsafe { read_f64(&state) } {
            Some(g) => {
                self.gain.set(g);
                self.probe.log(format!("comp.set_state {g}"));
                kResultOk
            }
            None => kResultFalse,
        }
    }

    unsafe fn get_state(&self, state: SharedVstPtr<dyn IBStream>) -> tresult {
        unsafe { write_bytes(&state, &self.gain.get().to_le_bytes()) }
    }
}

impl IAudioProcessor for Component {
    unsafe fn set_bus_arrangements(&self, inputs: *mut SpeakerArrangement, num_ins: i32, outputs: *mut SpeakerArrangement, num_outs: i32) -> tresult {
        let ok = num_ins == 1 && num_outs == 1 && unsafe { *inputs == 3 && *outputs == 3 };
        self.probe.log(format!("comp.set_bus_arrangements {ok}"));
        if ok { kResultOk } else { kResultFalse }
    }

    unsafe fn get_bus_arrangement(&self, _dir: BusDirection, index: i32, arr: *mut SpeakerArrangement) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        unsafe { *arr = 3 };
        kResultOk
    }

    unsafe fn can_process_sample_size(&self, symbolic_sample_size: i32) -> tresult {
        if symbolic_sample_size == 0 { kResultOk } else { kResultFalse }
    }

    unsafe fn get_latency_samples(&self) -> u32 {
        self.probe.latency.load(Ordering::Acquire)
    }

    unsafe fn setup_processing(&self, setup: *const ProcessSetup) -> tresult {
        let s = unsafe { &*setup };
        self.probe.log(format!("comp.setup_processing {} {} {}", s.sample_rate, s.max_samples_per_block, s.symbolic_sample_size));
        kResultOk
    }

    unsafe fn set_processing(&self, state: TBool) -> tresult {
        self.probe.log(format!("comp.set_processing {state}"));
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let d = unsafe { &mut *data };
        if !d.context.is_null() {
            let c = unsafe { &*d.context };
            let e = (c.state, c.tempo, c.project_time_music, c.project_time_samples, c.bar_position_music, c.cycle_end_music);
            self.probe.contexts.lock().unwrap().push(e);
        }
        let mut points: Vec<(i32, f64)> = Vec::new();
        unsafe {
            if let Some(pc) = d.input_param_changes.upgrade() {
                for i in 0..pc.get_parameter_count() {
                    let Some(q) = pc.get_parameter_data(i).upgrade() else { continue };
                    let id = q.get_parameter_id();
                    for k in 0..q.get_point_count() {
                        let (mut o, mut v) = (0, 0.0);
                        if q.get_point(k, &mut o, &mut v) == kResultOk && id == GAIN {
                            points.push((o, v));
                        }
                    }
                }
            }
        }
        self.probe.gain_points.lock().unwrap().extend_from_slice(&points);
        let n = d.num_samples.max(0) as usize;
        let mut peak = 0f32;
        let mut g = self.gain.get();
        if n > 0 && d.num_inputs == 1 && d.num_outputs == 1 {
            unsafe {
                let (ib, ob) = (&*d.inputs, &*d.outputs);
                for c in 0..2 {
                    let i = std::slice::from_raw_parts(*(ib.buffers as *mut *mut f32).add(c), n);
                    let o = std::slice::from_raw_parts_mut(*(ob.buffers as *mut *mut f32).add(c), n);
                    let (mut gc, mut p) = (self.gain.get(), 0);
                    for s in 0..n {
                        while p < points.len() && points[p].0 as usize <= s {
                            gc = points[p].1;
                            p += 1;
                        }
                        o[s] = i[s] * 2.0 * gc as f32;
                        peak = peak.max(o[s].abs());
                    }
                    g = gc;
                }
            }
        }
        if let Some(&(_, v)) = points.last() {
            g = v;
        }
        self.gain.set(g);
        if n > 0 {
            unsafe {
                if let Some(out) = d.output_param_changes.upgrade() {
                    let mut index = 0;
                    if let Some(q) = out.add_parameter_data(&PEAK, &mut index).upgrade() {
                        q.add_point(0, f64::from(peak.min(1.0)), &mut index);
                    }
                }
            }
        }
        kResultOk
    }

    unsafe fn get_tail_samples(&self) -> u32 {
        0
    }
}

impl IConnectionPoint for Component {
    unsafe fn connect(&self, other: SharedVstPtr<dyn IConnectionPoint>) -> tresult {
        self.probe.log("comp.connect");
        *self.peer.borrow_mut() = other.upgrade();
        kResultOk
    }

    unsafe fn disconnect(&self, _other: SharedVstPtr<dyn IConnectionPoint>) -> tresult {
        self.probe.log("comp.disconnect");
        *self.peer.borrow_mut() = None;
        kResultOk
    }

    unsafe fn notify(&self, _message: SharedVstPtr<dyn IMessage>) -> tresult {
        kResultOk
    }
}

// ---- edit controller ----

#[VST3(implements(IEditController, IConnectionPoint))]
pub struct Controller {
    probe: Arc<Probe>,
    handler: RefCell<Option<VstPtr<dyn IComponentHandler>>>,
    peer: RefCell<Option<VstPtr<dyn IConnectionPoint>>>,
    /// gain, mode, peak, extra (normalized).
    values: RefCell<[f64; 4]>,
    _live: Live,
}

impl Controller {
    fn new(probe: &Arc<Probe>) -> Box<Controller> {
        Controller::allocate(probe.clone(), RefCell::new(None), RefCell::new(None), RefCell::new([0.5, 0.0, 0.0, 0.0]), Live::new(probe))
    }

    fn slot(id: u32) -> Option<usize> {
        (GAIN..=EXTRA).contains(&id).then(|| (id - GAIN) as usize)
    }

    pub fn value(&self, id: u32) -> f64 {
        Self::slot(id).map_or(-1.0, |i| self.values.borrow()[i])
    }

    /// A knob turned in the editor: begin, value, end through the component handler.
    pub fn user_edit(&self, id: u32, v: f64) {
        if let Some(i) = Self::slot(id) {
            self.values.borrow_mut()[i] = v;
        }
        let h = self.handler.borrow().clone().expect("a component handler");
        unsafe {
            h.begin_edit(id);
            h.perform_edit(id, v);
            h.end_edit(id);
        }
    }

    pub fn restart(&self, flags: i32) {
        let h = self.handler.borrow().clone().expect("a component handler");
        unsafe { h.restart_component(flags) };
    }
}

impl IPluginBase for Controller {
    unsafe fn initialize(&self, context: *mut c_void) -> tresult {
        self.probe.log(format!("ctrl.initialize host={}", !context.is_null()));
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        self.probe.log("ctrl.terminate");
        *self.handler.borrow_mut() = None;
        kResultOk
    }
}

impl IEditController for Controller {
    unsafe fn set_component_state(&self, state: SharedVstPtr<dyn IBStream>) -> tresult {
        match unsafe { read_f64(&state) } {
            Some(g) => {
                self.values.borrow_mut()[0] = g;
                self.probe.log(format!("ctrl.set_component_state {g}"));
                kResultOk
            }
            None => kResultFalse,
        }
    }

    unsafe fn set_state(&self, state: SharedVstPtr<dyn IBStream>) -> tresult {
        // "MODE" + the mode value.
        let Some(s) = state.upgrade() else { return kInvalidArgument };
        let mut b = [0u8; 12];
        let mut n = 0;
        unsafe { s.read(b.as_mut_ptr() as *mut c_void, 12, &mut n) };
        if n != 12 || &b[..4] != b"MODE" {
            return kResultFalse;
        }
        let m = f64::from_le_bytes(b[4..].try_into().unwrap());
        self.values.borrow_mut()[1] = m;
        self.probe.log(format!("ctrl.set_state {m}"));
        kResultOk
    }

    unsafe fn get_state(&self, state: SharedVstPtr<dyn IBStream>) -> tresult {
        let mut b = b"MODE".to_vec();
        b.extend_from_slice(&self.values.borrow()[1].to_le_bytes());
        unsafe { write_bytes(&state, &b) }
    }

    unsafe fn get_parameter_count(&self) -> i32 {
        if self.probe.extra.load(Ordering::Acquire) { 4 } else { 3 }
    }

    unsafe fn get_parameter_info(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        if index < 0 || index >= unsafe { self.get_parameter_count() } {
            return kInvalidArgument;
        }
        let info = unsafe { &mut *info };
        let (id, title, units, steps, default, flags) = match index {
            0 => (GAIN, "Gain", "dB", 0, 0.5, 1),
            1 => (MODE, "Mode", "", 2, 0.0, 1 | 8),
            2 => (PEAK, "Peak", "", 0, 0.0, 2),
            _ => (EXTRA, "Extra", "%", 0, 0.0, 1),
        };
        info.id = id;
        copy_w(&mut info.title, title);
        copy_w(&mut info.short_title, title);
        copy_w(&mut info.units, units);
        info.step_count = steps;
        info.default_normalized_value = default;
        info.unit_id = 0;
        info.flags = flags;
        kResultOk
    }

    unsafe fn get_param_string_by_value(&self, id: u32, value: f64, string: *mut TChar) -> tresult {
        let s = match id {
            GAIN => format!("{:.1}", -24.0 + 48.0 * value),
            MODE => ["Soft", "Hard", "Clip"][((value * 2.0).round() as usize).min(2)].to_string(),
            _ => format!("{value:.2}"),
        };
        copy_w(unsafe { std::slice::from_raw_parts_mut(string, 128) }, &s);
        kResultOk
    }

    unsafe fn get_param_value_by_string(&self, _id: u32, _string: *const TChar, _value: *mut f64) -> tresult {
        kNotImplemented
    }

    unsafe fn normalized_param_to_plain(&self, id: u32, value: f64) -> f64 {
        match id {
            GAIN => -24.0 + 48.0 * value,
            MODE => value * 2.0,
            _ => value,
        }
    }

    unsafe fn plain_param_to_normalized(&self, id: u32, plain: f64) -> f64 {
        match id {
            GAIN => (plain + 24.0) / 48.0,
            MODE => plain / 2.0,
            _ => plain,
        }
    }

    unsafe fn get_param_normalized(&self, id: u32) -> f64 {
        self.value(id).max(0.0)
    }

    unsafe fn set_param_normalized(&self, id: u32, value: f64) -> tresult {
        match Self::slot(id) {
            Some(i) => {
                self.values.borrow_mut()[i] = value;
                kResultOk
            }
            None => kInvalidArgument,
        }
    }

    unsafe fn set_component_handler(&self, handler: SharedVstPtr<dyn IComponentHandler>) -> tresult {
        self.probe.log(format!("ctrl.set_component_handler {}", !handler.is_null()));
        *self.handler.borrow_mut() = handler.upgrade();
        kResultOk
    }

    unsafe fn create_view(&self, _name: FIDString) -> *mut c_void {
        std::ptr::null_mut()
    }
}

impl IConnectionPoint for Controller {
    unsafe fn connect(&self, other: SharedVstPtr<dyn IConnectionPoint>) -> tresult {
        self.probe.log("ctrl.connect");
        *self.peer.borrow_mut() = other.upgrade();
        kResultOk
    }

    unsafe fn disconnect(&self, _other: SharedVstPtr<dyn IConnectionPoint>) -> tresult {
        self.probe.log("ctrl.disconnect");
        *self.peer.borrow_mut() = None;
        kResultOk
    }

    unsafe fn notify(&self, message: SharedVstPtr<dyn IMessage>) -> tresult {
        let Some(m) = message.upgrade() else { return kInvalidArgument };
        unsafe {
            let id = CStr::from_ptr(m.get_message_id()).to_string_lossy().into_owned();
            let mut latency = -1;
            if let Some(a) = m.get_attributes().upgrade() {
                a.get_int(c"latency".as_ptr(), &mut latency);
            }
            self.probe.log(format!("ctrl.notify {id} {latency}"));
        }
        kResultOk
    }
}
