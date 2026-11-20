// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! `lemon-monitor <socket|port> [--token <token>]`: a window showing what a Lemon Studio session holds and does, read
//! through its control socket only (a session started with `--control` or `--serve`). It shows the project as the
//! session holds it, each change as it comes (`events`: what was done, by whom, which commands), the undo history,
//! the proposals waiting, and the parameters of a plugin chosen in the project's list. It changes nothing.

use lemon_monitor::Client;
use os_ui::{Canvas, Color, Font, Rect};
use serde_json::Value;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

/// How often the session is asked what changed.
const POLL: Duration = Duration::from_millis(200);
/// The whole project and the history are read again at least this often, events or not.
const REREAD: Duration = Duration::from_secs(2);
/// How long a new event stands out.
const FRESH: Duration = Duration::from_millis(1500);
/// The events kept.
const KEEP: usize = 1000;

/// What the monitor knows of the session, shared by the poller and the window.
#[derive(Default)]
struct Model {
    /// "Connected to ..." or why not.
    status: String,
    connected: bool,
    state: Value,
    /// Oldest first, with when each came.
    events: Vec<(Instant, Value)>,
    history: Value,
    proposals: Value,
    /// The plugin watched: a channel's or a slot's owner id, and its parameters.
    owner: Option<u32>,
    params: Option<Result<Value, String>>,
}

fn poll(at: String, token: Option<String>, model: Arc<Mutex<Model>>, wake: EventLoopProxy<()>) {
    let mut client: Option<Client> = None;
    let mut since = 0u64;
    let mut read_at = Instant::now() - REREAD;
    loop {
        if client.is_none() {
            match Client::connect(&at, "Monitor", token.as_deref()) {
                Ok(c) => {
                    let h = &c.hello;
                    let status = format!(
                        "{} {} at {at}, protocol {}, level {}",
                        h["app"].as_str().unwrap_or("?"),
                        h["version"].as_str().unwrap_or(""),
                        h["protocol"],
                        h["level"].as_str().unwrap_or("?")
                    );
                    let mut m = model.lock().unwrap();
                    (m.status, m.connected) = (status, true);
                    // Events from before this connection are not shown (the session keeps a few).
                    drop(m);
                    client = Some(c);
                    since = 0;
                    read_at = Instant::now() - REREAD;
                }
                Err(e) => {
                    let mut m = model.lock().unwrap();
                    (m.status, m.connected) = (format!("Not connected to {at}: {e}"), false);
                    drop(m);
                    let _ = wake.send_event(());
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
        }
        let c = client.as_mut().expect("connected");
        let owner = model.lock().unwrap().owner;
        let step = (|| -> Result<bool, lemon_monitor::Error> {
            let (events, last) = c.events(since)?;
            let changed = !events.is_empty() || read_at.elapsed() >= REREAD;
            since = last;
            let read = if changed { Some((c.state()?, c.history()?, c.proposals()?)) } else { None };
            let params = owner.map(|o| c.params(o).map_err(|e| e.to_string()));
            let mut m = model.lock().unwrap();
            let now = Instant::now();
            m.events.extend(events.into_iter().map(|e| (now, e)));
            let n = m.events.len();
            if n > KEEP {
                m.events.drain(..n - KEEP);
            }
            if let Some((state, history, proposals)) = read {
                (m.state, m.history, m.proposals) = (state, history, proposals);
                read_at = now;
            }
            if m.owner == owner {
                m.params = params;
            }
            Ok(changed || owner.is_some())
        })();
        match step {
            Ok(_) => {
                let _ = wake.send_event(());
            }
            Err(e) => {
                let mut m = model.lock().unwrap();
                (m.status, m.connected) = (format!("Lost {at}: {e}"), false);
                client = None;
                let _ = wake.send_event(());
            }
        }
        std::thread::sleep(POLL);
    }
}

const BG: Color = Color::rgb(0x1d2024);
const PANEL: Color = Color::rgb(0x262a2f);
const LINE: Color = Color::rgb(0x343a41);
const TEXT: Color = Color::rgb(0xd9dee4);
const DIM: Color = Color::rgb(0x89929b);
const LEMON: Color = Color::rgb(0xf2d04b);
const BAD: Color = Color::rgb(0xe8705f);
const PICKED: Color = Color::rgb(0x3b4250);

const ROW: f32 = 18.0;
const HEAD: f32 = 26.0;

/// A column of rows drawn from the top, scrolled by `scroll` rows; what can be clicked is noted in `hits`.
struct Column<'a> {
    c: &'a mut Canvas,
    x: f32,
    w: f32,
    y: f32,
    bottom: f32,
    skip: usize,
    hits: &'a mut Vec<(Rect, u32)>,
}

impl Column<'_> {
    fn heading(&mut self, s: &str) {
        self.y += 6.0;
        if self.y + ROW <= self.bottom {
            self.c.text(Font::new(13).bold(), self.x + 8.0, self.y, LEMON, s);
        }
        self.y += ROW + 2.0;
    }

    /// A row of text; `owner`: clicking it watches that plugin; `picked`: it is the one watched.
    fn row(&mut self, s: &str, color: Color, owner: Option<u32>, picked: bool) {
        if self.skip > 0 {
            self.skip -= 1;
            return;
        }
        if self.y + ROW > self.bottom {
            return;
        }
        let r = Rect::new(self.x, self.y, self.w, ROW);
        if picked {
            self.c.fill_rect(r, PICKED);
        }
        if let Some(o) = owner {
            self.hits.push((r, o));
        }
        self.c.text_clip(Font::new(13), self.x + 8.0, self.y + 1.0, self.w - 16.0, color, s);
        self.y += ROW;
    }
}

fn num(v: &Value) -> String {
    match v.as_f64() {
        Some(f) if f.fract() == 0.0 => format!("{f:.0}"),
        Some(f) => format!("{f:.3}"),
        None => v.as_str().map_or_else(|| v.to_string(), str::to_string),
    }
}

fn paint(c: &mut Canvas, m: &Model, scroll: &[usize; 4], hits: &mut Vec<(Rect, u32)>) {
    let (w, h) = c.size();
    c.clear(BG);
    c.fill_rect(Rect::new(0.0, 0.0, w, HEAD), PANEL);
    c.text(Font::new(13).bold(), 10.0, 5.0, LEMON, "Lemon monitor");
    c.text_clip(Font::new(13), 120.0, 5.0, w - 130.0, if m.connected { DIM } else { BAD }, &m.status);
    let cols = [0.0, 0.27, 0.58, 0.78, 1.0].map(|k| (k * w).round());
    for &x in &cols[1..4] {
        c.fill_rect(Rect::new(x, HEAD, 1.0, h - HEAD), LINE);
    }
    let bounds = |i: usize| (cols[i] + if i > 0 { 1.0 } else { 0.0 }, cols[i + 1] - cols[i] - 1.0);

    // The project.
    {
        let (x, cw) = bounds(0);
        let mut k = Column { c: &mut *c, x, w: cw, y: HEAD, bottom: h, skip: scroll[0], hits: &mut *hits };
        let s = &m.state;
        k.heading("Project");
        if s.is_null() {
            k.row("Nothing read yet", DIM, None, false);
        } else {
            let sig = match s["time_signature"].as_array() {
                Some(t) if t.len() == 2 => format!("{}/{}", num(&t[0]), num(&t[1])),
                _ => format!("{} beats of {} steps", num(&s["bar"]["beats"]), num(&s["bar"]["steps_per_beat"])),
            };
            k.row(&format!("Tempo {}, {sig}, PPQ {}", num(&s["tempo"]), num(&s["ppq"])), TEXT, None, false);
            if let Some(t) = s["info"]["title"].as_str().filter(|t| !t.is_empty()) {
                k.row(&format!("Title: {t}"), TEXT, None, false);
            }
            let channels = s["channels"].as_array().cloned().unwrap_or_default();
            k.heading(&format!("Channels ({})", channels.len()));
            for ch in &channels {
                let id = ch["id"].as_u64().map(|i| i as u32);
                let plugin = ch["plugin"].as_str().map(|p| format!("  {p}")).unwrap_or_default();
                let off = if ch["enabled"].as_bool() == Some(false) { "  (off)" } else { "" };
                let text = format!("{}  {}{plugin}{off}", num(&ch["row"]), ch["name"].as_str().unwrap_or("?"));
                k.row(&text, TEXT, id, id.is_some() && id == m.owner);
            }
            let tracks = s["mixer"].as_array().or_else(|| s["tracks"].as_array()).cloned().unwrap_or_default();
            k.heading(&format!("Mixer ({})", tracks.len()));
            for t in &tracks {
                let vol = t["volume"].as_f64().map(|v| format!("  vol {:.0}%", v / 0.8 * 100.0)).unwrap_or_default();
                k.row(&format!("{}  {}{vol}", num(&t["number"]), t["name"].as_str().unwrap_or("?")), TEXT, None, false);
                for sl in t["slots"].as_array().into_iter().flatten() {
                    let id = sl["id"].as_u64().map(|i| i as u32);
                    let text = format!("      slot {}: {}", num(&sl["slot"]), sl["plugin"].as_str().unwrap_or("?"));
                    k.row(&text, DIM, id, id.is_some() && id == m.owner);
                }
            }
            let patterns = s["patterns"].as_array().cloned().unwrap_or_default();
            k.heading(&format!("Patterns ({})", patterns.len()));
            for p in &patterns {
                k.row(&format!("{}  {}  {} notes", num(&p["number"]), p["name"].as_str().unwrap_or(""), num(&p["notes"])), TEXT, None, false);
            }
            if let Some(clips) = s["playlist"]["clips"].as_array() {
                k.heading(&format!("Arrangement: {} clips", clips.len()));
            }
        }
    }

    // What happens, newest first.
    {
        let (x, cw) = bounds(1);
        let mut k = Column { c: &mut *c, x, w: cw, y: HEAD, bottom: h, skip: scroll[1], hits: &mut *hits };
        k.heading(&format!("Changes ({})", m.events.len()));
        if m.events.is_empty() {
            k.row("None since the monitor connected", DIM, None, false);
        }
        for (at, e) in m.events.iter().rev() {
            let ops = e["ops"].as_array().map(|o| o.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            let ops = if ops.is_empty() { String::new() } else { format!("  [{ops}]") };
            let text = format!("{}  {}  {}: {}{ops}", num(&e["seq"]), e["by"].as_str().unwrap_or("?"), e["kind"].as_str().unwrap_or("?"), e["name"].as_str().unwrap_or(""));
            k.row(&text, if at.elapsed() < FRESH { LEMON } else { TEXT }, None, false);
        }
    }

    // The undo history, newest first, and the proposals.
    {
        let (x, cw) = bounds(2);
        let mut k = Column { c: &mut *c, x, w: cw, y: HEAD, bottom: h, skip: scroll[2], hits: &mut *hits };
        let proposals = m.proposals.as_array().cloned().unwrap_or_default();
        k.heading(&format!("Proposals ({})", proposals.len()));
        for p in &proposals {
            let n = p["changes"].as_array().map_or(0, Vec::len);
            k.row(&format!("{}  {}  ({n} changes)", num(&p["proposal"]), p["name"].as_str().unwrap_or("")), TEXT, None, false);
        }
        let steps = m.history["steps"].as_array().cloned().unwrap_or_default();
        k.heading(&format!("History ({} steps)", steps.len()));
        for st in steps.iter().rev() {
            let undone = st["undone"].as_bool() == Some(true);
            k.row(&format!("{}  {}", num(&st["step"]), st["name"].as_str().unwrap_or("")), if undone { DIM } else { TEXT }, None, false);
        }
    }

    // The watched plugin's parameters.
    {
        let (x, cw) = bounds(3);
        let mut k = Column { c: &mut *c, x, w: cw, y: HEAD, bottom: h, skip: scroll[3], hits: &mut *hits };
        match (&m.owner, &m.params) {
            (None, _) => {
                k.heading("Parameters");
                k.row("Click a channel or a slot", DIM, None, false);
            }
            (Some(o), None) => {
                k.heading("Parameters");
                k.row(&format!("Reading {o}..."), DIM, None, false);
            }
            (Some(_), Some(Err(e))) => {
                k.heading("Parameters");
                k.row(e, BAD, None, false);
            }
            (Some(_), Some(Ok(p))) => {
                let list = p["params"].as_array().cloned().unwrap_or_default();
                k.heading(&format!("{} ({})", p["plugin"].as_str().unwrap_or("Plugin"), list.len()));
                for q in &list {
                    let value = q["text"].as_str().map(str::to_string).filter(|t| !t.is_empty()).unwrap_or_else(|| num(&q["value"]));
                    k.row(&format!("{}: {value}", q["name"].as_str().unwrap_or("?")), TEXT, None, false);
                }
            }
        }
    }
}

struct App {
    model: Arc<Mutex<Model>>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    canvas: Option<Canvas>,
    hits: Vec<(Rect, u32)>,
    scroll: [usize; 4],
    mouse: (f32, f32),
}

impl App {
    fn column_at(&self, x: f32) -> usize {
        let w = self.canvas.as_ref().map_or(1.0, |c| c.size().0);
        [0.27, 0.58, 0.78].iter().take_while(|&&k| x >= k * w).count()
    }

    fn redraw(&mut self) {
        let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else { return };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        let k = window.scale_factor() as f32;
        let canvas = match &mut self.canvas {
            Some(c) => {
                if c.pixmap.width() != size.width || c.pixmap.height() != size.height || c.scale() != k {
                    c.resize(size.width, size.height, k);
                }
                c
            }
            None => match Canvas::new(size.width, size.height, k) {
                Ok(c) => self.canvas.insert(c),
                Err(_) => return,
            },
        };
        self.hits.clear();
        paint(canvas, &self.model.lock().unwrap(), &self.scroll, &mut self.hits);
        if surface.resize(w, h).is_ok() {
            if let Ok(mut buf) = surface.buffer_mut() {
                canvas.copy_to_xrgb(&mut buf);
                let _ = buf.present();
            }
        }
    }
}

impl ApplicationHandler<()> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Lemon monitor").with_inner_size(LogicalSize::new(1280.0, 800.0));
        let Ok(window) = event_loop.create_window(attrs) else {
            event_loop.exit();
            return;
        };
        let window = Rc::new(window);
        let Ok(context) = softbuffer::Context::new(window.clone()) else { return };
        self.surface = softbuffer::Surface::new(&context, window.clone()).ok();
        self.window = Some(window);
    }

    fn user_event(&mut self, _: &ActiveEventLoop, _: ()) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let k = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
                self.mouse = (position.x as f32 / k, position.y as f32 / k);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let rows = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * 3.0,
                    MouseScrollDelta::PixelDelta(p) => -(p.y as f32) / ROW,
                };
                let i = self.column_at(self.mouse.0);
                self.scroll[i] = (self.scroll[i] as f32 + rows).max(0.0).round() as usize;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                let (x, y) = self.mouse;
                if let Some(&(_, owner)) = self.hits.iter().find(|(r, _)| r.contains(x, y)) {
                    let mut m = self.model.lock().unwrap();
                    m.owner = if m.owner == Some(owner) { None } else { Some(owner) };
                    m.params = None;
                    drop(m);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let token = args.iter().position(|a| a == "--token").map(|i| {
        let t = args.get(i + 1).cloned();
        args.drain(i..(i + 2).min(args.len()));
        t
    });
    let Some(at) = args.first().cloned() else {
        eprintln!("usage: lemon-monitor <socket path | port> [--token <token>]");
        std::process::exit(2);
    };
    let event_loop = match EventLoop::<()>::with_user_event().build() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("lemon-monitor: no window: {e}");
            std::process::exit(1);
        }
    };
    let model = Arc::new(Mutex::new(Model { status: format!("Connecting to {at}..."), ..Model::default() }));
    let wake = event_loop.create_proxy();
    {
        let model = model.clone();
        let token = token.flatten();
        std::thread::Builder::new().name("poll".into()).spawn(move || poll(at, token, model, wake)).expect("poll thread");
    }
    let mut app = App { model, window: None, surface: None, canvas: None, hits: Vec::new(), scroll: [0; 4], mouse: (0.0, 0.0) };
    let _ = event_loop.run_app(&mut app);
}
