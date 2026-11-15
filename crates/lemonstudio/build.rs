// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Embeds `os-app/assets/lemonstudio.ico` (a nightly's: `lemonstudio-nightly.ico`) as icon resource 1 and the version
//! (`LEMON_VERSION`, else the workspace's) as the version resource (Windows). Uses the Windows SDK's `rc.exe` (or `$RC`);
//! without it the build goes on and the exe just has no icon. Also sets `cfg(plugin_windows)` where plugin windows exist,
//! `cfg(lemon_nightly)` when `LEMON_CHANNEL` is `nightly` (the icon).

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

/// The program's version: the release workflow's `LEMON_VERSION`, else the workspace's (`crate::version`).
fn version() -> String {
    println!("cargo:rerun-if-env-changed=LEMON_VERSION");
    std::env::var("LEMON_VERSION").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap())
}

/// Whether this is a nightly build (the icon; `os_session::version::NIGHTLY` has its own).
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
    let version = version();
    let nightly = nightly();
    // `plugin_windows`: plugin editors open in Lemon Studio's frame windows (Win32 on Windows, X11 on Linux, AppKit on
    // macOS), with the `plugin-windows` feature.
    println!("cargo::rustc-check-cfg=cfg(plugin_windows)");
    let os = matches!(std::env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("windows" | "linux" | "macos"));
    if os && std::env::var_os("CARGO_FEATURE_PLUGIN_WINDOWS").is_some() {
        println!("cargo::rustc-cfg=plugin_windows");
    }
    println!("cargo:rerun-if-changed=../os-app/assets/lemonstudio.rc");
    println!("cargo:rerun-if-changed=../os-app/assets/lemonstudio.ico");
    println!("cargo:rerun-if-changed=../os-app/assets/lemonstudio-nightly.ico");
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
    let (ico, name) = if nightly { ("../os-app/assets/lemonstudio-nightly.ico", "Lemon Studio Nightly") } else { ("../os-app/assets/lemonstudio.ico", "Lemon Studio") };
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
