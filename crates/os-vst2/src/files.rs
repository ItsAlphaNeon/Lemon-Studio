// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Finding VST 2 plugin files: the usual folders, and which files are plugins.
//!
//! VST 2 plugins have no extension of their own outside macOS: on Windows they are DLLs and on Linux shared objects, next
//! to libraries that are not plugins. A file counts as a plugin when its export table names an entry point
//! (`VSTPluginMain`, or `main` in older plugins), read from the file without loading it, so the scan never starts a
//! scanner for an ordinary library. On macOS a plugin is a `.vst` bundle.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// The entry points a plugin file exports, newest first.
pub const ENTRY_POINTS: [&str; 2] = ["VSTPluginMain", "main"];

/// The extension of plugin files (bundle folders on macOS).
pub const EXTENSION: &str = if cfg!(windows) {
    "dll"
} else if cfg!(target_os = "macos") {
    "vst"
} else {
    "so"
};

/// The folders VST 2 plugins are usually installed in (there is no single standard one).
///
/// Windows: the folder installers record in the registry, then those of the usual ones that exist (at least
/// `Program Files\VSTPlugins`). The `Program Files (x86)` ones are among them: many installers put 64-bit plugins
/// there too (32-bit ones found there are listed as such). macOS: `~/Library/Audio/Plug-Ins/VST` and
/// `/Library/Audio/Plug-Ins/VST`. Linux: `VST_PATH` when it is set, else `~/.vst`, `/usr/lib/vst`, `/usr/local/lib/vst`
/// and the `lxvst` ones that exist.
pub fn standard_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        v.extend(registry_dir());
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        let program_files = env("ProgramFiles");
        let program_files_x86 = env("ProgramFiles(x86)");
        let common = env("CommonProgramFiles");
        let usual = [
            program_files.as_ref().map(|p| p.join("VSTPlugins")),
            program_files.as_ref().map(|p| p.join("Steinberg").join("VSTPlugins")),
            common.as_ref().map(|c| c.join("VST2")),
            common.as_ref().map(|c| c.join("Steinberg").join("VST2")),
            program_files_x86.as_ref().map(|p| p.join("VSTPlugins")),
            program_files_x86.as_ref().map(|p| p.join("Steinberg").join("VSTPlugins")),
        ];
        let first = usual[0].clone();
        v.extend(usual.into_iter().flatten().filter(|p| p.is_dir()));
        if v.is_empty() {
            v.extend(first);
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(h) = std::env::var_os("HOME") {
            v.push(PathBuf::from(h).join("Library/Audio/Plug-Ins/VST"));
        }
        v.push(PathBuf::from("/Library/Audio/Plug-Ins/VST"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        match std::env::var_os("VST_PATH").filter(|p| !p.is_empty()) {
            Some(list) => v.extend(std::env::split_paths(&list)),
            None => {
                let home = std::env::var_os("HOME").map(PathBuf::from);
                v.extend(home.as_ref().map(|h| h.join(".vst")));
                v.push(PathBuf::from("/usr/lib/vst"));
                v.push(PathBuf::from("/usr/local/lib/vst"));
                let lxvst = [home.map(|h| h.join(".lxvst")), Some(PathBuf::from("/usr/lib/lxvst")), Some(PathBuf::from("/usr/local/lib/lxvst"))];
                v.extend(lxvst.into_iter().flatten().filter(|p| p.is_dir()));
            }
        }
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for d in v {
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// `HKEY_LOCAL_MACHINE\SOFTWARE\VST`, value `VSTPluginsPath`: the folder plugin installers put VST 2 plugins in.
#[cfg(windows)]
fn registry_dir() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let key: Vec<u16> = "SOFTWARE\\VST\0".encode_utf16().collect();
    let name: Vec<u16> = "VSTPluginsPath\0".encode_utf16().collect();
    let mut buf = vec![0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: the buffer holds `bytes` bytes; the strings are NUL-terminated. A value with environment variables in it
    // comes back expanded.
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut bytes) };
    if r != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let p = PathBuf::from(std::ffi::OsString::from_wide(&buf[..len]));
    (!p.as_os_str().is_empty()).then_some(p)
}

/// The file to load for plugin `path`: a `.vst` bundle's executable on macOS, else the file itself.
pub fn module_file(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    return os_host::bundle::module_file(path);
    #[cfg(not(target_os = "macos"))]
    {
        path.to_path_buf()
    }
}

/// Whether `p` has the plugin extension of this platform (a folder on macOS, a file elsewhere).
pub fn has_extension(p: &Path) -> bool {
    p.extension().is_some_and(|x| x.eq_ignore_ascii_case(EXTENSION)) && if cfg!(target_os = "macos") { p.is_dir() } else { p.is_file() }
}

/// Whether `p` is a VST 2 plugin file of this platform: a `.vst` bundle on macOS; elsewhere a library with the plugin
/// extension whose export table names an entry point (read without loading the file).
pub fn is_plugin(p: &Path) -> bool {
    if !has_extension(p) {
        return false;
    }
    if cfg!(target_os = "macos") {
        return true;
    }
    let Ok(mut f) = File::open(p) else { return false };
    exports(&mut f).is_some_and(|names| names.iter().any(|n| ENTRY_POINTS.contains(&n.as_str())))
}

/// Why plugin `path` cannot load in this program, when its file tells without being loaded: a 32-bit Windows plugin in
/// 64-bit Lemon Studio, or a plugin built for Intel Macs only in Lemon Studio on Apple silicon. Such a plugin is
/// reported with this reason instead of with the system's loader error, and is not tried in a scanner.
pub fn unsupported(path: &Path) -> Option<String> {
    let file = module_file(path);
    if cfg!(all(windows, target_pointer_width = "64")) && pe_machine(&file) == Some("x86") {
        return Some("a 32-bit plugin, which a 64-bit program cannot load".into());
    }
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) && macho_cpus(&file).is_some_and(|c| !c.is_empty() && !c.contains(&CPU_ARM64)) {
        return Some("built for Intel Macs only, which Lemon Studio on Apple silicon cannot load".into());
    }
    None
}

/// Mach-O CPU types: 64-bit Intel and Apple silicon.
pub const CPU_X86_64: u32 = 0x0100_0007;
pub const CPU_ARM64: u32 = 0x0100_000C;

/// The CPU types a Mach-O file is built for: one for a thin file, each slice's for a universal one. `None` when `p` is
/// not a Mach-O file.
pub fn macho_cpus(p: &Path) -> Option<Vec<u32>> {
    let mut f = File::open(p).ok()?;
    let head = read_at(&mut f, 0, 8)?;
    let be = |b: &[u8], at: usize| Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?));
    match be(&head, 0)? {
        // Thin files, little-endian: 64-bit and 32-bit.
        0xCFFA_EDFE | 0xCEFA_EDFE => Some(vec![u32_at(&head, 4)?]),
        // Universal files (big-endian headers): `fat_arch` entries of 20 bytes, `fat_arch_64` of 32.
        magic @ (0xCAFE_BABE | 0xCAFE_BABF) => {
            let n = (be(&head, 4)? as usize).min(32);
            let size = if magic == 0xCAFE_BABE { 20 } else { 32 };
            let table = read_at(&mut f, 8, n * size)?;
            (0..n).map(|i| be(&table, i * size)).collect()
        }
        _ => None,
    }
}

/// The architecture of a Windows library ("x86" for a 32-bit one), when `p` is one.
pub fn pe_machine(p: &Path) -> Option<&'static str> {
    let mut f = File::open(p).ok()?;
    let pe = pe_header(&mut f)?;
    Some(match u16_at(&read_at(&mut f, pe + 4, 2)?, 0)? {
        0x014C => "x86",
        0x8664 => "x86_64",
        0xAA64 => "arm64",
        _ => "unknown",
    })
}

/// The names a library exports: a PE file's (Windows) or an ELF file's defined dynamic symbols (Linux).
pub fn exports<R: Read + Seek>(f: &mut R) -> Option<Vec<String>> {
    let magic = read_at(f, 0, 4)?;
    if magic.starts_with(b"MZ") {
        pe_exports(f)
    } else if magic == b"\x7FELF" {
        elf_exports(f)
    } else {
        None
    }
}

/// Most bytes read for one table (string tables of very large libraries are skipped rather than read whole).
const MAX_READ: usize = 32 << 20;
/// Most export names looked at.
const MAX_NAMES: usize = 1 << 16;

fn read_at<R: Read + Seek>(f: &mut R, at: u64, len: usize) -> Option<Vec<u8>> {
    if len > MAX_READ {
        return None;
    }
    f.seek(SeekFrom::Start(at)).ok()?;
    let mut b = vec![0; len];
    f.read_exact(&mut b).ok()?;
    Some(b)
}

/// Up to `len` bytes at `at` (fewer at the end of the file).
fn read_upto<R: Read + Seek>(f: &mut R, at: u64, len: usize) -> Option<Vec<u8>> {
    f.seek(SeekFrom::Start(at)).ok()?;
    let mut b = Vec::with_capacity(len);
    f.take(len as u64).read_to_end(&mut b).ok()?;
    Some(b)
}

fn u16_at(b: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(p..p + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(p..p + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], p: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(p..p + 8)?.try_into().ok()?))
}

/// A NUL-terminated name at `p` of `b`.
fn name_at(b: &[u8], p: usize) -> Option<String> {
    let rest = b.get(p..)?;
    let end = rest.iter().position(|&c| c == 0)?;
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// The offset of a PE file's `PE\0\0` signature.
fn pe_header<R: Read + Seek>(f: &mut R) -> Option<u64> {
    let dos = read_at(f, 0, 64)?;
    if !dos.starts_with(b"MZ") {
        return None;
    }
    let pe = u64::from(u32_at(&dos, 0x3C)?);
    (read_at(f, pe, 4)? == b"PE\0\0").then_some(pe)
}

/// A PE file's exported names (its export directory's name table).
fn pe_exports<R: Read + Seek>(f: &mut R) -> Option<Vec<String>> {
    let pe = pe_header(f)?;
    let coff = read_at(f, pe + 4, 20)?;
    let sections = usize::from(u16_at(&coff, 2)?);
    let opt_size = usize::from(u16_at(&coff, 16)?);
    let opt_at = pe + 24;
    let opt = read_at(f, opt_at, opt_size)?;
    // The data directories follow the optional header's fixed part, which is longer in 64-bit files.
    let (count_at, dirs_at) = match u16_at(&opt, 0)? {
        0x10B => (92, 96),
        0x20B => (108, 112),
        _ => return None,
    };
    if u32_at(&opt, count_at)? == 0 {
        return None;
    }
    let export_rva = u32_at(&opt, dirs_at)?;
    if export_rva == 0 {
        return Some(Vec::new());
    }
    let table = read_at(f, opt_at + opt_size as u64, sections.checked_mul(40)?)?;
    let to_offset = |rva: u32| -> Option<u64> {
        table.chunks_exact(40).find_map(|s| {
            let (vsize, va, raw_size, raw) = (u32_at(s, 8)?, u32_at(s, 12)?, u32_at(s, 16)?, u32_at(s, 20)?);
            (rva >= va && rva - va < vsize.max(raw_size)).then(|| u64::from(raw) + u64::from(rva - va))
        })
    };
    let dir = read_at(f, to_offset(export_rva)?, 40)?;
    let count = (u32_at(&dir, 24)? as usize).min(MAX_NAMES);
    let names = read_at(f, to_offset(u32_at(&dir, 32)?)?, count * 4)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let Some(at) = u32_at(&names, i * 4).and_then(to_offset) else { continue };
        out.extend(read_upto(f, at, 64).and_then(|b| name_at(&b, 0)));
    }
    Some(out)
}

/// A 64-bit little-endian ELF file's defined dynamic symbols.
fn elf_exports<R: Read + Seek>(f: &mut R) -> Option<Vec<String>> {
    let h = read_at(f, 0, 64)?;
    // 64-bit, little-endian (a 32-bit library cannot load in this program anyway).
    if h[4] != 2 || h[5] != 1 {
        return None;
    }
    let sh_off = u64_at(&h, 0x28)?;
    let sh_size = usize::from(u16_at(&h, 0x3A)?);
    let sh_num = usize::from(u16_at(&h, 0x3C)?);
    if sh_size < 64 {
        return None;
    }
    let table = read_at(f, sh_off, sh_size.checked_mul(sh_num)?)?;
    let section = |i: usize| table.get(i * sh_size..(i + 1) * sh_size);
    const SHT_DYNSYM: u32 = 11;
    let syms = (0..sh_num).filter_map(section).find(|s| u32_at(s, 4) == Some(SHT_DYNSYM))?;
    let strings = section(u32_at(syms, 0x28)? as usize)?;
    let sym_bytes = read_at(f, u64_at(syms, 0x18)?, usize::try_from(u64_at(syms, 0x20)?).ok()?)?;
    let str_bytes = read_at(f, u64_at(strings, 0x18)?, usize::try_from(u64_at(strings, 0x20)?).ok()?)?;
    let entry = usize::try_from(u64_at(syms, 0x38)?).ok().filter(|&e| e >= 24)?;
    let out = sym_bytes
        .chunks_exact(entry)
        .take(MAX_NAMES)
        // Defined symbols only (section index 0 = undefined: an import).
        .filter(|s| u16_at(s, 6).is_some_and(|ndx| ndx != 0))
        .filter_map(|s| name_at(&str_bytes, u32_at(s, 0)? as usize))
        .filter(|n| !n.is_empty())
        .collect();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A minimal 64-bit PE library exporting `names` (one section holding the export directory and its names).
    fn pe_with_exports(names: &[&str], machine: u16) -> Vec<u8> {
        let mut b = vec![0u8; 0x400];
        b[0..2].copy_from_slice(b"MZ");
        b[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        b[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        b[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        let opt_size: u16 = 240;
        b[0x94..0x96].copy_from_slice(&opt_size.to_le_bytes());
        let opt = 0x98;
        b[opt..opt + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
        b[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
        // The section: virtual address 0x1000, file offset 0x200.
        let (va, raw) = (0x1000u32, 0x200u32);
        b[opt + 112..opt + 116].copy_from_slice(&va.to_le_bytes());
        let sec = opt + opt_size as usize;
        b[sec..sec + 5].copy_from_slice(b".edat");
        b[sec + 8..sec + 12].copy_from_slice(&0x200u32.to_le_bytes());
        b[sec + 12..sec + 16].copy_from_slice(&va.to_le_bytes());
        b[sec + 16..sec + 20].copy_from_slice(&0x200u32.to_le_bytes());
        b[sec + 20..sec + 24].copy_from_slice(&raw.to_le_bytes());
        // Export directory at the section's start, the name pointers after it, then the names.
        let dir = raw as usize;
        b[dir + 24..dir + 28].copy_from_slice(&(names.len() as u32).to_le_bytes());
        let ptrs = 40;
        b[dir + 32..dir + 36].copy_from_slice(&(va + ptrs).to_le_bytes());
        let mut at = ptrs as usize + names.len() * 4;
        for (i, n) in names.iter().enumerate() {
            let p = dir + ptrs as usize + i * 4;
            b[p..p + 4].copy_from_slice(&(va + at as u32).to_le_bytes());
            b[dir + at..dir + at + n.len()].copy_from_slice(n.as_bytes());
            at += n.len() + 1;
        }
        b
    }

    /// A minimal 64-bit ELF library: `defined` exported, `imported` undefined.
    fn elf_with_symbols(defined: &[&str], imported: &[&str]) -> Vec<u8> {
        let mut strtab = vec![0u8];
        let mut syms = vec![0u8; 24];
        for (n, ndx) in defined.iter().map(|n| (n, 9u16)).chain(imported.iter().map(|n| (n, 0u16))) {
            let mut s = [0u8; 24];
            s[0..4].copy_from_slice(&(strtab.len() as u32).to_le_bytes());
            s[4] = 0x12;
            s[6..8].copy_from_slice(&ndx.to_le_bytes());
            syms.extend(s);
            strtab.extend(n.as_bytes());
            strtab.push(0);
        }
        let mut b = vec![0u8; 64];
        b[0..4].copy_from_slice(b"\x7FELF");
        b[4] = 2;
        b[5] = 1;
        let sym_off = b.len();
        b.extend(&syms);
        let str_off = b.len();
        b.extend(&strtab);
        let sh_off = b.len();
        // Sections: null, .dynsym (linked to 2), .dynstr.
        let mut sh = vec![0u8; 64 * 3];
        let set = |sh: &mut Vec<u8>, i: usize, ty: u32, off: usize, size: usize, link: u32, entsize: u64| {
            let s = &mut sh[i * 64..(i + 1) * 64];
            s[4..8].copy_from_slice(&ty.to_le_bytes());
            s[0x18..0x20].copy_from_slice(&(off as u64).to_le_bytes());
            s[0x20..0x28].copy_from_slice(&(size as u64).to_le_bytes());
            s[0x28..0x2C].copy_from_slice(&link.to_le_bytes());
            s[0x38..0x40].copy_from_slice(&entsize.to_le_bytes());
        };
        set(&mut sh, 1, 11, sym_off, syms.len(), 2, 24);
        set(&mut sh, 2, 3, str_off, strtab.len(), 0, 0);
        b.extend(sh);
        b[0x28..0x30].copy_from_slice(&(sh_off as u64).to_le_bytes());
        b[0x3A..0x3C].copy_from_slice(&64u16.to_le_bytes());
        b[0x3C..0x3E].copy_from_slice(&3u16.to_le_bytes());
        b
    }

    #[test]
    fn reads_pe_export_tables() {
        let b = pe_with_exports(&["GetPluginFactory", "VSTPluginMain", "main"], 0x8664);
        assert_eq!(exports(&mut Cursor::new(b)).unwrap(), ["GetPluginFactory", "VSTPluginMain", "main"]);
        let b = pe_with_exports(&["CreatePlugInstance"], 0x8664);
        assert_eq!(exports(&mut Cursor::new(b)).unwrap(), ["CreatePlugInstance"]);
        assert_eq!(exports(&mut Cursor::new(b"MZ but not a library".to_vec())), None);
    }

    #[test]
    fn reads_elf_dynamic_symbols() {
        let b = elf_with_symbols(&["VSTPluginMain", "helper"], &["malloc"]);
        assert_eq!(exports(&mut Cursor::new(b)).unwrap(), ["VSTPluginMain", "helper"]);
        assert_eq!(exports(&mut Cursor::new(b"\x7FELF".to_vec())), None);
    }

    /// A thin Mach-O file of `cpu` (64-bit, little-endian), or a universal one with a slice per CPU.
    fn macho(cpus: &[u32]) -> Vec<u8> {
        let mut b = Vec::new();
        if let [cpu] = cpus {
            b.extend(0xFEED_FACFu32.to_le_bytes());
            b.extend(cpu.to_le_bytes());
        } else {
            b.extend(0xCAFE_BABEu32.to_be_bytes());
            b.extend((cpus.len() as u32).to_be_bytes());
            for cpu in cpus {
                b.extend(cpu.to_be_bytes());
                b.extend([0; 16]);
            }
        }
        b.extend([0; 32]);
        b
    }

    #[test]
    fn reads_the_cpus_of_mach_o_files() {
        let d = std::env::temp_dir().join(format!("os-vst2-macho-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let write = |name: &str, b: Vec<u8>| {
            let p = d.join(name);
            std::fs::write(&p, b).unwrap();
            p
        };
        assert_eq!(macho_cpus(&write("intel", macho(&[CPU_X86_64]))), Some(vec![CPU_X86_64]));
        assert_eq!(macho_cpus(&write("universal", macho(&[CPU_X86_64, CPU_ARM64]))), Some(vec![CPU_X86_64, CPU_ARM64]));
        assert_eq!(macho_cpus(&write("text", b"not a binary".to_vec())), None);
        // An Intel-only bundle cannot load in Lemon Studio on Apple silicon; a universal one can.
        let bundle = |name: &str, cpus: &[u32]| {
            let b = d.join(format!("{name}.vst"));
            std::fs::create_dir_all(b.join("Contents/MacOS")).unwrap();
            std::fs::write(b.join("Contents/MacOS").join(name), macho(cpus)).unwrap();
            b
        };
        let (old, both) = (bundle("Old", &[CPU_X86_64]), bundle("Both", &[CPU_X86_64, CPU_ARM64]));
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert!(unsupported(&old).is_some_and(|w| w.contains("Intel Macs only")));
        } else {
            assert_eq!(unsupported(&old), None);
        }
        assert_eq!(unsupported(&both), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn plugin_files_are_told_apart_by_their_exports() {
        let d = std::env::temp_dir().join(format!("os-vst2-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let file = |name: &str| d.join(format!("{name}.{EXTENSION}"));
        if cfg!(target_os = "macos") {
            std::fs::create_dir_all(file("Synth").join("Contents/MacOS")).unwrap();
            std::fs::write(file("Loose"), b"").unwrap();
            assert!(is_plugin(&file("Synth")));
            assert!(!is_plugin(&file("Loose")), "a plugin is a bundle folder on macOS");
        } else {
            let (plugin, library) = if cfg!(windows) {
                (pe_with_exports(&["VSTPluginMain"], 0x8664), pe_with_exports(&["DllMain2"], 0x8664))
            } else {
                (elf_with_symbols(&["main"], &[]), elf_with_symbols(&["helper"], &["VSTPluginMain"]))
            };
            std::fs::write(file("Synth"), plugin).unwrap();
            std::fs::write(file("Support"), library).unwrap();
            assert!(is_plugin(&file("Synth")));
            assert!(!is_plugin(&file("Support")), "a library without an entry point is no plugin");
            assert_eq!(module_file(&file("Synth")), file("Synth"));
        }
        std::fs::write(d.join("notes.txt"), b"").unwrap();
        assert!(!is_plugin(&d.join("notes.txt")));
        let x86 = d.join("Old.dll");
        std::fs::write(&x86, pe_with_exports(&["main"], 0x014C)).unwrap();
        assert_eq!(pe_machine(&x86), Some("x86"));
        assert_eq!(pe_machine(&d.join("notes.txt")), None);
        // A 32-bit plugin cannot load in 64-bit Lemon Studio on Windows; the same file means nothing elsewhere.
        let why = unsupported(&x86);
        if cfg!(all(windows, target_pointer_width = "64")) {
            assert!(why.is_some_and(|w| w.contains("32-bit")));
        } else if !cfg!(target_os = "macos") {
            assert_eq!(why, None);
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
