# Releasing Lemon Studio

Lemon Studio is released by [.github/workflows/release.yml](../.github/workflows/release.yml) on two channels. Each
build updates itself on its own channel. A build from source has no channel and never updates itself.

| Channel | Made by | Version | GitHub release |
|---|---|---|---|
| Nightly | Every night at 04:00 UTC from `main`, once turned on, if `main` moved since the last nightly; or by hand (Actions ▸ Release ▸ Run workflow, with *force* to build one anyway) | Dated: `YEAR.MONTH.DAY.N` (UTC, no zero padding, `N` counts that day's nightlies from 1), e.g. `2026.11.7.1` | Pre-release `nightly-2026.11.7.1`; the pre-release `nightly` holds `nightly.json`, pointing at the newest. The newest 14 are kept |
| Tagged | Pushing a tag `vMAJOR.MINOR.PATCH` on `main` | Semantic: the workspace's `version` in `Cargo.toml`, which the tag must match, e.g. `0.1.0` | Release `v0.1.0`, holding `stable.json`; GitHub's *latest* release |

**A tagged release with major version 0 is an alpha.** The tag has no `alpha` in it: `v0.1.0`, `v0.1.1` and `v0.2.0` are
alphas, and `v1.0.0` is the first stable release. Lemon Studio, the diagnostics and the release's title say which it is
([version.rs](../crates/os-app/src/version.rs)), and installed alphas update on to the first stable release like any
other newer version. Alphas are not marked as pre-releases on GitHub, because the updater finds the newest one as
GitHub's *latest* release. In the workflow and the manifest the channel is called `stable`, alphas included.

Each release has:

- `LemonStudio.exe` (a nightly: `LemonStudioNightly.exe`)
- `LemonStudio.AppImage` (a nightly: `LemonStudioNightly.AppImage`), built on Ubuntu 22.04 by [tools/package/appimage.sh](../tools/package/appimage.sh)
- `LemonStudio-<version>-macos-aarch64.zip`, holding `Lemon Studio.app` (a nightly: `Lemon Studio Nightly.app`), signed
  ad hoc and not notarized
- `LemonStudio-Plugins-<version>-<platform>.zip`: the VST3 and CLAP plugins for other DAWs, with a `README.txt` on where they go
- `nightly.json` or `stable.json`, and its minisign signature (`.minisig`)
- `LICENSE` and `THIRD-PARTY-NOTICES.txt`

The `.exe` and the AppImage have no version in their names: an update replaces the file in place, keeping its name, so
shortcuts to it never break. The release's title and Lemon Studio itself say which version it is.

The release notes come from [tools/package/release-notes.md](../tools/package/release-notes.md) (how to install and
open the builds, including the macOS steps past Gatekeeper), followed by GitHub's list of the pull requests since the
previous release on that channel. The first release on a channel has no such list.

## What makes a nightly look like one

With `LEMON_CHANNEL=nightly`, a build has the Nightly logo and its own name, Lemon Studio Nightly, so that it can sit
beside a tagged release on one computer: the two share the user folder (settings, projects), but keep their updates
apart.

| Where | What changes | Made from |
|---|---|---|
| Windows | The `.exe`'s file name, icon and version resource, the window's title | `assets/lemonstudio-nightly.ico` |
| Linux | The AppImage's file name, desktop entry and icon, the window's app id and icon | `assets/lemonstudio-nightly.png` |
| macOS | The app's name, identifier and icon | `assets/lemonstudio-nightly.car` and `.icns` |
| Startup splash | The moon, its craters, the night sky and the stars | `crate::logo`'s `NIGHTLY_*`, [splash.rs](../crates/os-app/src/splash.rs) |

All of these are in `crates/os-app/assets` and are committed, so a build needs no tools to make them. To change them,
edit the sources in `assets/logo`, then regenerate:

- `icon-lemon-nightly.png` (edited in `icon-lemon-nightly.pdn`, Paint.NET) and `logo-colored-lemon-nightly.svg`:
  `python tools/logo/gen.py`, which needs Pillow.
- `macOS-nightly.icon`, which has its own copy of the SVG: `tools/logo/macos-icon.sh nightly`, on macOS with Xcode 26.

## How a build updates itself

The workflow builds `lemonstudio-updater` ([tools/updater](../tools/updater)) first. It then builds Lemon Studio with
these environment variables set:

- `LEMON_CHANNEL` and `LEMON_VERSION`: [crates/os-app/src/version.rs](../crates/os-app/src/version.rs)
- `LEMON_UPDATER`: the updater, which the program carries inside itself (`build.rs`)
- `LEMON_UPDATE_KEY`: the public key the updater checks manifests with

At start, Lemon Studio writes the updater to `<user folder>/Updates/<channel>` and runs it there. The updater reads the
channel's manifest:

- nightly: `releases/download/nightly/nightly.json`
- tagged: `releases/latest/download/stable.json`

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

## Going live (once)

Merging the workflow publishes nothing: its schedule stays off, and a run by hand refuses to start without the signing
key. In this order:

### 1. The signing key

Manifests are signed with [minisign](https://jedisct1.github.io/minisign/) (Windows: `winget install jedisct1.minisign`).
Make a key pair without a password; the GitHub secret store protects it:

```bash
minisign -G -W -p lemonstudio-update.pub -s lemonstudio-update.key
gh secret set MINISIGN_SECRET_KEY < lemonstudio-update.key
gh variable set MINISIGN_PUBLIC_KEY --body "$(tail -n1 lemonstudio-update.pub)"
```

Keep `lemonstudio-update.key` somewhere safe and offline, and don't commit it. Every installed copy trusts only this
key. If the key is lost or replaced, the copies already out there can't verify new manifests, and their users have to
download a new build by hand. The workflow refuses to run without both settings, and it checks each signature against
the public key before publishing.

### 2. Rehearse

A dry run builds, packages and signs everything, then keeps the files as the run's artifacts instead of publishing them.
Nothing is created on the releases page. Rehearse a nightly and a tag (the tag must match `Cargo.toml`, and need not
exist):

```bash
gh workflow run release.yml -f dry_run=true
```

```bash
gh workflow run release.yml -f dry_run=true -f rehearse_tag=v0.0.1
```

Then download the artifacts and open each platform's build. A rehearsed build can't update itself, as there is no
release for it to find yet.

### 3. Publish the first nightly and turn the schedule on

```bash
gh workflow run release.yml
```

```bash
gh variable set NIGHTLIES --body on
```

The website's download buttons come alive at its next rebuild, 05:30 UTC each night.

### 4. Try an update on each platform

An update needs two builds. With the first nightly installed on each system, publish a second by hand with *force*, and
check that each one offers it, installs it and starts again.

## Making a tagged release

1. Set `version` under `[workspace.package]` in `Cargo.toml`, e.g. `0.1.0`, run `cargo check` so that `Cargo.lock`
   follows, and merge it.
2. Tag the merge commit and push the tag:

   ```bash
   git switch main && git pull && git tag v0.1.0 && git push origin v0.1.0
   ```

The workflow refuses a tag that isn't `vMAJOR.MINOR.PATCH`, doesn't match `Cargo.toml` or isn't on `main`.

Once the first alpha is out, nightlies and builds from source stop being "pre-alpha": change `stage_of` in
[version.rs](../crates/os-app/src/version.rs).

## Testing the workflow's scripts

`tools/package/test.sh` tests the release plan and the manifest without building anything, and CI runs it. CI also
checks that a nightly's and a tagged release's settings compile on every platform.
