// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A release channel's manifest: `nightly.json` or `stable.json`, written and signed by the release workflow
//! (`.github/workflows/release.yml`), signed with minisign. It names the channel's newest build and, per platform, the
//! file to download with its size and SHA-256.
//!
//! ```json
//! { "channel": "nightly", "version": "2026.11.7.1", "commit": "0be813c5…", "date": "2026-11-07T03:12:44Z",
//!   "page": "https://github.com/ItsAlphaNeon/Lemon-Studio/releases/tag/nightly-2026.11.7.1",
//!   "notes": ["Linux: typed notes end when their key is up", "…"],
//!   "assets": { "windows-x86_64": { "url": "https://…", "size": 52183040, "sha256": "9f2c…" }, … } }
//! ```
//!
//! Versions: a nightly is dated, `YEAR.MONTH.DAY.N` (UTC, no zero padding; `N` counts that day's nightlies from 1); a
//! tagged release (`stable`: an alpha while its major version is 0) is semantic, `MAJOR.MINOR.PATCH`. The two are never
//! compared with each other.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const NIGHTLY: &str = "nightly";
pub const STABLE: &str = "stable";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub channel: String,
    pub version: String,
    #[serde(default)]
    pub commit: String,
    #[serde(default)]
    pub date: String,
    /// The release's page on GitHub.
    #[serde(default)]
    pub page: String,
    /// What changed, one line each (pull request titles).
    #[serde(default)]
    pub notes: Vec<String>,
    /// By platform (`platform()`).
    pub assets: BTreeMap<String, Asset>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub url: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// This build's platform, as the manifest's `assets` name it: `windows-x86_64`, `linux-x86_64`, `macos-aarch64`, ...
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Where a channel's manifest is, under the releases URL `base` (`https://github.com/<owner>/<repo>/releases`). Stable:
/// the newest release that is not a pre-release (GitHub's `latest`). Nightly: the `nightly` pre-release, which only
/// holds the manifest and is moved on by every nightly.
pub fn url(base: &str, channel: &str) -> Result<String, String> {
    let base = base.trim_end_matches('/');
    match channel {
        STABLE => Ok(format!("{base}/latest/download/{STABLE}.json")),
        NIGHTLY => Ok(format!("{base}/download/{NIGHTLY}/{NIGHTLY}.json")),
        _ => Err(format!("unknown channel {channel:?}")),
    }
}

/// The manifest in `body`, once its minisign signature `sig` checks out against the public key `key` (base64, the last
/// line of a `minisign.pub`), and it is for `channel`.
pub fn verified(body: &[u8], sig: &str, key: &str, channel: &str) -> Result<Manifest, String> {
    let key = key.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("");
    let pk = minisign_verify::PublicKey::from_base64(key).map_err(|e| format!("update key: {e}"))?;
    let sig = minisign_verify::Signature::decode(sig).map_err(|e| format!("manifest signature: {e}"))?;
    pk.verify(body, &sig, false).map_err(|e| format!("manifest signature does not match: {e}"))?;
    let m: Manifest = serde_json::from_slice(body).map_err(|e| format!("manifest: {e}"))?;
    if m.channel != channel {
        return Err(format!("the manifest is for the {} channel, not {channel}", m.channel));
    }
    Ok(m)
}

/// A nightly's dated version, `YEAR.MONTH.DAY.N`.
fn dated(v: &str) -> Option<[u32; 4]> {
    let parts: Vec<u32> = v.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let [y, m, d, n] = parts[..] else { return None };
    ((1..=12).contains(&m) && (1..=31).contains(&d) && n >= 1).then_some([y, m, d, n])
}

/// `candidate` against `current` on `channel`.
pub fn compare(channel: &str, candidate: &str, current: &str) -> Result<Ordering, String> {
    match channel {
        NIGHTLY => {
            let bad = |v: &str| format!("{v:?} is not a nightly version (YEAR.MONTH.DAY.N)");
            Ok(dated(candidate).ok_or_else(|| bad(candidate))?.cmp(&dated(current).ok_or_else(|| bad(current))?))
        }
        STABLE => {
            let parse = |v: &str| semver::Version::parse(v.trim_start_matches('v')).map_err(|e| format!("{v:?}: {e}"));
            Ok(parse(candidate)?.cmp(&parse(current)?))
        }
        _ => Err(format!("unknown channel {channel:?}")),
    }
}

/// Whether `candidate` is newer than `current` on `channel`: never a step back, so an old manifest served again is no
/// update.
pub fn newer(channel: &str, candidate: &str, current: &str) -> Result<bool, String> {
    compare(channel, candidate, current).map(|o| o == Ordering::Greater)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nightlies_compare_by_date_then_count() {
        assert!(newer(NIGHTLY, "2026.11.7.2", "2026.11.7.1").unwrap());
        assert!(newer(NIGHTLY, "2026.11.10.1", "2026.11.7.3").unwrap(), "numbers, not text");
        assert!(newer(NIGHTLY, "2027.1.1.1", "2026.12.31.9").unwrap());
        assert!(!newer(NIGHTLY, "2026.11.7.1", "2026.11.7.1").unwrap());
        assert!(!newer(NIGHTLY, "2026.11.6.4", "2026.11.7.1").unwrap(), "never back");
    }

    #[test]
    fn nightly_versions_have_four_parts() {
        for bad in ["2026.11.7", "2026.11.7.1.1", "2026.13.1.1", "2026.11.7.0", "0.0.1", "v2026.11.7.1", ""] {
            assert!(compare(NIGHTLY, bad, "2026.11.7.1").is_err(), "{bad}");
        }
    }

    #[test]
    fn stable_versions_are_semantic() {
        assert!(newer(STABLE, "1.0.0", "0.9.12").unwrap());
        assert!(newer(STABLE, "1.10.0", "1.9.0").unwrap());
        assert!(newer(STABLE, "v1.0.1", "1.0.0").unwrap(), "a tag's v is allowed");
        assert!(newer(STABLE, "1.0.0", "1.0.0-rc.1").unwrap());
        assert!(newer(STABLE, "0.1.1", "0.1.0").unwrap(), "from alpha to alpha");
        assert!(newer(STABLE, "1.0.0", "0.12.3").unwrap(), "from an alpha on to the first stable release");
        assert!(!newer(STABLE, "0.1.0", "0.1.1").unwrap(), "never back");
        assert!(!newer(STABLE, "1.0.0", "1.0.0").unwrap());
        assert!(compare(STABLE, "2026.11.7.1", "1.0.0").is_err());
    }

    #[test]
    fn manifest_urls() {
        let base = "https://github.com/ItsAlphaNeon/Lemon-Studio/releases/";
        assert_eq!(url(base, STABLE).unwrap(), "https://github.com/ItsAlphaNeon/Lemon-Studio/releases/latest/download/stable.json");
        assert_eq!(url(base, NIGHTLY).unwrap(), "https://github.com/ItsAlphaNeon/Lemon-Studio/releases/download/nightly/nightly.json");
        assert!(url(base, "beta").is_err());
    }

    /// A manifest signed with a throwaway minisign key (made for this test and thrown away; not the release key).
    const KEY: &str = "untrusted comment: minisign public key: F6AB7B4AA4ADAE5B\nRWRbrq2kSnur9jI/ZYtWjdCOEicegzn5/f461vgXZ92Kaeg6J5bhht+y\n";
    const BODY: &[u8] = br#"{"channel":"nightly","version":"2026.11.7.1","assets":{"windows-x86_64":{"url":"https://example.invalid/a.exe","size":3,"sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"}}}"#;
    const SIG: &str = "untrusted comment: signature from rsign secret key
RURbrq2kSnur9oGwtsObmQdSbebup0WmxfV3i9lx6JG9wQtaEK2OqMw6uEw5yL2nvXpL7kVwuPou0MWIWbRVJwCW4ncRARf2XwQ=
trusted comment: lemonstudio nightly 2026.11.7.1
yBFH76fWe28O6lf7nNetV+sKQgVqbVa/oFOHYguWLovG9++HyRUbDadW7Gcd7mRSNn2pcHhqfKGOM88m+nLeCQ==
";

    #[test]
    fn a_signed_manifest_is_read() {
        let m = verified(BODY, SIG, KEY, NIGHTLY).unwrap();
        assert_eq!(m.version, "2026.11.7.1");
        assert_eq!(m.assets["windows-x86_64"].size, 3);
        // The key alone (the line `MINISIGN_PUBLIC_KEY` holds) works as well as the whole .pub file.
        assert!(verified(BODY, SIG, KEY.lines().nth(1).unwrap(), NIGHTLY).is_ok());
    }

    #[test]
    fn a_changed_or_unsigned_manifest_is_refused() {
        let changed = String::from_utf8(BODY.to_vec()).unwrap().replace("2026.11.7.1", "2026.11.7.2");
        assert!(verified(changed.as_bytes(), SIG, KEY, NIGHTLY).is_err());
        assert!(verified(BODY, "not a signature", KEY, NIGHTLY).is_err());
        let other_key = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
        assert!(verified(BODY, SIG, other_key, NIGHTLY).is_err());
    }

    #[test]
    fn a_manifest_for_another_channel_is_refused() {
        assert!(verified(BODY, SIG, KEY, STABLE).unwrap_err().contains("nightly channel"));
    }
}
