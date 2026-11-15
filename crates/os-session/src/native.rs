//! The switches of the native-plugin host (`os-native`: FL Studio's native plugins, spec plugins/fruity-abi.md),
//! for the whole program. Without the `fl-native` feature the host is not in this build: it is never on, and
//! turning it on does nothing.

#[cfg(feature = "fl-native")]
pub use os_native::{enabled, prefer_native, prefer_native_set, set_enabled, set_prefer_native, settings_shown};

/// The native-plugin host is not in this build: never on.
#[cfg(not(feature = "fl-native"))]
pub fn enabled() -> bool {
    false
}

/// The native-plugin host is not in this build: its settings are never shown.
#[cfg(not(feature = "fl-native"))]
pub fn settings_shown() -> bool {
    false
}

/// The native-plugin host is not in this build: it stays off.
#[cfg(not(feature = "fl-native"))]
pub fn set_enabled(_on: bool) {}

/// The native-plugin host is not in this build: opened projects' native plugins are never played by it.
#[cfg(not(feature = "fl-native"))]
pub fn prefer_native() -> bool {
    false
}

/// The native-plugin host is not in this build: no choice is stored.
#[cfg(not(feature = "fl-native"))]
pub fn prefer_native_set() -> bool {
    false
}

/// The native-plugin host is not in this build: nothing to prefer.
#[cfg(not(feature = "fl-native"))]
pub fn set_prefer_native(_on: bool) {}
