//! `cargo run -p bundle-plugins [-- --install]`: builds every OpenStudio plugin in release mode and writes
//! `<target>/bundled/<Name>.vst3` (bundle folder) and `<Name>.clap`; `--install` copies them to the per-user plugin
//! folders (no admin rights needed).
//!
// spec: plugins/plugin-formats.md (bundles and install folders)

use std::path::{Path, PathBuf};
use std::process::Command;

/// (cargo package, library file stem, display name).
const PLUGINS: &[(&str, &str, &str)] =
    &[("open-sampler", "open_sampler", "Open Sampler"), ("open-parametric-eq-2", "open_parametric_eq_2", "Open Parametric EQ 2")];

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
        let dll = target.join("release").join(format!("{stem}.dll"));
        let vst3 = out.join(format!("{name}.vst3"));
        let arch = vst3.join("Contents").join("x86_64-win");
        std::fs::create_dir_all(&arch).expect("vst3 bundle");
        std::fs::copy(&dll, arch.join(format!("{name}.vst3"))).expect("copy vst3");
        let clap = out.join(format!("{name}.clap"));
        std::fs::copy(&dll, &clap).expect("copy clap");
        println!("{}\n{}", vst3.display(), clap.display());
        if install {
            let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Programs").join("Common");
            copy_dir(&vst3, &local.join("VST3").join(format!("{name}.vst3"))).expect("install vst3");
            std::fs::create_dir_all(local.join("CLAP")).expect("clap folder");
            std::fs::copy(&clap, local.join("CLAP").join(format!("{name}.clap"))).expect("install clap");
            println!("installed to {}", local.display());
        }
    }
}
