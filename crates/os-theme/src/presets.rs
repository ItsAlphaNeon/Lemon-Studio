//! Built-in theme presets, in `.flstheme` text form (parsed by `FlTheme::parse`).
// spec: ui/theme-presets.md
//
// OpenStudio ships no third-party theme data. Settings > Theme lists these, then the user's own
// `.flstheme` files from `%APPDATA%\OpenStudio\themes`.

/// (name, theme text).
pub const PRESETS: &[(&str, &str)] = &[];
