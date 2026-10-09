//! OpenStudio: the application window.

mod app;
mod icons;
mod logo;
mod piano;
mod tabler;
mod transport;
mod views;

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Window, WindowId};

use app::App;
use os_ui::Canvas;

struct Shell {
    app: App,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    canvas: Option<Canvas>,
    mods: ModifiersState,
    last_click: Instant,
}

impl ApplicationHandler for Shell {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("OpenStudio")
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(1920.0, 1080.0))
            .with_maximized(true);
        // App icon: resource 1, embedded by build.rs from assets/openstudio.ico.
        #[cfg(windows)]
        let attrs = {
            use winit::dpi::PhysicalSize;
            use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
            use winit::window::Icon;
            let icon = |s: u32| Icon::from_resource(1, Some(PhysicalSize::new(s, s))).ok();
            attrs.with_window_icon(icon(16)).with_taskbar_icon(icon(32))
        };
        let window = Rc::new(el.create_window(attrs).expect("create window"));
        let context = softbuffer::Context::new(window.clone()).expect("graphics context");
        self.surface = Some(softbuffer::Surface::new(&context, window.clone()).expect("surface"));
        self.window = Some(window);
        el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16)));
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        // Animate while playing; otherwise repaint a few times a second (peaks, clock).
        let ms = if self.app.transport.playing() { 16 } else { 100 };
        el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(ms)));
        if self.app.tick() {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else { return };
        let scale = window.scale_factor() as f32;
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                let was = self.app.resize_cursor;
                self.app.mouse_move(position.x as f32 / scale, position.y as f32 / scale);
                if self.app.resize_cursor != was {
                    window.set_cursor(if self.app.resize_cursor { CursorIcon::EwResize } else { CursorIcon::Default });
                }
                if self.app.has_damage() {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                if state == ElementState::Pressed {
                    let double = self.last_click.elapsed() < Duration::from_millis(350);
                    self.last_click = Instant::now();
                    self.app.mouse_down();
                    if self.app.want_drag_window {
                        self.app.want_drag_window = false;
                        if double {
                            window.set_maximized(!window.is_maximized());
                        } else {
                            let _ = window.drag_window();
                        }
                    }
                } else {
                    self.app.mouse_up();
                    if self.app.resize_cursor && self.app.drag.is_none() {
                        self.app.resize_cursor = false;
                        window.set_cursor(CursorIcon::Default);
                    }
                }
                window.request_redraw();
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right, .. } => {
                self.app.mouse_down_right();
                window.request_redraw();
            }
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Right, .. } => {
                self.app.mouse_up();
                window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32 / 40.0, p.y as f32 / 40.0),
                };
                self.app.wheel(dx, dy, self.mods.shift_key(), self.mods.control_key());
                window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && !event.repeat => {
                self.app.key(&event.logical_key, self.mods.control_key(), self.mods.alt_key());
                window.request_redraw();
            }
            WindowEvent::Resized(_) => {
                self.app.invalidate();
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let size = window.inner_size();
                let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
                if self.canvas.is_none() {
                    match Canvas::new(size.width, size.height, scale) {
                        Ok(c) => self.canvas = Some(c),
                        Err(e) => {
                            eprintln!("openstudio: {e}");
                            el.exit();
                            return;
                        }
                    }
                }
                let canvas = self.canvas.as_mut().expect("canvas");
                if canvas.pixmap.width() != size.width || canvas.pixmap.height() != size.height || canvas.scale != scale {
                    canvas.resize(size.width, size.height, scale);
                    self.app.invalidate();
                }
                // Paint only what changed, then present only that region.
                let damage = self.app.take_damage();
                if let Some(d) = damage {
                    self.app.paint(canvas, d);
                }
                if let Some(surface) = self.surface.as_mut() {
                    if surface.resize(w, h).is_ok() {
                        if let Ok(mut buf) = surface.buffer_mut() {
                            let age = buf.age();
                            match damage {
                                Some(Some(r)) if age > 0 => {
                                    let px = canvas.device_rect(r);
                                    canvas.copy_region_to_xrgb(&mut buf, px);
                                    match (NonZeroU32::new(px.2 as u32), NonZeroU32::new(px.3 as u32)) {
                                        (Some(rw), Some(rh)) => {
                                            let _ = buf.present_with_damage(&[softbuffer::Rect { x: px.0 as u32, y: px.1 as u32, width: rw, height: rh }]);
                                        }
                                        _ => {
                                            let _ = buf.present();
                                        }
                                    }
                                }
                                // Nothing painted (the OS asked for a redraw): the buffer already
                                // holds the last frame unless it was discarded.
                                None => {
                                    if age == 0 {
                                        canvas.copy_to_xrgb(&mut buf);
                                    }
                                    let _ = buf.present();
                                }
                                _ => {
                                    canvas.copy_to_xrgb(&mut buf);
                                    let _ = buf.present();
                                }
                            }
                        }
                    }
                }
                if self.app.want_minimize {
                    self.app.want_minimize = false;
                    window.set_minimized(true);
                }
                if self.app.want_maximize {
                    self.app.want_maximize = false;
                    window.set_maximized(!window.is_maximized());
                }
                if self.app.want_close {
                    el.exit();
                }
            }
            _ => {}
        }
    }
}

/// `openstudio --screenshot out.png [--view playlist|mixer|piano|none] [--pattern N] [--channel I] [--no-rack] [project.flp]`:
/// paint one 1920×1080 frame offscreen (used to compare with reference measurements).
fn screenshot(args: &[String]) {
    let mut out = None;
    let mut view = "playlist".to_string();
    let mut rack = true;
    let mut browser = true;
    let mut path = None;
    let mut pattern = None;
    let mut channel = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--screenshot" => out = it.next().cloned(),
            "--view" => view = it.next().cloned().unwrap_or_default(),
            "--no-rack" => rack = false,
            "--no-browser" => browser = false,
            "--pattern" => pattern = it.next().and_then(|v| v.parse().ok()),
            "--channel" => channel = it.next().and_then(|v| v.parse().ok()),
            other => path = Some(std::path::PathBuf::from(other)),
        }
    }
    let mut app = App::new(path);
    if let Some(p) = pattern {
        app.state.pattern = p;
    }
    if let Some(c) = channel {
        app.state.selected_channel = c;
    }
    app.state.show_rack = rack;
    app.state.show_browser = browser;
    app.state.front = match view.as_str() {
        "mixer" => Some(app::Win::Mixer),
        "piano" => Some(app::Win::PianoRoll),
        "none" => None,
        _ => Some(app::Win::Playlist),
    };
    if app.state.front == Some(app::Win::PianoRoll) {
        piano::center_keys(&mut app.state, 744.0);
    }
    let mut canvas = Canvas::new(1920, 1080, 1.0).expect("canvas");
    app.paint(&mut canvas, None);
    if std::env::var_os("OS_BENCH").is_some() {
        let t0 = Instant::now();
        for _ in 0..20 {
            app.paint(&mut canvas, None);
        }
        eprintln!("paint: {:.2} ms/frame", t0.elapsed().as_secs_f64() * 50.0);
        for (name, r) in [
            ("rack-sized damage", os_ui::Rect::new(317.0, 195.0, 510.0, 260.0)),
            ("fader-column damage", os_ui::Rect::new(317.0, 500.0, 50.0, 500.0)),
            ("hover-button damage", os_ui::Rect::new(975.0, 45.0, 36.0, 36.0)),
        ] {
            let t1 = Instant::now();
            for _ in 0..20 {
                app.paint(&mut canvas, Some(r));
            }
            eprintln!("  {name}: {:.2} ms/frame", t1.elapsed().as_secs_f64() * 50.0);
        }
    }
    canvas.pixmap.save_png(out.expect("--screenshot needs a path")).expect("write png");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--screenshot") {
        screenshot(&args);
        return;
    }
    let path = args.first().map(std::path::PathBuf::from);
    let event_loop = EventLoop::new().expect("event loop");
    let mut shell = Shell {
        app: App::new(path),
        window: None,
        surface: None,
        canvas: None,
        mods: ModifiersState::default(),
        last_click: Instant::now() - Duration::from_secs(10),
    };
    event_loop.run_app(&mut shell).expect("run");
}
