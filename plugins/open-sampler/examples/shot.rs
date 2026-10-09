//! Renders the Open Sampler editor offscreen: `cargo run -p open-sampler --example shot -- out.png [tab] [sample.wav]`.
use open_sampler::editor::Ptrs;
use open_sampler::params::SamplerParams;
use open_sampler::view::{self, SampleView, ViewState};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).map(String::as_str).unwrap_or("open-sampler.png");
    let tab: u8 = args.get(2).and_then(|t| t.parse().ok()).unwrap_or(0);
    let sample = args.get(3).map(|p| os_engine::sample::load(std::path::Path::new(p)).expect("sample"));
    let file = args.get(3).map(|p| p.rsplit(['/', '\\']).next().unwrap().to_string()).unwrap_or_default();
    let params = SamplerParams::default();
    let ptrs = Ptrs::new(&params);
    let mut c = os_ui::Canvas::new(view::W, view::H, 1.0).unwrap();
    let st = ViewState { tab, ..ViewState::default() };
    view::paint(&mut c, &ptrs, &st, &SampleView { sample: sample.as_ref(), file: &file }, "");
    let mut buf = vec![0u32; (view::W * view::H) as usize];
    c.copy_to_xrgb(&mut buf);
    // The editor the host shows (cropped); pass `full` as a 4th argument for the whole painted area.
    let full = args.get(4).is_some_and(|a| a == "full");
    let (ox, oy, w, h) = if full { (0, 0, view::W, view::H) } else { (view::CROP_X, view::CROP_Y, view::EDITOR_W, view::EDITOR_H) };
    let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
    for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
        let (x, y) = (i as u32 % w + ox, i as u32 / w + oy);
        let v = buf[(y * view::W + x) as usize];
        *px = tiny_skia::PremultipliedColorU8::from_rgba((v >> 16) as u8, (v >> 8) as u8, v as u8, 255).unwrap();
    }
    pm.save_png(out).unwrap();
}
