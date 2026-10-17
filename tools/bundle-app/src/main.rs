// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! `cargo run -p bundle-app [-- --install]` (macOS): builds Lemon Studio in release mode and writes
//! `<target>/bundled/Lemon Studio.app`: the program, its icon and an `Info.plist` that lets the Finder open `.flp`
//! projects with it. The icon is made from the Icon Composer document `assets/logo/macOS.icon` by
//! `tools/logo/macos-icon.sh`: `assets/lemonstudio.car` (the bundle's `Assets.car`) is the icon macOS 26 and later draw,
//! in every appearance, and `assets/lemonstudio.icns` the one older macOS shows. The bundle is signed ad hoc, as Apple
//! silicon requires of any program it runs. `--install` copies it to `~/Applications`.
//!
//! The macOS counterpart of Linux's desktop entry (`crates/os-app/assets/lemonstudio.desktop`).

use std::path::PathBuf;
use std::process::Command;

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
    let assets = root.join("crates/os-app/assets");
    std::fs::copy(assets.join(format!("{EXE}.car")), resources.join("Assets.car")).expect("copy the icon");
    std::fs::copy(assets.join(format!("{EXE}.icns")), resources.join(format!("{EXE}.icns"))).expect("copy the icon");
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
	<key>CFBundleIconName</key>
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
