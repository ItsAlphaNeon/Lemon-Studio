// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! `cargo run -p bundle-plugins [-- --install]`: builds every Lemon Studio plugin in release mode (in
//! `<target>/plugins`) and writes `<target>/bundled/<Name>.vst3` (bundle folder) and `<Name>.clap` (a bundle folder too
//! on macOS, ad-hoc signed like the VST3 one so Apple silicon loads them); `--install` copies them to the per-user
//! plugin folders (no admin rights needed): `%LOCALAPPDATA%\Programs\Common\{VST3,CLAP}` on Windows, `~/.vst3` and `~/.clap` on Linux,
//! `~/Library/Audio/Plug-Ins/{VST3,CLAP}` on macOS. A plugin whose installed copy is already this build is left as it
//! is, so the run from source (`cargo run -p os-app --release`, which installs them before it starts) costs little when
//! nothing changed, and the plugin scan has nothing new to look at.
//!
// spec: plugins/plugin-formats.md (bundles and install folders)

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// (cargo package, library file stem, display name).
const PLUGINS: &[(&str, &str, &str)] =
    &[("lemon-sampler", "lemon_sampler", "Lemon Sampler"), ("lemon-parametric-eq-2", "lemon_parametric_eq_2", "Lemon Parametric EQ"), ("lemon-3x-osc", "lemon_3x_osc", "Lemon Trio"), ("lemon-limiter", "lemon_limiter", "Lemon Limiter"), ("lemon-reverb", "lemon_reverb", "Lemon Reverb"), ("lemon-delay", "lemon_delay", "Lemon Delay"), ("lemon-soft-clipper", "lemon_soft_clipper", "Lemon Soft Clipper"), ("lemon-fast-distortion", "lemon_fast_distortion", "Lemon Fast Distortion"), ("lemon-balance", "lemon_balance", "Lemon Balance"), ("lemon-stereo-enhancer", "lemon_stereo_enhancer", "Lemon Stereo Enhancer"), ("lemon-transient-processor", "lemon_transient_processor", "Lemon Transient Processor"), ("lemon-chorus", "lemon_chorus", "Lemon Chorus"), ("lemon-stereo-shaper", "lemon_stereo_shaper", "Lemon Stereo Shaper"), ("lemon-filter", "lemon_filter", "Lemon Filter"), ("citrus", "citrus", "Citrus"), ("tesla", "tesla", "Tesla"), ("pulp-filter", "pulp_filter", "Pulp Filter"), ("press", "press", "Press"), ("lemon-flanger", "lemon_flanger", "Lemon Flanger"), ("lemon-phaser", "lemon_phaser", "Lemon Phaser"), ("lemon-peak-controller", "lemon_peak_controller", "Lemon Peak Controller")];
/// The plugins' earlier display names: an install removes bundles left under them (same plugin ids, so a DAW would
/// list each plugin twice).
const OLD_NAMES: &[&str] = &["Lemon Parametric EQ 2", "Lemon 3x Osc", "Open Sampler", "Open Parametric EQ 2", "Open 3x Osc"];

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() { copy_dir(&e.path(), &to)? } else { std::fs::copy(e.path(), to).map(|_| ())? }
    }
    Ok(())
}

fn main() {
    let install = std::env::args().any(|a| a == "--install");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repository root");
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
    let target = if target.is_relative() { root.join(target) } else { target };
    let out = target.join("bundled");
    std::fs::create_dir_all(&out).expect("bundled folder");
    // A build folder of their own: Lemon Studio links some plugins with their `linked` feature, and a plugin library's
    // file names carry no hash, so the app's build and this one would each overwrite the other's and rebuild every time.
    let build = target.join("plugins");
    for (pkg, stem, name) in PLUGINS {
        let ok = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["build", "--release", "--lib", "-p", pkg])
            .current_dir(&root)
            .env("CARGO_TARGET_DIR", &build)
            .status()
            .expect("cargo")
            .success();
        assert!(ok, "build of {pkg} failed");
        let lib = build.join("release").join(library_file(stem));
        let built = std::fs::metadata(&lib).and_then(|m| m.modified()).expect("library time stamp");
        let vst3 = out.join(format!("{name}.vst3"));
        let clap = out.join(format!("{name}.clap"));
        if cfg!(target_os = "macos") {
            mac_bundle(&lib, &vst3, pkg, name);
            mac_bundle(&lib, &clap, pkg, name);
        } else {
            let arch = vst3.join("Contents").join(BUNDLE_ARCH);
            std::fs::create_dir_all(&arch).expect("vst3 bundle");
            std::fs::copy(&lib, arch.join(bundle_module(name))).expect("copy vst3");
            std::fs::copy(&lib, &clap).expect("copy clap");
        }
        stamp(&module(&vst3, name), built);
        stamp(&module(&clap, name), built);
        println!("{}\n{}", vst3.display(), clap.display());
        if install {
            let (vst3_dir, clap_dir) = install_dirs();
            for old in OLD_NAMES {
                for path in [vst3_dir.join(format!("{old}.vst3")), clap_dir.join(format!("{old}.clap"))] {
                    let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
                }
            }
            let copies = [(&vst3, vst3_dir.join(format!("{name}.vst3"))), (&clap, clap_dir.join(format!("{name}.clap")))];
            if copies.iter().all(|(from, to)| same_build(&module(from, name), &module(to, name))) {
                println!("{name} is installed and up to date");
                continue;
            }
            for (from, to) in &copies {
                // Replaced whole, never rewritten in place: a file left from an older build would break a macOS
                // bundle's signature, and on Linux and macOS a program that has the old build loaded keeps it intact.
                let _ = if to.is_dir() { std::fs::remove_dir_all(to) } else { std::fs::remove_file(to) };
                if from.is_dir() {
                    copy_dir(from, to).expect("install bundle");
                } else {
                    std::fs::create_dir_all(to.parent().expect("plugin folder")).expect("plugin folder");
                    std::fs::copy(from, to).expect("install plugin");
                }
                stamp(&module(to, name), built);
            }
            println!("installed to {} and {}", vst3_dir.display(), clap_dir.display());
        }
    }
}

/// The plugin's module inside `bundle` (`.vst3` or `.clap`): a CLAP plugin is the module itself outside macOS.
fn module(bundle: &Path, name: &str) -> PathBuf {
    if !cfg!(target_os = "macos") && bundle.extension().is_some_and(|e| e == "clap") {
        bundle.to_path_buf()
    } else {
        bundle.join("Contents").join(BUNDLE_ARCH).join(bundle_module(name))
    }
}

/// Gives a module the time stamp of the library it was made from, whatever the copy gave it: copies of one build then
/// share it, which tells an install whether its copy is current.
fn stamp(module: &Path, built: SystemTime) {
    std::fs::File::options().write(true).open(module).and_then(|f| f.set_modified(built)).expect("module time stamp");
}

/// Whether the module `copy` is the same build as `module`: the same size and time stamp.
fn same_build(module: &Path, copy: &Path) -> bool {
    let meta = |p: &Path| std::fs::metadata(p).and_then(|m| Ok((m.len(), m.modified()?))).ok();
    meta(copy).is_some_and(|c| meta(module) == Some(c))
}

/// The bundle folder holding the module: `Contents/<architecture>-<platform>` (VST 3 bundle layout), `Contents/MacOS`
/// on macOS.
const BUNDLE_ARCH: &str = if cfg!(target_os = "macos") {
    "MacOS"
} else if cfg!(windows) {
    if cfg!(target_arch = "aarch64") { "arm64-win" } else { "x86_64-win" }
} else if cfg!(target_arch = "aarch64") {
    "aarch64-linux"
} else {
    "x86_64-linux"
};

/// The shared library cargo writes for a `cdylib` named `stem`.
fn library_file(stem: &str) -> String {
    format!("{}{stem}{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX)
}

/// The module's name inside the bundle: `<Name>.vst3` on Windows, `<Name>.so` on Linux, `<Name>` on macOS.
fn bundle_module(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.vst3")
    } else if cfg!(target_os = "macos") {
        name.to_owned()
    } else {
        format!("{name}{}", std::env::consts::DLL_SUFFIX)
    }
}

/// A macOS plugin bundle at `bundle` (`.vst3` or `.clap`, written afresh): `Contents/Info.plist`, `Contents/PkgInfo`
/// and the module in `Contents/MacOS`, ad-hoc signed (Apple silicon loads no unsigned code).
fn mac_bundle(lib: &Path, bundle: &Path, pkg: &str, name: &str) {
    if bundle.is_dir() {
        std::fs::remove_dir_all(bundle).expect("old bundle");
    } else if bundle.exists() {
        std::fs::remove_file(bundle).expect("old bundle");
    }
    let contents = bundle.join("Contents");
    let arch = contents.join(BUNDLE_ARCH);
    std::fs::create_dir_all(&arch).expect("bundle folder");
    std::fs::copy(lib, arch.join(bundle_module(name))).expect("copy module");
    let format = bundle.extension().and_then(|e| e.to_str()).unwrap_or_default();
    std::fs::write(contents.join("PkgInfo"), "BNDL????").expect("PkgInfo");
    std::fs::write(contents.join("Info.plist"), info_plist(&format!("org.lemonstudio.{pkg}.{format}"), name)).expect("Info.plist");
    let signed = Command::new("codesign").args(["--force", "--sign", "-"]).arg(bundle).status().is_ok_and(|s| s.success());
    assert!(signed, "could not sign {}", bundle.display());
}

/// A plugin bundle's `Info.plist`.
fn info_plist(id: &str, name: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>English</string>
    <key>CFBundleExecutable</key>
    <string>{name}</string>
    <key>CFBundleIdentifier</key>
    <string>{id}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>{name}</string>
    <key>CFBundleDisplayName</key>
    <string>{name}</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
"#
    )
}

/// The per-user VST3 and CLAP folders.
fn install_dirs() -> (PathBuf, PathBuf) {
    if cfg!(windows) {
        let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Programs").join("Common");
        (local.join("VST3"), local.join("CLAP"))
    } else if cfg!(target_os = "macos") {
        let plugins = PathBuf::from(std::env::var_os("HOME").expect("HOME")).join("Library").join("Audio").join("Plug-Ins");
        (plugins.join("VST3"), plugins.join("CLAP"))
    } else {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        (home.join(".vst3"), home.join(".clap"))
    }
}
