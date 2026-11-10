// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Embeds `assets/lemonstudio.ico` (a nightly's: `lemonstudio-nightly.ico`) as icon resource 1 and the version
//! (`LEMON_VERSION`, else the workspace's) as the version resource (Windows). Uses the Windows SDK's `rc.exe` (or `$RC`);
//! without it the build goes on and the exe just has no icon. Also sets `cfg(plugin_windows)` where plugin windows exist,
//! `cfg(lemon_updater)` when `LEMON_UPDATER` names the updater to carry (release builds, `crate::update`), and
//! `cfg(lemon_nightly)` when `LEMON_CHANNEL` is `nightly` (`crate::version`).

use std::path::PathBuf;
use std::process::Command;

fn find_rc() -> Option<PathBuf> {
    if let Ok(rc) = std::env::var("RC") {
        return Some(rc.into());
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        _ => "x64",
    };
    let pf = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
    let bin = PathBuf::from(pf).join(r"Windows Kits\10\bin");
    let mut vers: Vec<PathBuf> = std::fs::read_dir(&bin).ok()?.flatten().map(|e| e.path()).collect();
    vers.sort();
    vers.into_iter().rev().map(|v| v.join(arch).join("rc.exe")).find(|p| p.exists())
}

/// The factory presets in `factory-presets/<plugin>/[<folder>/]<name>.ospreset`, as a Rust slice expression of (path
/// under that folder, `/`-separated; bytes) for `presets.rs` to include (spec: plugins/presets.md "Factory presets").
fn factory_presets() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("factory-presets");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    let files = |dir: &std::path::Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        v.sort();
        v
    };
    let name = |p: &std::path::Path| p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let preset = |p: &std::path::Path| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ospreset"));
    for plugin in files(&root).into_iter().filter(|p| p.is_dir()) {
        for p in files(&plugin) {
            if preset(&p) {
                found.push((format!("{}/{}", name(&plugin), name(&p)), p));
            } else if p.is_dir() {
                for q in files(&p).into_iter().filter(|q| preset(q)) {
                    found.push((format!("{}/{}/{}", name(&plugin), name(&p), name(&q)), q));
                }
            }
        }
    }
    let mut out = String::from("&[\n");
    for (rel, path) in found {
        out.push_str(&format!("    ({rel:?}, include_bytes!({:?}) as &[u8]),\n", path.display().to_string()));
    }
    out.push(']');
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("factory_presets.rs"), out).unwrap();
}

/// Release builds: `LEMON_UPDATER` names the `lemonstudio-updater` built just before, which the program carries
/// (`update::UPDATER`, through `LEMON_UPDATER_FILE`). A build from source has none.
fn updater() {
    println!("cargo::rustc-check-cfg=cfg(lemon_updater)");
    println!("cargo:rerun-if-env-changed=LEMON_UPDATER");
    let Some(path) = std::env::var_os("LEMON_UPDATER").filter(|p| !p.is_empty()) else { return };
    let path = std::path::absolute(PathBuf::from(path)).expect("LEMON_UPDATER");
    assert!(path.is_file(), "LEMON_UPDATER names no file: {}", path.display());
    println!("cargo:rerun-if-changed={}", path.display());
    println!("cargo::rustc-env=LEMON_UPDATER_FILE={}", path.display());
    println!("cargo::rustc-cfg=lemon_updater");
}

/// The program's version: the release workflow's `LEMON_VERSION`, else the workspace's (`crate::version`).
fn version() -> String {
    println!("cargo:rerun-if-env-changed=LEMON_VERSION");
    std::env::var("LEMON_VERSION").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap())
}

/// Whether this is a nightly build (`crate::version::NIGHTLY`).
fn nightly() -> bool {
    println!("cargo::rustc-check-cfg=cfg(lemon_nightly)");
    println!("cargo:rerun-if-env-changed=LEMON_CHANNEL");
    let nightly = std::env::var("LEMON_CHANNEL").is_ok_and(|c| c == "nightly");
    if nightly {
        println!("cargo::rustc-cfg=lemon_nightly");
    }
    nightly
}

/// Windows: the resource script, the icon and the version resource (four numbers: a nightly's `YEAR.MONTH.DAY.N`
/// fits as it is, a semantic version gets a 0 at the end). `name` is the program's (`crate::version::NAME`).
fn resource_script(ico: &std::path::Path, version: &str, name: &str) -> String {
    let mut n: Vec<u16> = version.split(['-', '+']).next().unwrap_or("").split('.').map(|p| p.parse().unwrap_or(0)).collect();
    n.resize(4, 0);
    let nums = format!("{},{},{},{}", n[0], n[1], n[2], n[3]);
    let ico = ico.display().to_string().replace('\\', "\\\\");
    let text = version.replace('"', "");
    format!(
        r#"1 ICON "{ico}"

1 VERSIONINFO
FILEVERSION {nums}
PRODUCTVERSION {nums}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "The Lemon Studio contributors"
      VALUE "FileDescription", "{name}"
      VALUE "FileVersion", "{text}"
      VALUE "InternalName", "lemonstudio"
      VALUE "LegalCopyright", "Copyright (C) 2026 ItsAlphaNeon and the Lemon Studio contributors. GPL-3.0-or-later."
      VALUE "OriginalFilename", "lemonstudio.exe"
      VALUE "ProductName", "{name}"
      VALUE "ProductVersion", "{text}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}

fn main() {
    factory_presets();
    updater();
    let version = version();
    let nightly = nightly();
    // `plugin_windows`: plugin editors open in Lemon Studio's frame windows (Win32 on Windows, X11 on Linux, AppKit on
    // macOS).
    println!("cargo::rustc-check-cfg=cfg(plugin_windows)");
    if matches!(std::env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("windows" | "linux" | "macos")) {
        println!("cargo::rustc-cfg=plugin_windows");
    }
    println!("cargo:rerun-if-changed=assets/lemonstudio.rc");
    println!("cargo:rerun-if-changed=assets/lemonstudio.ico");
    println!("cargo:rerun-if-changed=assets/lemonstudio-nightly.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found (Windows SDK); building without the app icon");
        return;
    };
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let out = out_dir.join("lemonstudio.res");
    let (ico, name) = if nightly { ("assets/lemonstudio-nightly.ico", "Lemon Studio Nightly") } else { ("assets/lemonstudio.ico", "Lemon Studio") };
    let ico = std::path::absolute(ico).expect("icon path");
    let script = out_dir.join("lemonstudio.rc");
    std::fs::write(&script, resource_script(&ico, &version, name)).expect("resource script");
    let ok = Command::new(&rc)
        .args(["/nologo", "/fo"])
        .arg(&out)
        .arg(&script)
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        println!("cargo:rustc-link-arg-bins={}", out.display());
    } else {
        println!("cargo:warning=rc.exe failed; building without the app icon");
    }
}
