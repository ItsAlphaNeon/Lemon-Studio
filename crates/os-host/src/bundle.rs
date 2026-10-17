// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! macOS plugin bundles: a `.clap` or `.vst3` plugin is a bundle folder (`Name.clap/Contents/MacOS/<executable>`) whose
//! `Info.plist` names the module to load (`CFBundleExecutable`). Core Foundation reads the bundle, as other hosts do;
//! VST 3 modules also get the bundle itself (`bundleEntry(CFBundleRef)`).
// spec: plugins/plugin-formats.md, plugins/vst3-hosting.md §2-§3

use std::ffi::c_void;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

type CFTypeRef = *const c_void;
type CFIndex = isize;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFURLCreateFromFileSystemRepresentation(alloc: CFTypeRef, buffer: *const u8, len: CFIndex, is_directory: u8) -> CFTypeRef;
    fn CFURLGetFileSystemRepresentation(url: CFTypeRef, resolve_against_base: u8, buffer: *mut u8, max_len: CFIndex) -> u8;
    fn CFBundleCreate(alloc: CFTypeRef, url: CFTypeRef) -> CFTypeRef;
    fn CFBundleCopyExecutableURL(bundle: CFTypeRef) -> CFTypeRef;
}

/// A bundle opened through Core Foundation (`CFBundleRef`), released when dropped.
pub struct Bundle(CFTypeRef);

impl Bundle {
    /// The bundle at folder `path`, when it is one.
    pub fn open(path: &Path) -> Option<Bundle> {
        if !path.is_dir() {
            return None;
        }
        let bytes = path.as_os_str().as_bytes();
        unsafe {
            let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as CFIndex, 1);
            if url.is_null() {
                return None;
            }
            let b = CFBundleCreate(std::ptr::null(), url);
            CFRelease(url);
            (!b.is_null()).then_some(Bundle(b))
        }
    }

    /// The `CFBundleRef`, valid while `self` lives.
    pub fn as_ptr(&self) -> *mut c_void {
        self.0 as *mut c_void
    }

    /// The module file its `Info.plist` names (`CFBundleExecutable`).
    pub fn executable(&self) -> Option<PathBuf> {
        unsafe {
            let url = CFBundleCopyExecutableURL(self.0);
            if url.is_null() {
                return None;
            }
            let mut buf = vec![0u8; 4096];
            let ok = CFURLGetFileSystemRepresentation(url, 1, buf.as_mut_ptr(), buf.len() as CFIndex) != 0;
            CFRelease(url);
            let end = buf.iter().position(|&b| b == 0)?;
            ok.then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..end])))
        }
    }
}

impl Drop for Bundle {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

/// The module to load for plugin `path`: a bundle's executable (`Contents/MacOS/<CFBundleExecutable>`, or the bundle's
/// name there when its `Info.plist` does not say), else `path` itself.
pub fn module_file(path: &Path) -> PathBuf {
    if !path.is_dir() {
        return path.to_path_buf();
    }
    let dir = path.join("Contents").join("MacOS");
    // Core Foundation gives the path resolved (`/private/var/...`): the name, in the folder as the caller wrote it.
    let named = Bundle::open(path).and_then(|b| b.executable()).and_then(|e| e.file_name().map(|n| dir.join(n)));
    named.filter(|p| p.is_file()).unwrap_or_else(|| dir.join(path.file_stem().unwrap_or_default()))
}

#[cfg(test)]
mod tests { // This code is a part of L e m o n S t u d i o.
    use super::*;

    /// A bundle's executable comes from its `Info.plist`; without one, the bundle's name in `Contents/MacOS`.
    #[test]
    fn bundle_modules() {
        let d = std::env::temp_dir().join(format!("os-host-bundle-{}", std::process::id()));
        let named = d.join("Synth.clap");
        std::fs::create_dir_all(named.join("Contents/MacOS")).unwrap();
        std::fs::write(named.join("Contents/MacOS/SynthBinary"), b"").unwrap();
        std::fs::write(
            named.join("Contents/Info.plist"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>SynthBinary</string>\
             <key>CFBundlePackageType</key><string>BNDL</string></dict></plist>",
        )
        .unwrap();
        let bare = d.join("Bare.vst3");
        std::fs::create_dir_all(bare.join("Contents/MacOS")).unwrap();
        let module = module_file(&named);
        assert_eq!(module.file_name().unwrap(), "SynthBinary");
        assert!(module.is_file());
        assert_eq!(module_file(&bare), bare.join("Contents/MacOS/Bare"));
        let file = d.join("plain.dylib");
        assert_eq!(module_file(&file), file);
        let _ = std::fs::remove_dir_all(&d);
    }
}
