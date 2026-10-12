<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/logo-light.png">
  <img alt="Lemon Studio" src="docs/assets/logo-light.png" width="520">
</picture>

<h3>A free and open-source digital audio workstation, written in Rust.</h3>

<p>
  <a href="LICENSE"><img alt="License: GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue?style=flat-square"></a>
  <img alt="Rust 2024 edition" src="https://img.shields.io/badge/rust-2024_edition-e57324?style=flat-square&logo=rust&logoColor=white">
  <img alt="Platform: Windows | Linux" src="https://img.shields.io/badge/platform-Windows_%7C_Linux-0078d4?style=flat-square">
  <a href="https://github.com/ItsAlphaNeon/Lemon-Studio/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/ItsAlphaNeon/Lemon-Studio/ci.yml?branch=main&style=flat-square&label=CI"></a>
  <img alt="Status: pre-alpha" src="https://img.shields.io/badge/status-pre--alpha-ffe066?style=flat-square">
  <a href="https://discord.gg/yC2uCuGs3M"><img alt="Discord" src="https://img.shields.io/badge/discord-join_the_server-5865f2?style=flat-square&logo=discord&logoColor=white"></a>
</p>

<p>
  <a href="#overview">Overview</a> ·
  <a href="#capabilities">Capabilities</a> ·
  <a href="#getting-started">Getting started</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="#roadmap">Roadmap</a> ·
  <a href="#contributing">Contributing</a> ·
  <a href="#community">Community</a> ·
  <a href="#license">License</a>
</p>

</div>

---

## Overview

Lemon Studio is a pattern-based music production environment: build patterns in a step sequencer and
piano roll, arrange them on a playlist, and mix them through a routable mixer. It is written from the
ground up in Rust, with a sample-accurate audio engine, a fast CPU-rendered interface and support for
modern open plugin standards.

A central goal is **interoperability**. Lemon Studio reads and writes the `.flp` project format
losslessly, so existing projects open, play and save without losing data it does not yet understand.

> [!WARNING]
> **Lemon Studio is pre-alpha software.** It is under active development and is not yet a usable DAW:
> expect missing features, rough edges and breaking changes.
>
> - **Greyed-out controls are not implemented yet.** Many menus and buttons are already laid out but do
>   nothing so far; they are drawn dim and say "not available yet".
> - **No prebuilt binaries** will be published until Lemon Studio reaches a usable state. Until then it
>   can only be built from source.
> - **Only projects saved by the latest FL Studio 26 open for now.** Projects from older FL Studio versions
>   are not supported yet; support for them is on the roadmap.
> - **Project-file support is at a very early stage.** Do not overwrite your original `.flp` projects
>   with files saved by Lemon Studio until this support is declared stable. Work on copies and keep backups.
> - **Support is not available yet**, and issue reports will not be tracked until a later stage of
>   development.

## Capabilities

A broad picture of where Lemon Studio stands against what a complete DAW offers. Controls for anything that
is not implemented yet are **greyed out** in the application: hovering one says "not available yet", and
clicking it does nothing.

| Area | Status | Notes |
|---|:---:|---|
| Step sequencing and patterns | ✅ | Channel rack with steps, mini piano rolls, swing, per-channel loops |
| Piano roll note editing | ✅ | Full tool set, velocity and event lanes, quantize / chop / glue / legato / strum / arpeggiate / randomize, vertical zoom, ghost channels, pattern time markers (saved) |
| Song arrangement | ✅ | Playlist with pattern, audio and automation clips, clip groups, markers, several arrangements; track height, zoom and scroll saved with the project |
| Automation | 🚧 | Automation clips for channel, mixer and plugin parameters; no recording of movements |
| Mixing and routing | ✅ | Mixer tracks, sends, sidechain, effect slots, meters |
| Plugin hosting | ✅ | CLAP and VST3 instruments and effects, with their editors and saved state |
| Built-in instruments and effects | 🚧 | A sampler, a three-oscillator synth and a parametric EQ so far |
| Exporting audio | 🚧 | Song or pattern to a WAV file (32-bit float); no MP3 or FLAC yet |
| Project files | 🚧 | Opens and saves `.flp` projects from the latest FL Studio 26 only (older versions are on the roadmap); early, always keep backups |
| Undo | 🚧 | Per editor (piano roll, playlist); no redo or global history |
| Note input | 🚧 | Mouse, computer keyboard and MIDI controllers (notes play the selected channel); no note recording yet |
| Audio recording | ❌ | Not available yet |
| MIDI file import / export | ❌ | Not available yet |
| Audio editing and time-stretching | ❌ | Not available yet |
| Plugin delay compensation | ❌ | Not available yet |
| Audio drivers | 🚧 | WASAPI (shared mode) on Windows, ALSA (and PipeWire / PulseAudio through it) on Linux; no ASIO or JACK yet |
| Platforms | 🚧 | Windows 10 / 11; Linux (X11, and Wayland through XWayland); no macOS yet |
| Look and themes | 🚧 | Lemon design (lemon on neutral greys, rounded-square controls) and the Classic look, switched with Ctrl+Shift+D; theme files apply to Classic; plugin editors not restyled yet |

✅ usable · 🚧 partial · ❌ not available yet

The full, item-by-item roadmap lives on the [Discord server](https://discord.gg/yC2uCuGs3M).

## Getting started

### Prerequisites

- **Windows 10 or 11** (x64), or **Linux** (x64; Wayland or X11). macOS support is planned.
- **Rust**, stable toolchain with 2024 edition support (1.85 or newer), via [rustup](https://rustup.rs/).
- *Optional:* the Windows SDK, used to embed the application icon. Without it the build still succeeds.
- *Linux:* the ALSA development files and `pkg-config` (Debian / Ubuntu: `libasound2-dev pkg-config`; Arch:
  `alsa-lib pkgconf`; Fedora: `alsa-lib-devel pkgconf-pkg-config`). For the "save changes?" prompt, `zenity` or
  KDE's `kdialog`.

### Build and run

No prebuilt binaries are provided at this stage; Lemon Studio is built from source.

```bash
git clone https://github.com/ItsAlphaNeon/Lemon-Studio.git
cd Lemon-Studio
cargo run -p os-app --release
```

Use `FILE > Export as WAV` (Ctrl+R) to render the song, or the current pattern in PAT mode, to a file.
Ctrl+Shift+D switches between the Lemon and Classic designs (the choice is remembered).

Open a project directly by passing its path:

```bash
cargo run -p os-app --release -- path/to/project.flp
```

### Plugins

Build Open Sampler, Open 3x Osc and Open Parametric EQ 2 as VST3 and CLAP bundles (written to `target/bundled/`):

```bash
cargo run -p bundle-plugins
```

Pass `--install` to copy the bundles into the per-user plugin folders (`%LOCALAPPDATA%\Programs\Common\{VST3,CLAP}` on
Windows, `~/.vst3` and `~/.clap` on Linux).

### Linux

Lemon Studio runs on X11, and on Wayland desktops through XWayland. Plugin editors on Linux are X11 windows, and an X11
window cannot sit inside a native Wayland one; with the main window on X11 too, plugin windows dock inside it as on
Windows (or detach, from their window menu). A few things differ from Windows for now:

- **Native Wayland** is used only when there is no X server, or when `OPENSTUDIO_WAYLAND=1` is set. Plugin windows are
  then always separate windows, kept above Lemon Studio while it is the active application; their window menu, Presets
  list and Misc lists do not open, and files cannot be dropped in from a file manager (the windowing library does not
  support it there).
- **Scaling:** under XWayland Lemon Studio follows the X11 scale (`Xft.dpi`). If it looks soft on a fractionally scaled
  Wayland desktop, let X11 applications scale themselves (KDE: Display Configuration > Legacy applications (X11) >
  Apply scaling themselves).
- **Keys** pressed while a plugin's editor has the keyboard focus do not play the typing keyboard yet, and a sample
  dragged from Lemon Studio's browser onto a plugin window does not reach the plugin yet.
- **Open Sampler** cannot copy or paste audio through the clipboard yet.
- **Settings, themes and presets** live in `$XDG_CONFIG_HOME/Lemon Studio` (usually `~/.config/Lemon Studio`);
  on Windows in `%APPDATA%\Lemon Studio`. `OPENSTUDIO_USER_DIR` overrides the folder.
- Plugins are found in `~/.clap`, `/usr/lib/clap`, `/usr/local/lib/clap` and `CLAP_PATH`, and in `~/.vst3`,
  `/usr/lib/vst3` and `/usr/local/lib/vst3`.

To install the app with its icon and menu entry (the desktop entry also names the icon under native Wayland):

```bash
cargo install --path crates/os-app
install -Dm644 crates/os-app/assets/lemonstudio.png ~/.local/share/icons/hicolor/256x256/apps/lemonstudio.png
install -Dm644 crates/os-app/assets/lemonstudio.desktop ~/.local/share/applications/lemonstudio.desktop
```

### Tests

```bash
cargo test --workspace
```

Tests that need an audio device are marked `#[ignore]` and run with `cargo test -p os-app -- --ignored`.
Project-file corpus tests run when `OPENSTUDIO_FLP_CORPUS` points to a text file listing `.flp` paths;
project files are never committed to this repository.

### Command-line tools

| Tool | Purpose |
|---|---|
| `flp-dump` | Print a project file's event stream and verify that it round-trips |
| `os-render` | Render a project or pattern offline to a WAV file |
| `os-play` | Play a project through the default audio output |

## Architecture

Lemon Studio is a Cargo workspace of focused crates:

| Crate | Responsibility |
|---|---|
| [`os-model`](crates/os-model) | Project data model, free of I/O |
| [`os-flp`](crates/os-flp) | Lossless `.flp` event-stream reader/writer and model mapping |
| [`os-engine`](crates/os-engine) | Audio engine: sampler, mixer, routing, effects, envelopes, automation |
| [`os-dsp`](crates/os-dsp) | DSP building blocks: level curve, pan split, gain glides, mixer matrix and equaliser |
| [`os-theme`](crates/os-theme) | Colour palette, designs (Lemon, Classic), theme files and the HSL adjustment model |
| [`os-ui`](crates/os-ui) | CPU canvas, text rendering and the shared widget kit |
| [`os-app`](crates/os-app) | The `lemonstudio` application: windows, editors, transport |
| [`os-host`](crates/os-host) | Format-neutral plugin hosting layer (CLAP, VST3, native) |
| [`os-clap`](crates/os-clap) | CLAP plugin host |
| [`os-vst3`](crates/os-vst3) | VST3 plugin host |
| [`os-hostlink`](crates/os-hostlink) | Host link protocol between Lemon Studio and its own plugins |
| [`os-native`](crates/os-native) | Experimental host for plugins built on a third-party native plugin SDK (Windows only) |
| [`os-plugkit`](crates/os-plugkit) | Shared editor pieces of Lemon Studio's own plugins |
| [`os-paths`](crates/os-paths) | Where Lemon Studio keeps its settings, themes and presets on each platform |
| [`open-sampler`](plugins/open-sampler) | Open Sampler instrument (VST3 + CLAP) |
| [`open-3x-osc`](plugins/open-3x-osc) | Open 3x Osc instrument (VST3 + CLAP) |
| [`open-parametric-eq-2`](plugins/open-parametric-eq-2) | Open Parametric EQ 2 effect (VST3 + CLAP) |

## Roadmap

The roadmap is kept on the [Lemon Studio Discord server](https://discord.gg/yC2uCuGs3M), where every item is
marked as complete, in progress or not started. The broad next steps are the engine rework (sampler voice,
automation, delay compensation), opening projects from older FL Studio versions, audio and note recording, MIDI
settings, ASIO output, more built-in instruments and effects, the rest of the plugin window features on Linux, and
macOS support.

## Development approach

Lemon Studio is developed independently. File-format and behaviour details needed for
interoperability are first written down as specifications, and the product code is implemented
from those specifications and public documentation. All artwork, icons and interface graphics
in this repository are original or come from the openly licensed projects listed below.

## Contributing

Lemon Studio is not accepting outside contributions yet. Once it does, pull requests will be welcome
from developers who are comfortable with Rust and the pull-request workflow.

All contributions must be **clean-room** work. Decompiled code, and code or pseudocode derived from
decompiled output, will not be accepted in any form: the codebase must contain none of it.

## Community

Join the [Lemon Studio Discord server](https://discord.gg/yC2uCuGs3M) to follow development and talk
about the project. Please keep in mind that support is not offered at this stage.

## Third-party components

| Component | Use | License |
|---|---|---|
| [Cuprum](https://fonts.google.com/specimen/Cuprum) by Jovanny Lemonad | Interface typeface | [SIL Open Font License 1.1](crates/os-ui/fonts/Cuprum/OFL.txt) |
| [Tabler Icons](https://tabler.io/icons) | Interface icons | [MIT](crates/os-ui/icons/TABLER-LICENSE.txt) |
| [NIH-plug](https://github.com/robbert-vdh/nih-plug) | Plugin framework for Lemon Studio's plugins | ISC |
| [vst3-sys](https://github.com/RustAudio/vst3-sys) | VST3 interface bindings (plugin hosting and Lemon Studio's plugins) | GPLv3 |
| Rust crates listed in `Cargo.lock` | Various | Their respective licenses |

## License

Lemon Studio is free software: you can redistribute it and/or modify it under the terms of the
[GNU General Public License](LICENSE), version 3 or (at your option) any later version.

## Trademarks

FL Studio is a trademark of Image-Line Software. Lemon Studio is an independent project and is not
affiliated with, sponsored by or endorsed by Image-Line Software. The `.flp` file format is referred
to solely to describe interoperability.

VST is a registered trademark of Steinberg Media Technologies GmbH. All other trademarks are the
property of their respective owners.

<div align="center">
<br>
<img alt="" src="docs/assets/mark.png" width="48">
</div>
