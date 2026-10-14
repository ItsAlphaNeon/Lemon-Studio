//! `cargo run -p bundle-app [-- --install]` (macOS): builds Lemon Studio in release mode and writes
//! `<target>/bundled/Lemon Studio.app`: the program, its icon (made from `crates/os-app/assets/logo/icon-lemon.png`) and
//! an `Info.plist` that lets the Finder open `.flp` projects with it. The bundle is signed ad hoc, as Apple silicon
//! requires of any program it runs. `--install` copies it to `~/Applications`.
//!
//! The macOS counterpart of Linux's desktop entry (`crates/os-app/assets/lemonstudio.desktop`).

use std::path::{Path, PathBuf};
use std::process::Command;

use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

const NAME: &str = "Lemon Studio";
const EXE: &str = "lemonstudio";
const IDENTIFIER: &str = "io.github.itsalphaneon.lemonstudio";

fn main() {
    if !cfg!(target_os = "macos") {
        eprintln!("bundle-app: app bundles are a macOS thing; on Linux install the desktop entry (see the README)");
        std::process::exit(1);
    }
    let install = std::env::args().any(|a| a == "--install");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repository root");
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
    let target = if target.is_relative() { root.join(target) } else { target };
    let ok = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "--bin", EXE, "-p", "os-app"])
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", &target)
        .status()
        .expect("cargo")
        .success();
    assert!(ok, "build of os-app failed");

    let app = target.join("bundled").join(format!("{NAME}.app"));
    if app.exists() {
        std::fs::remove_dir_all(&app).expect("remove the old bundle");
    }
    let contents = app.join("Contents");
    let (macos, resources) = (contents.join("MacOS"), contents.join("Resources"));
    std::fs::create_dir_all(&macos).expect("bundle folders");
    std::fs::create_dir_all(&resources).expect("bundle folders");
    std::fs::copy(target.join("release").join(EXE), macos.join(EXE)).expect("copy the program");
    std::fs::write(contents.join("Info.plist"), info_plist()).expect("Info.plist");
    std::fs::write(contents.join("PkgInfo"), "APPL????").expect("PkgInfo");
    icon(&root.join("crates/os-app/assets/logo/icon-lemon.png"), &target.join("bundled"), &resources.join(format!("{EXE}.icns")));
    run(Command::new("codesign").args(["--force", "--sign", "-"]).arg(&app), "codesign");
    println!("{}", app.display());

    if install {
        let dir = PathBuf::from(std::env::var_os("HOME").expect("HOME")).join("Applications");
        let to = dir.join(format!("{NAME}.app"));
        if to.exists() {
            std::fs::remove_dir_all(&to).expect("remove the installed bundle");
        }
        std::fs::create_dir_all(&dir).expect("~/Applications");
        // `ditto` keeps the signature's extended attributes.
        run(Command::new("ditto").arg(&app).arg(&to), "ditto");
        println!("installed to {}", to.display());
    }
}

fn run(cmd: &mut Command, what: &str) {
    assert!(cmd.status().unwrap_or_else(|e| panic!("{what}: {e}")).success(), "{what} failed");
}

fn info_plist() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>{NAME}</string>
	<key>CFBundleDisplayName</key>
	<string>{NAME}</string>
	<key>CFBundleExecutable</key>
	<string>{EXE}</string>
	<key>CFBundleIdentifier</key>
	<string>{IDENTIFIER}</string>
	<key>CFBundleIconFile</key>
	<string>{EXE}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.music</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>CFBundleDocumentTypes</key>
	<array>
		<dict>
			<key>CFBundleTypeName</key>
			<string>FL Studio project</string>
			<key>CFBundleTypeRole</key>
			<string>Editor</string>
			<key>LSHandlerRank</key>
			<string>Alternate</string>
			<key>CFBundleTypeExtensions</key>
			<array>
				<string>flp</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
"#
    )
}

/// Writes the app icon: `src` (transparent margin cropped, centred on a square) in every size an `.icns` holds, at 80 %
/// of the cell as macOS icons are drawn, through `iconutil`.
fn icon(src: &Path, scratch: &Path, out: &Path) {
    let png = Pixmap::load_png(src).expect("icon-lemon.png");
    let (x0, y0, x1, y1) = alpha_bounds(&png);
    let set = scratch.join(format!("{EXE}.iconset"));
    let _ = std::fs::remove_dir_all(&set);
    std::fs::create_dir_all(&set).expect("iconset folder");
    for (px, name) in [
        (16, "16x16"),
        (32, "16x16@2x"),
        (32, "32x32"),
        (64, "32x32@2x"),
        (128, "128x128"),
        (256, "128x128@2x"),
        (256, "256x256"),
        (512, "256x256@2x"),
        (512, "512x512"),
        (1024, "512x512@2x"),
    ] {
        let mut cell = Pixmap::new(px, px).expect("icon cell");
        let (w, h) = ((x1 - x0) as f32, (y1 - y0) as f32);
        let k = px as f32 * 0.8 / w.max(h);
        let (dx, dy) = ((px as f32 - w * k) / 2.0, (px as f32 - h * k) / 2.0);
        let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
        let t = Transform::from_translate(-(x0 as f32), -(y0 as f32)).post_scale(k, k).post_translate(dx, dy);
        cell.draw_pixmap(0, 0, png.as_ref(), &paint, t, None);
        cell.save_png(set.join(format!("icon_{name}.png"))).expect("icon png");
    }
    run(Command::new("iconutil").args(["-c", "icns", "-o"]).arg(out).arg(&set), "iconutil");
    let _ = std::fs::remove_dir_all(&set);
}

/// The box (x0, y0, x1, y1; end-exclusive) of the pixels that are not fully transparent.
fn alpha_bounds(p: &Pixmap) -> (u32, u32, u32, u32) {
    let (w, h) = (p.width(), p.height());
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (i, c) in p.pixels().iter().enumerate() {
        if c.alpha() > 0 {
            let (x, y) = (i as u32 % w, i as u32 / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
    }
    if x1 <= x0 { (0, 0, w, h) } else { (x0, y0, x1, y1) }
}
