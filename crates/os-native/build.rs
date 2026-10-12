//! Compiles `src/guard.c` (structured exception handling around plugin calls) on Windows MSVC targets and sets
//! `cfg(native_guard)`. Elsewhere, or without a C compiler, plugin calls are made directly.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(native_guard)");
    println!("cargo:rerun-if-changed=src/guard.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    match cc::Build::new().file("src/guard.c").try_compile("os_native_guard") {
        Ok(()) => println!("cargo::rustc-cfg=native_guard"),
        Err(e) => println!("cargo:warning=no C compiler for src/guard.c ({e}); native plugin errors will not be contained"),
    }
}
