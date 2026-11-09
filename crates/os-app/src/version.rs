// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Which Lemon Studio this is. The release workflow (`.github/workflows/release.yml`) builds with `LEMON_CHANNEL`
//! (`nightly` or `stable`) and `LEMON_VERSION` set: a nightly's version is dated, `YEAR.MONTH.DAY.N`; a tagged
//! release's is semantic, `MAJOR.MINOR.PATCH` (the workspace's `version`), and while its major version is 0 it is an
//! alpha. A build from source has neither: it is the workspace's version and belongs to no channel, so it never updates
//! itself (`crate::update`).

/// This build's version.
pub const VERSION: &str = match option_env!("LEMON_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// The release channel this build came from: `nightly` or `stable`; None for a build from source.
pub const CHANNEL: Option<&str> = option_env!("LEMON_CHANNEL");

/// Whether this is a nightly build, which has its own name, icon and splash.
pub const NIGHTLY: bool = cfg!(lemon_nightly);

/// The program's name, where the system shows it (the window's title, the file association).
pub const NAME: &str = if NIGHTLY { "Lemon Studio Nightly" } else { "Lemon Studio" };

/// The version as Lemon Studio shows it: "2026.11.7.1 nightly", "1.0.0", or "0.0.1" for a build from source.
pub fn text() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| match CHANNEL {
        Some("nightly") => format!("{VERSION} nightly"),
        _ => VERSION.to_string(),
    })
}

/// The releases a build on `channel` follows, in words: `nightly`, `alpha` (a tagged release's version 0.x) or `stable`.
fn releases_of(channel: Option<&str>, version: &str) -> &'static str {
    match channel {
        Some("nightly") => "nightly",
        _ if version.starts_with("0.") => "alpha",
        _ => "stable",
    }
}

/// The releases this build follows, in words.
pub fn releases() -> &'static str {
    releases_of(CHANNEL, VERSION)
}

/// How far along a build says it is: a tagged release says what it is, an alpha or nothing at all. What is not tagged
/// (a nightly, a build from source) comes before the first alpha, until that is out and this changes.
fn stage_of(channel: Option<&str>, version: &str) -> Option<&'static str> {
    match channel {
        Some("stable") => (releases_of(channel, version) == "alpha").then_some("alpha"),
        _ => Some("pre-alpha"),
    }
}

/// `text` with the stage: "2026.11.7.1 nightly (pre-alpha)", "0.1.0 (alpha)", "1.0.0".
pub fn staged() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| match stage_of(CHANNEL, VERSION) {
        Some(stage) => format!("{} ({stage})", text()),
        None => text().to_string(),
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

    /// Alpha is in the version's number, not in the tag: v0.1.0 is an alpha, v1.0.0 is not.
    #[test]
    fn a_tagged_release_says_if_it_is_an_alpha() {
        assert_eq!(stage_of(Some("stable"), "0.1.0"), Some("alpha"));
        assert_eq!(stage_of(Some("stable"), "0.12.3"), Some("alpha"));
        assert_eq!(stage_of(Some("stable"), "1.0.0"), None);
        assert_eq!(stage_of(Some("stable"), "10.0.0"), None);
        assert_eq!(stage_of(Some("nightly"), "2026.11.8.1"), Some("pre-alpha"));
        assert_eq!(stage_of(None, "0.0.1"), Some("pre-alpha"));
        assert_eq!(releases_of(Some("stable"), "0.1.0"), "alpha");
        assert_eq!(releases_of(Some("stable"), "1.0.0"), "stable");
        assert_eq!(releases_of(Some("nightly"), "2026.11.8.1"), "nightly");
    }
}
