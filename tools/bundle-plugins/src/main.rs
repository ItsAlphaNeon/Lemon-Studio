//! `cargo run -p bundle-plugins [-- --install]`: builds every Lemon Studio plugin in release mode and writes
//! `<target>/bundled/<Name>.vst3` (bundle folder) and `<Name>.clap`; `--install` copies them to the per-user plugin
//! folders (no admin rights needed): `%LOCALAPPDATA%\Programs\Common\{VST3,CLAP}` on Windows, `~/.vst3` and
//! `~/.clap` on Linux.
//!
// spec: plugins/plugin-formats.md (bundles and install folders)

use std::path::{Path, PathBuf};
use std::process::Command;

/// (cargo package, library file stem, display name).
const PLUGINS: &[(&str, &str, &str)] =
    &[("open-sampler", "open_sampler", "Open Sampler"), ("open-parametric-eq-2", "open_parametric_eq_2", "Open Parametric EQ 2"), ("open-3x-osc", "open_3x_osc", "Open 3x Osc")];

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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
    let target = if target.is_relative() { root.join(target) } else { target };
    let out = target.join("bundled");
    std::fs::create_dir_all(&out).expect("bundled folder");
    for (pkg, stem, name) in PLUGINS {
        let ok = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["build", "--release", "--lib", "-p", pkg])
            .current_dir(&root)
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .expect("cargo")
            .success();
        assert!(ok, "build of {pkg} failed");
        let lib = target.join("release").join(library_file(stem));
        let vst3 = out.join(format!("{name}.vst3"));
        let arch = vst3.join("Contents").join(BUNDLE_ARCH);
        std::fs::create_dir_all(&arch).expect("vst3 bundle");
        std::fs::copy(&lib, arch.join(bundle_module(name))).expect("copy vst3");
        let clap = out.join(format!("{name}.clap"));
        std::fs::copy(&lib, &clap).expect("copy clap");
        println!("{}\n{}", vst3.display(), clap.display());
        if install {
            let (vst3_dir, clap_dir) = install_dirs();
            copy_dir(&vst3, &vst3_dir.join(format!("{name}.vst3"))).expect("install vst3");
            std::fs::create_dir_all(&clap_dir).expect("clap folder");
            std::fs::copy(&clap, clap_dir.join(format!("{name}.clap"))).expect("install clap");
            println!("installed to {} and {}", vst3_dir.display(), clap_dir.display());
        }
    }
}

/// The bundle folder holding the module: `Contents/<architecture>-<platform>` (VST 3 bundle layout).
const BUNDLE_ARCH: &str = if cfg!(windows) {
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

/// The module's name inside the bundle: `<Name>.vst3` on Windows, `<Name>.so` on Linux.
fn bundle_module(name: &str) -> String {
    if cfg!(windows) { format!("{name}.vst3") } else { format!("{name}{}", std::env::consts::DLL_SUFFIX) }
}

/// The per-user VST3 and CLAP folders.
fn install_dirs() -> (PathBuf, PathBuf) {
    if cfg!(windows) {
        let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Programs").join("Common");
        (local.join("VST3"), local.join("CLAP"))
    } else {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        (home.join(".vst3"), home.join(".clap"))
    }
}
