<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/logo-light.png">
  <img alt="OpenStudio" src="docs/assets/logo-light.png" width="520">
</picture>

<h3>A free and open-source digital audio workstation, written in Rust.</h3>

<p>
  <a href="LICENSE"><img alt="License: GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue?style=flat-square"></a>
  <img alt="Rust 2024 edition" src="https://img.shields.io/badge/rust-2024_edition-e57324?style=flat-square&logo=rust&logoColor=white">
  <img alt="Platform: Windows" src="https://img.shields.io/badge/platform-Windows-0078d4?style=flat-square">
  <img alt="Status: pre-alpha" src="https://img.shields.io/badge/status-pre--alpha-orange?style=flat-square">
</p>

<p>
  <a href="#overview">Overview</a> ·
  <a href="#features">Features</a> ·
  <a href="#getting-started">Getting started</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="#roadmap">Roadmap</a> ·
  <a href="#license">License</a>
</p>

</div>

---

## Overview

OpenStudio is a pattern-based music production environment: build patterns in a step sequencer and
piano roll, arrange them on a playlist, and mix them through a routable mixer. It is written from the
ground up in Rust, with a sample-accurate audio engine, a fast CPU-rendered interface and support for
modern open plugin standards.

A central goal is **interoperability**. OpenStudio reads and writes the `.flp` project format
losslessly, so existing projects open, play and save without losing data it does not yet understand.

> [!NOTE]
> OpenStudio is in early, active development. Many features are complete, but expect missing pieces,
> rough edges and breaking changes. It is not yet ready for production work.

## Features

<table>
<tr>
<td width="50%" valign="top">

### 🎛️ Audio engine
- Sample-accurate pattern and song playback
- Sampler with high-quality windowed-sinc resampling
- Mixer graph with sends, sidechain routing and measured gain, pan and pitch laws
- Built-in effects: parametric EQ and limiter
- Channel envelopes, note repeat and per-pattern control lanes
- Automation clips for channel and mixer parameters
- Live edits reach playback instantly, without clicks
- Offline rendering and low-latency WASAPI output

</td>
<td width="50%" valign="top">

### 🖥️ Interface
- Playlist with clips, markers, automation and nine editing tools
- Channel rack: step sequencer, mini piano rolls, graph editor, per-channel loops, filter groups, fill tools, swing, clone / move / delete channels
- Piano roll with full tool set, velocity and event lanes
- Mixer: track select / rename / colour, mute and solo, polarity and channel swap, click-to-route sends with send knobs
  and sidechain, per-track meters, add / insert / move / delete tracks, effect slots hosting CLAP effects (bypass, dry/wet)
- Browser with drag and drop, plugin picker
- Floating, resizable, stackable windows
- Theme engine with `.flstheme` import
- Damage-tracked CPU rendering for fast repaints
- Computer-keyboard note input

</td>
</tr>
<tr>
<td width="50%" valign="top">

### 📁 Project files
- Lossless `.flp` reader and writer: unknown data is preserved byte for byte
- Typed project model for patterns, channels, mixer, playlist and automation
- New projects saved in a form other compatible software opens
- Channel rack settings saved: swing, filter groups, locked and zipped rows, pattern length, loop lengths, and added, cloned, moved or deleted channels
- Round-trip test suite over a corpus of real-world projects

</td>
<td width="50%" valign="top">

### 🔌 Plugins
- [CLAP](https://cleveraudio.org/) instrument hosting with native editor windows
- **Open Sampler**: OpenStudio's sampler, also built as a standalone
  VST3 and CLAP plugin for use in other hosts
- **Open Parametric EQ 2**: 7-band EQ (VST3 + CLAP) with a graph editor, band tokens,
  spectrum monitor and A/B compare; it plays and saves the EQ slots of `.flp` projects
- Host link protocol that lets OpenStudio's own plugins integrate with the host's window frame

</td>
</tr>
</table>

## Getting started

### Prerequisites

- **Windows 10 or 11** (x64). Linux and macOS support is planned.
- **Rust**, stable toolchain with 2024 edition support (1.85 or newer), via [rustup](https://rustup.rs/).
- *Optional:* the Windows SDK, used to embed the application icon. Without it the build still succeeds.

### Build and run

```bash
git clone <repository-url> OpenStudio
cd OpenStudio
cargo run -p os-app --release
```

Open a project directly by passing its path:

```bash
cargo run -p os-app --release -- path/to/project.flp
```

### Plugins

Build Open Sampler and Open Parametric EQ 2 as VST3 and CLAP bundles (written to `target/bundled/`):

```bash
cargo run -p bundle-plugins
```

Pass `--install` to copy the bundles into the per-user plugin folders.

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

OpenStudio is a Cargo workspace of focused crates:

| Crate | Responsibility |
|---|---|
| [`os-model`](crates/os-model) | Project data model, free of I/O |
| [`os-flp`](crates/os-flp) | Lossless `.flp` event-stream reader/writer and model mapping |
| [`os-engine`](crates/os-engine) | Audio engine: sampler, mixer, routing, effects, envelopes, automation |
| [`os-theme`](crates/os-theme) | Colour palette, theme files and the HSL adjustment model |
| [`os-ui`](crates/os-ui) | CPU canvas, text rendering and the shared widget kit |
| [`os-app`](crates/os-app) | The `openstudio` application: windows, editors, transport |
| [`os-clap`](crates/os-clap) | CLAP plugin host |
| [`os-hostlink`](crates/os-hostlink) | Host link protocol between OpenStudio and its own plugins |
| [`os-native`](crates/os-native) | Experimental host for plugins built on a third-party native plugin SDK |
| [`os-plugkit`](crates/os-plugkit) | Shared editor pieces of OpenStudio's own plugins |
| [`open-sampler`](plugins/open-sampler) | Open Sampler instrument (VST3 + CLAP) |
| [`open-parametric-eq-2`](plugins/open-parametric-eq-2) | Open Parametric EQ 2 effect (VST3 + CLAP) |

## Roadmap

- [x] Lossless project file reading and writing
- [x] Audio engine with mixer, routing and effects
- [x] Playlist, channel rack, piano roll, mixer and browser
- [x] CLAP hosting (instruments and mixer effects) and the Open Sampler plugin
- [x] Open Parametric EQ 2 plugin, built in for mixer slots
- [x] Theme engine and theme import
- [ ] VST3 hosting
- [ ] ASIO audio output
- [ ] Saving plugin channels and full mixer state to project files
- [ ] More built-in instruments and effects
- [ ] UI scaling and high-DPI layouts
- [ ] Linux and macOS support

## Development approach

OpenStudio is developed independently. File-format and behaviour details needed for
interoperability are first written down as specifications, and the product code is implemented
from those specifications and public documentation. All artwork, icons and interface graphics
in this repository are original or come from the openly licensed projects listed below.

## Third-party components

| Component | Use | License |
|---|---|---|
| [Cuprum](https://fonts.google.com/specimen/Cuprum) by Jovanny Lemonad | Interface typeface | [SIL Open Font License 1.1](crates/os-ui/fonts/Cuprum/OFL.txt) |
| [Tabler Icons](https://tabler.io/icons) | Interface icons | [MIT](crates/os-ui/icons/TABLER-LICENSE.txt) |
| [NIH-plug](https://github.com/robbert-vdh/nih-plug) | Plugin framework for OpenStudio's plugins | ISC |
| Rust crates listed in `Cargo.lock` | Various | Their respective licenses |

## License

OpenStudio is free software: you can redistribute it and/or modify it under the terms of the
[GNU General Public License](LICENSE), version 3 or (at your option) any later version.

## Trademarks

FL Studio is a trademark of Image-Line Software. OpenStudio is an independent project and is not
affiliated with, sponsored by or endorsed by Image-Line Software. The `.flp` file format is referred
to solely to describe interoperability.

VST is a registered trademark of Steinberg Media Technologies GmbH. All other trademarks are the
property of their respective owners.

<div align="center">
<br>
<img alt="" src="docs/assets/mark.png" width="48">
</div>
