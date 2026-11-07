// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Which Lemon Studio this is. The release workflow (`.github/workflows/release.yml`) builds with `LEMON_CHANNEL`
//! (`nightly` or `stable`) and `LEMON_VERSION` set: a nightly's version is dated, `YEAR.MONTH.DAY.N`; a stable
//! release's is semantic, `MAJOR.MINOR.PATCH` (the workspace's `version`). A build from source has neither: it is the
//! workspace's version and belongs to no channel, so it never updates itself (`crate::update`).

/// This build's version.
pub const VERSION: &str = match option_env!("LEMON_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// The release channel this build came from: `nightly` or `stable`; None for a build from source.
pub const CHANNEL: Option<&str> = option_env!("LEMON_CHANNEL");

/// The version as Lemon Studio shows it: "2026.11.7.1 nightly", "1.0.0", or "0.0.1" for a build from source.
pub fn text() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| match CHANNEL {
        Some("nightly") => format!("{VERSION} nightly"),
        _ => VERSION.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_from_source_is_the_workspace_version() {
        if CHANNEL.is_none() {
            assert_eq!(text(), env!("CARGO_PKG_VERSION"));
        }
    }
}
