//! Simple vector glyphs for toolbar and window buttons. Original drawings: each only suggests its
//! function (no artwork is copied from any other program).

use os_ui::{Canvas, Color, Face, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Minimize,
    Restore,
    Close,
    Play,
    Stop,
    Metronome,
    WaitInput,
    Countdown,
    Blend,
    LoopRecord,
    Undo,
    Cut,
    Mic,
    Help,
    Save,
    SaveRender,
    Info,
    Download,
    TypingKeyboard,
    StepEdit,
    Slide,
    Link,
    MidiControl,
    Magnet,
    Playlist,
    PianoRoll,
    ChannelRack,
    Mixer,
    Browser,
    Project,
    Plug,
    AudioEditor,
    Touch,
    Store,
    Globe,
    ArrowRight,
    ArrowUp,
    Back,
    Wave,
    Page,
    Star,
    Folder,
    Search,
    Pencil,
    Brush,
    Delete,
    Mute,
    Slip,
    Slice,
    Select,
    Zoom,
    Speaker,
    Plus,
    Note,
    Person,
    Cloud,
    Box,
}

/// Draw `icon` centered in `r` with color `c`.
pub fn draw(cv: &mut Canvas, icon: Icon, r: Rect, c: Color) {
    let (x, y) = (r.cx(), r.cy());
    let s = r.w.min(r.h) / 2.0; // half-size of the drawing box
    let u = s / 8.0; // unit: the glyph is drawn on a 16×16 grid centered at (x, y)
    let p = |dx: f32, dy: f32| (x + dx * u, y + dy * u);
    let w = (1.6 * u).max(1.2);
    match icon {
        Icon::Minimize => cv.line(x - 4.0, y + 4.0, x + 4.0, y + 4.0, 2.0, c),
        Icon::Restore => {
            cv.stroke_rrect(Rect::new(x - 5.0, y - 2.0, 8.0, 7.0), 0.0, 1.6, c);
            cv.polyline(&[(x - 2.0, y - 2.0), (x - 2.0, y - 5.0), (x + 5.0, y - 5.0), (x + 5.0, y + 2.0), (x + 3.0, y + 2.0)], 1.6, c);
        }
        Icon::Close => {
            cv.line(x - 5.0, y - 5.0, x + 5.0, y + 5.0, 2.0, c);
            cv.line(x - 5.0, y + 5.0, x + 5.0, y - 5.0, 2.0, c);
        }
        Icon::Play => cv.fill_polygon(&[p(-3.5, -5.0), p(5.0, 0.0), p(-3.5, 5.0)], c),
        Icon::Stop => cv.fill_rect(Rect::new(x - 4.0 * u, y - 4.0 * u, 8.0 * u, 8.0 * u), c),
        Icon::Metronome => {
            cv.polyline(&[p(-7.0, 5.0), p(-1.0, -5.0), p(1.0, -5.0), p(7.0, 5.0)], w, c);
            cv.line(p(0.0, 5.0).0, p(0.0, 5.0).1, p(3.0, -6.0).0, p(3.0, -6.0).1, w, c);
        }
        Icon::WaitInput => {
            keys(cv, p(-7.0, -3.0), u, 3, c);
            cv.stroke_circle(p(4.0, 1.0).0, p(4.0, 1.0).1, 3.5 * u, w, c);
            cv.line(p(4.0, 1.0).0, p(4.0, 1.0).1, p(4.0, -1.0).0, p(4.0, -1.0).1, w, c);
        }
        Icon::Countdown => {
            cv.text(Face::Ui, 11.0 * u / 0.6, x - 7.0 * u, y - 5.0 * u, c, "3.2");
        }
        Icon::Blend => {
            keys(cv, p(-7.0, -3.0), u, 3, c);
            cv.line(p(3.0, 0.0).0, p(3.0, 0.0).1, p(8.0, 0.0).0, p(8.0, 0.0).1, w, c);
            cv.line(p(5.5, -2.5).0, p(5.5, -2.5).1, p(5.5, 2.5).0, p(5.5, 2.5).1, w, c);
        }
        Icon::LoopRecord => {
            keys(cv, p(-7.0, -3.0), u, 3, c);
            cv.arc(p(4.5, 1.0).0, p(4.5, 1.0).1, 3.0 * u, -1.2, 5.0, w, c);
        }
        Icon::Undo => {
            cv.arc(x, y, 5.5 * u, -2.3, 5.2, w * 1.2, c);
            cv.fill_polygon(&[p(-7.0, -6.0), p(-2.0, -6.0), p(-6.0, -2.0)], c);
        }
        Icon::Cut => {
            cv.stroke_circle(p(-3.0, 4.0).0, p(-3.0, 4.0).1, 2.2 * u, w, c);
            cv.stroke_circle(p(3.0, 4.0).0, p(3.0, 4.0).1, 2.2 * u, w, c);
            cv.line(p(-2.0, 2.0).0, p(-2.0, 2.0).1, p(4.0, -7.0).0, p(4.0, -7.0).1, w, c);
            cv.line(p(2.0, 2.0).0, p(2.0, 2.0).1, p(-4.0, -7.0).0, p(-4.0, -7.0).1, w, c);
        }
        Icon::Mic => {
            cv.fill_rrect(Rect::new(x - 2.5 * u, y - 7.0 * u, 5.0 * u, 9.0 * u), 2.5 * u, c);
            cv.arc(x, y, 4.5 * u, 0.0, std::f32::consts::PI, w, c);
            cv.line(x, y + 4.5 * u, x, y + 7.0 * u, w, c);
            cv.line(x - 3.0 * u, y + 7.0 * u, x + 3.0 * u, y + 7.0 * u, w, c);
        }
        Icon::Help => cv.text_center(Face::Ui, 16.0 * u / 0.8, r, c, "?"),
        Icon::Save | Icon::SaveRender => {
            let b = Rect::new(x - 6.0 * u, y - 6.0 * u, 12.0 * u, 12.0 * u);
            cv.fill_rrect(b, 1.5 * u, c);
            cv.fill_rect(Rect::new(x - 3.5 * u, y - 6.0 * u, 7.0 * u, 4.0 * u), Color::rgb(0x2e3234));
            cv.fill_rect(Rect::new(x - 4.0 * u, y + 1.0 * u, 8.0 * u, 4.0 * u), Color::rgb(0x2e3234));
            if icon == Icon::SaveRender {
                cv.fill_rect(Rect::new(x - 7.0 * u, y + 6.5 * u, 14.0 * u, 1.2 * u), c);
            }
        }
        Icon::Info => {
            cv.stroke_circle(x, y, 7.0 * u, w, c);
            cv.fill_circle(x, y - 3.5 * u, 1.2 * u, c);
            cv.line(x, y - 1.0 * u, x, y + 4.0 * u, w * 1.2, c);
        }
        Icon::Download => {
            cv.line(x, y - 7.0 * u, x, y + 1.0 * u, w * 1.3, c);
            cv.fill_polygon(&[p(-3.5, -1.0), p(3.5, -1.0), p(0.0, 3.0)], c);
            cv.polyline(&[p(-7.0, 1.0), p(-7.0, 6.0), p(7.0, 6.0), p(7.0, 1.0)], w * 1.3, c);
        }
        Icon::TypingKeyboard => {
            cv.stroke_rrect(Rect::new(x - 8.0 * u, y - 5.0 * u, 16.0 * u, 10.0 * u), 0.0, w, c);
            for i in 1..4 {
                let xx = x - 8.0 * u + i as f32 * 4.0 * u;
                cv.line(xx, y - 5.0 * u, xx, y + 5.0 * u, w * 0.7, c);
            }
            cv.line(x - 8.0 * u, y, x + 8.0 * u, y, w * 0.7, c);
        }
        Icon::StepEdit => {
            cv.line(p(-6.0, 0.0).0, p(-6.0, 0.0).1, p(4.0, 0.0).0, p(4.0, 0.0).1, w * 1.4, c);
            cv.fill_polygon(&[p(2.0, -5.0), p(7.5, 0.0), p(2.0, 5.0)], c);
        }
        Icon::Slide => {
            cv.fill_circle(p(-3.0, 4.0).0, p(-3.0, 4.0).1, 1.8 * u, c);
            cv.polyline(&[p(-1.5, 3.5), p(1.0, -4.0), p(4.0, -6.0)], w, c);
            cv.fill_circle(p(4.0, -2.0).0, p(4.0, -2.0).1, 1.2 * u, c);
        }
        Icon::Link => {
            cv.stroke_rrect(Rect::new(x - 7.0 * u, y - 1.0 * u, 8.0 * u, 5.0 * u), 2.5 * u, w, c);
            cv.stroke_rrect(Rect::new(x - 1.0 * u, y - 4.0 * u, 8.0 * u, 5.0 * u), 2.5 * u, w, c);
        }
        Icon::MidiControl => {
            cv.fill_rrect(Rect::new(x - 3.0 * u, y - 7.0 * u, 6.0 * u, 9.0 * u), 2.0 * u, c);
            cv.fill_rrect(Rect::new(x - 7.0 * u, y + 3.0 * u, 14.0 * u, 3.0 * u), 1.5 * u, c);
        }
        Icon::Magnet => {
            cv.arc(x, y - 1.0 * u, 4.5 * u, std::f32::consts::PI, std::f32::consts::PI, w * 1.6, c);
            cv.line(x - 4.5 * u, y - 1.0 * u, x - 4.5 * u, y + 5.0 * u, w * 1.6, c);
            cv.line(x + 4.5 * u, y - 1.0 * u, x + 4.5 * u, y + 5.0 * u, w * 1.6, c);
        }
        Icon::Playlist => {
            cv.stroke_rrect(Rect::new(x - 7.0 * u, y - 5.0 * u, 14.0 * u, 11.0 * u), 1.0, w, c);
            cv.fill_polygon(&[p(-2.0, -8.0), p(2.0, -8.0), p(0.0, -6.0)], c);
            cv.line(p(-5.0, 0.0).0, p(-5.0, 0.0).1, p(-1.0, 0.0).0, p(-1.0, 0.0).1, w, c);
            cv.line(p(1.0, 3.0).0, p(1.0, 3.0).1, p(5.0, 3.0).0, p(5.0, 3.0).1, w, c);
        }
        Icon::PianoRoll => {
            cv.fill_rect(Rect::new(x - 7.0 * u, y - 6.0 * u, 3.0 * u, 12.0 * u), c);
            for (i, dx) in [(0, -2.0), (1, 1.0), (2, 4.0)] {
                let yy = y - 4.0 * u + i as f32 * 3.0 * u;
                cv.fill_rect(Rect::new(x + dx * u, yy, 3.0 * u, 2.0 * u), c);
            }
        }
        Icon::ChannelRack => {
            for i in 0..3 {
                let yy = y - 6.0 * u + i as f32 * 4.5 * u;
                cv.stroke_rrect(Rect::new(x - 6.0 * u, yy, 9.0 * u, 3.0 * u), 0.0, w * 0.8, c);
                cv.stroke_rrect(Rect::new(x + 4.5 * u, yy, 2.5 * u, 3.0 * u), 0.0, w * 0.8, c);
            }
        }
        Icon::Mixer => {
            for dx in [-4.0, 4.0] {
                cv.line(x + dx * u, y - 7.0 * u, x + dx * u, y + 7.0 * u, w, c);
                cv.stroke_rrect(Rect::new(x + (dx - 2.5) * u, y - 1.5 * u, 5.0 * u, 3.0 * u), 0.0, w, c);
            }
        }
        Icon::Browser => {
            cv.stroke_rrect(Rect::new(x - 7.0 * u, y - 7.0 * u, 5.0 * u, 3.5 * u), 0.0, w * 0.8, c);
            cv.polyline(&[p(-5.0, -3.5), p(-5.0, 5.0), p(0.0, 5.0)], w, c);
            cv.line(p(-5.0, 0.0).0, p(-5.0, 0.0).1, p(0.0, 0.0).0, p(0.0, 0.0).1, w, c);
            cv.fill_polygon(&[p(1.0, -2.0), p(5.0, 0.0), p(1.0, 2.0)], c);
            cv.fill_polygon(&[p(1.0, 3.0), p(5.0, 5.0), p(1.0, 7.0)], c);
        }
        Icon::Project | Icon::Page => {
            cv.fill_polygon(&[p(-4.0, -7.0), p(3.0, -7.0), p(6.0, -4.0), p(6.0, 7.0), p(-4.0, 7.0)], c);
            if icon == Icon::Project {
                cv.stroke_rrect(Rect::new(x - 7.0 * u, y - 5.0 * u, 10.0 * u, 13.0 * u), 0.0, w * 0.8, c);
            }
        }
        Icon::Plug => {
            cv.fill_rrect(Rect::new(x - 5.0 * u, y - 3.0 * u, 10.0 * u, 6.0 * u), 2.0 * u, c);
            cv.line(x - 2.5 * u, y - 7.0 * u, x - 2.5 * u, y - 3.0 * u, w, c);
            cv.line(x + 2.5 * u, y - 7.0 * u, x + 2.5 * u, y - 3.0 * u, w, c);
            cv.line(x, y + 3.0 * u, x, y + 7.5 * u, w * 1.3, c);
        }
        Icon::AudioEditor => {
            cv.fill_circle(p(2.0, -5.0).0, p(2.0, -5.0).1, 2.5 * u, c);
            cv.line(p(1.0, -3.0).0, p(1.0, -3.0).1, p(-4.0, 5.0).0, p(-4.0, 5.0).1, w * 1.3, c);
            cv.line(p(-7.0, 7.0).0, p(-7.0, 7.0).1, p(2.0, 7.0).0, p(2.0, 7.0).1, w * 1.3, c);
        }
        Icon::Touch => {
            cv.fill_polygon(&[p(-6.0, -3.0), p(2.0, -1.0), p(6.0, 3.0), p(2.0, 6.0), p(-2.0, 3.0)], c);
            cv.line(p(-7.0, -6.0).0, p(-7.0, -6.0).1, p(0.0, -2.0).0, p(0.0, -2.0).1, w, c);
        }
        Icon::Store => {
            cv.polyline(&[p(-8.0, -6.0), p(-5.0, -6.0), p(-3.0, 3.0), p(6.0, 3.0), p(7.0, -3.0), p(-4.0, -3.0)], w, c);
            cv.fill_circle(p(-2.0, 6.0).0, p(-2.0, 6.0).1, 1.3 * u, c);
            cv.fill_circle(p(5.0, 6.0).0, p(5.0, 6.0).1, 1.3 * u, c);
        }
        Icon::Globe => {
            cv.stroke_circle(x, y, 6.5 * u, w, c);
            cv.line(x - 6.5 * u, y, x + 6.5 * u, y, w * 0.7, c);
            cv.stroke_rrect(Rect::new(x - 3.0 * u, y - 6.5 * u, 6.0 * u, 13.0 * u), 3.0 * u, w * 0.7, c);
        }
        Icon::ArrowRight => cv.fill_polygon(&[p(-2.5, -4.0), p(3.0, 0.0), p(-2.5, 4.0)], c),
        Icon::ArrowUp => {
            cv.line(x, y - 6.0 * u, x, y + 6.0 * u, w * 1.2, c);
            cv.fill_polygon(&[p(-4.0, -1.0), p(0.0, -7.0), p(4.0, -1.0)], c);
        }
        Icon::Back => {
            cv.arc(x, y + 1.0 * u, 5.0 * u, -2.6, 4.0, w * 1.2, c);
            cv.fill_polygon(&[p(-7.0, -5.0), p(-2.0, -5.0), p(-5.5, -1.0)], c);
        }
        Icon::Wave => {
            let pts: Vec<(f32, f32)> =
                (0..=16).map(|i| p(-8.0 + i as f32, if i % 2 == 0 { 0.0 } else { [3.0, -4.0, 5.0, -2.0][i / 2 % 4] })).collect();
            cv.polyline(&pts, w, c);
        }
        Icon::Star => {
            let pts: Vec<(f32, f32)> = (0..10)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                    let rr = if i % 2 == 0 { 7.0 } else { 3.0 };
                    p(rr * a.cos(), rr * a.sin())
                })
                .collect();
            cv.fill_polygon(&pts, c);
        }
        Icon::Folder => cv.polyline(&[p(-7.0, -5.0), p(-2.0, -5.0), p(0.0, -3.0), p(7.0, -3.0), p(7.0, 5.0), p(-7.0, 5.0), p(-7.0, -5.0)], w, c),
        Icon::Search => {
            cv.stroke_circle(p(-1.5, -1.5).0, p(-1.5, -1.5).1, 4.5 * u, w * 1.3, c);
            cv.line(p(2.0, 2.0).0, p(2.0, 2.0).1, p(6.5, 6.5).0, p(6.5, 6.5).1, w * 1.8, c);
        }
        Icon::Pencil => {
            cv.polyline(&[p(-6.0, 6.0), p(-5.0, 2.0), p(3.0, -6.0), p(6.0, -3.0), p(-2.0, 5.0), p(-6.0, 6.0)], w, c);
        }
        Icon::Brush => {
            cv.fill_polygon(&[p(-7.0, 7.0), p(-5.0, 1.0), p(-1.0, 5.0)], c);
            cv.line(p(-3.0, 3.0).0, p(-3.0, 3.0).1, p(6.0, -6.0).0, p(6.0, -6.0).1, w * 1.6, c);
        }
        Icon::Delete => {
            cv.stroke_circle(x, y, 6.0 * u, w, c);
            cv.line(p(-4.0, 4.0).0, p(-4.0, 4.0).1, p(4.0, -4.0).0, p(4.0, -4.0).1, w, c);
        }
        Icon::Mute => {
            speaker(cv, p(-3.0, 0.0), u, c);
            cv.line(p(2.0, -2.0).0, p(2.0, -2.0).1, p(6.0, 2.0).0, p(6.0, 2.0).1, w, c);
            cv.line(p(2.0, 2.0).0, p(2.0, 2.0).1, p(6.0, -2.0).0, p(6.0, -2.0).1, w, c);
        }
        Icon::Slip => {
            cv.fill_polygon(&[p(-8.0, 0.0), p(-3.0, -4.0), p(-3.0, 4.0)], c);
            cv.fill_polygon(&[p(8.0, 0.0), p(3.0, -4.0), p(3.0, 4.0)], c);
        }
        Icon::Slice => {
            cv.line(p(-6.0, -7.0).0, p(-6.0, -7.0).1, p(5.0, 7.0).0, p(5.0, 7.0).1, w, c);
            cv.fill_polygon(&[p(-1.0, -6.0), p(6.0, -6.0), p(6.0, 3.0)], c);
        }
        Icon::Select => {
            for (a, b) in [((-6.0, -6.0), (-2.0, -6.0)), ((2.0, -6.0), (6.0, -6.0)), ((-6.0, 6.0), (-2.0, 6.0)), ((2.0, 6.0), (6.0, 6.0))] {
                cv.line(p(a.0, a.1).0, p(a.0, a.1).1, p(b.0, b.1).0, p(b.0, b.1).1, w, c);
            }
            for (a, b) in [((-6.0, -6.0), (-6.0, -2.0)), ((6.0, -6.0), (6.0, -2.0)), ((-6.0, 2.0), (-6.0, 6.0)), ((6.0, 2.0), (6.0, 6.0))] {
                cv.line(p(a.0, a.1).0, p(a.0, a.1).1, p(b.0, b.1).0, p(b.0, b.1).1, w, c);
            }
        }
        Icon::Zoom => {
            cv.stroke_circle(p(-1.0, -1.0).0, p(-1.0, -1.0).1, 5.0 * u, w * 1.2, c);
            cv.line(p(3.0, 3.0).0, p(3.0, 3.0).1, p(7.0, 7.0).0, p(7.0, 7.0).1, w * 1.8, c);
        }
        Icon::Speaker => {
            speaker(cv, p(-2.0, 0.0), u, c);
            cv.arc(p(1.0, 0.0).0, p(1.0, 0.0).1, 4.0 * u, -0.9, 1.8, w, c);
        }
        Icon::Plus => {
            cv.line(x - 4.0, y, x + 4.0, y, 1.6, c);
            cv.line(x, y - 4.0, x, y + 4.0, 1.6, c);
        }
        Icon::Note => {
            cv.fill_circle(p(-2.0, 5.0).0, p(-2.0, 5.0).1, 2.5 * u, c);
            cv.polyline(&[p(0.3, 5.0), p(0.3, -7.0), p(5.0, -4.0)], w, c);
        }
        Icon::Person => {
            cv.fill_circle(x, y - 3.0 * u, 3.0 * u, c);
            cv.fill_rrect(Rect::new(x - 5.0 * u, y + 1.0 * u, 10.0 * u, 6.0 * u), 3.0 * u, c);
        }
        Icon::Cloud => {
            cv.fill_circle(p(-3.0, 1.0).0, p(-3.0, 1.0).1, 4.0 * u, c);
            cv.fill_circle(p(2.0, -1.0).0, p(2.0, -1.0).1, 5.0 * u, c);
            cv.fill_rect(Rect::new(x - 7.0 * u, y + 1.0 * u, 14.0 * u, 4.0 * u), c);
        }
        Icon::Box => cv.stroke_rrect(Rect::new(x - 6.0 * u, y - 5.0 * u, 12.0 * u, 10.0 * u), 1.0, w, c),
    }
}

fn keys(cv: &mut Canvas, (x, y): (f32, f32), u: f32, n: usize, c: Color) {
    for i in 0..n {
        cv.fill_rect(Rect::new(x + i as f32 * 3.0 * u, y, 2.0 * u, 7.0 * u), c);
    }
    cv.fill_rect(Rect::new(x, y + 6.0 * u, n as f32 * 3.0 * u - u, 2.0 * u), c);
}

fn speaker(cv: &mut Canvas, (x, y): (f32, f32), u: f32, c: Color) {
    cv.fill_polygon(&[(x - 4.0 * u, y - 2.5 * u), (x - 1.0 * u, y - 2.5 * u), (x + 3.0 * u, y - 6.0 * u), (x + 3.0 * u, y + 6.0 * u), (x - 1.0 * u, y + 2.5 * u), (x - 4.0 * u, y + 2.5 * u)], c);
}

/// OpenStudio placeholder mark (until the project has a logo): a ring with an "O" and an "S" wave.
pub fn logo(cv: &mut Canvas, cx: f32, cy: f32, r: f32, c: Color) {
    cv.stroke_circle(cx, cy, r, r * 0.16, c);
    let pts: Vec<(f32, f32)> = (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let a = t * std::f32::consts::TAU;
            (cx - r * 0.55 + t * r * 1.1, cy - a.sin() * r * 0.32)
        })
        .collect();
    cv.polyline(&pts, r * 0.14, c);
}
