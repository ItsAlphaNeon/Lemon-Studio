# Releasing Lemon Studio

Lemon Studio is released by [.github/workflows/release.yml](../.github/workflows/release.yml) on two channels. Each
build updates itself on its own channel. A build from source has no channel and never updates itself.

| Channel | Made by | Version | GitHub release |
|---|---|---|---|
| Nightly | Every night at 04:00 UTC from `main`, if `main` moved since the last nightly; or by hand (Actions ▸ Release ▸ Run workflow, with *force* to build one anyway) | Dated: `YEAR.MONTH.DAY.N` (UTC, no zero padding, `N` counts that day's nightlies from 1), e.g. `2026.11.7.1` | Pre-release `nightly-2026.11.7.1`; the pre-release `nightly` holds `nightly.json`, pointing at the newest. The newest 14 are kept |
| Stable (after the alpha) | Pushing a tag `vMAJOR.MINOR.PATCH` | Semantic: the workspace's `version` in `Cargo.toml`, which the tag must match | Release `v1.0.0`, holding `stable.json`; GitHub's *latest* release |

Each release has:

- `LemonStudio-<version>-windows-x86_64.exe`
- `LemonStudio-<version>-linux-x86_64.AppImage`, built on Ubuntu 22.04 by [tools/package/appimage.sh](../tools/package/appimage.sh)
- `LemonStudio-<version>-macos-aarch64.zip`, holding `Lemon Studio.app`, signed ad hoc and not notarized
- `LemonStudio-Plugins-<version>-<platform>.zip`: the VST3 and CLAP plugins for other DAWs, with a `README.txt` on where they go
- `nightly.json` or `stable.json`, and its minisign signature (`.minisig`)
- `LICENSE` and `THIRD-PARTY-NOTICES.txt`

The release notes come from [tools/package/release-notes.md](../tools/package/release-notes.md) (how to install and
open the builds, including the macOS steps past Gatekeeper), followed by GitHub's list of the pull requests since the
previous release on that channel.

## How a build updates itself

The workflow builds `lemonstudio-updater` ([tools/updater](../tools/updater)) first. It then builds Lemon Studio with
these environment variables set:

- `LEMON_CHANNEL` and `LEMON_VERSION`: [crates/os-app/src/version.rs](../crates/os-app/src/version.rs)
- `LEMON_UPDATER`: the updater, which the program carries inside itself (`build.rs`)
- `LEMON_UPDATE_KEY`: the public key the updater checks manifests with

At start, Lemon Studio writes the updater to `<user folder>/Updates` and runs it there. The updater reads the channel's
manifest:

- nightly: `releases/download/nightly/nightly.json`
- stable: `releases/latest/download/stable.json`

It checks the manifest's signature and compares versions: dated with dated, semantic with semantic, and never accepts an
older version. If there is a newer version, it downloads this platform's file and checks its size and SHA-256. Lemon
Studio then offers the update. If the user installs it, Lemon Studio closes and the updater replaces the `.exe`, the
AppImage (`$APPIMAGE`) or the `.app`, then starts the new version. The next start reports how it went. The
module docs in [update.rs](../crates/os-app/src/update.rs) and the updater's `main.rs` have the details.

A copy that can't replace itself says so and doesn't offer the update:

- a Linux build not run from its AppImage
- a macOS app run from where it was downloaded (macOS runs it from a read-only copy)
- a program in a folder the user can't write to

To try an update against a server of your own, set `OPENSTUDIO_UPDATE_BASE` to its releases URL. The manifest must
still be signed with the release key. `OPENSTUDIO_NO_UPDATE_CHECK=1` turns off the check at start.

## The signing key (once)

Manifests are signed with [minisign](https://jedisct1.github.io/minisign/). Make a key pair without a password; the
GitHub secret store protects it:

```bash
minisign -G -W -p lemonstudio-update.pub -s lemonstudio-update.key
gh secret set MINISIGN_SECRET_KEY < lemonstudio-update.key
gh variable set MINISIGN_PUBLIC_KEY --body "$(tail -n1 lemonstudio-update.pub)"
```

Keep `lemonstudio-update.key` somewhere safe and offline, and don't commit it. Every installed copy trusts only this
key. If the key is lost or replaced, the copies already out there can't verify new manifests, and their users have to
download a new build by hand. The workflow refuses to run without both settings, and it checks each signature against
the public key before publishing.

## Making a stable release

1. Set `version` under `[workspace.package]` in `Cargo.toml`, e.g. `1.0.0`, and merge it.
2. Tag the merge commit and push the tag: `git tag v1.0.0 && git push origin v1.0.0`.
