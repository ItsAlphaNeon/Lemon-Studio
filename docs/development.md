# Development notes

The details that used to be in the README. For how to install and use Lemon Studio, see the
[wiki](https://getlemonstudio.app/wiki/). Building on each system is in the
[build guide](https://getlemonstudio.app/wiki/reference/build-from-source/), and what differs on Linux and macOS is in
[Troubleshooting](https://getlemonstudio.app/wiki/reference/troubleshooting/).

## Running from source

`cargo run -p os-app --release` builds and runs Lemon Studio. Before Lemon Studio starts, it also rebuilds Lemon Studio's
own plugins and installs the ones that changed as VST3 and CLAP plugins, in the folders where other DAWs find them. Lemon
Studio itself has every one of them built in and needs no plugin files. A plugin that is new or updated in the repository
shows up in other DAWs at the next run. The first run builds every plugin, which takes a few minutes more; later runs
rebuild only what changed. Set `OPENSTUDIO_NO_PLUGIN_BUILD=1` to skip the plugin build. A Lemon Studio installed with
`cargo install` builds nothing: after pulling changes, either run the command above once, or build and install only the
plugins with `cargo run -p bundle-plugins -- --install`.

Open a project directly by passing its path:

```bash
cargo run -p os-app --release -- path/to/project.lemon
```

The render that `FILE > Export audio` does also runs without a window, hosting the same plugins the app would and without
opening an audio device:

```bash
cargo run -p os-app --release -- --render path/to/project.lemon out.wav [--rate 48000] [--seconds 30] [--from TICK] [--builtin-only]
```

`--builtin-only` hosts only Lemon Studio's own plugins, for a render that is the same on every machine.

On Windows, `lemonstudio.exe` is a GUI program: started from Explorer, the Start menu or a shortcut it opens no console
window. Under `cargo run` its output goes to the terminal as before. Started directly from a terminal it prints there too,
but the terminal does not wait for it: for `--render`, add `| Out-Default` in PowerShell (or use `start /wait` in `cmd`)
to wait until the render is done.

## Where things live

`OPENSTUDIO_USER_DIR` overrides the folder for settings, themes and presets. By default it is:

| System | Folder |
|---|---|
| Windows | `%APPDATA%\Lemon Studio` |
| Linux | `$XDG_CONFIG_HOME/Lemon Studio` (usually `~/.config/Lemon Studio`) |
| macOS | `~/Library/Application Support/Lemon Studio` |

Plugins are found in these folders, plus any added in Settings > Plugins:

| System | Folders |
|---|---|
| Windows | `%LOCALAPPDATA%\Programs\Common\{VST3,CLAP}`, `%COMMONPROGRAMFILES%\{VST3,CLAP}` and `CLAP_PATH`. VST2: the folder installers record (`HKEY_LOCAL_MACHINE\SOFTWARE\VST`, `VSTPluginsPath`) and those of `%PROGRAMFILES%\VSTPlugins`, `%PROGRAMFILES%\Steinberg\VSTPlugins`, `%COMMONPROGRAMFILES%\VST2` and `%COMMONPROGRAMFILES%\Steinberg\VST2` that exist |
| Linux | `~/.clap`, `/usr/lib/clap`, `/usr/local/lib/clap` and `CLAP_PATH`; `~/.vst3`, `/usr/lib/vst3` and `/usr/local/lib/vst3`. VST2: `VST_PATH`, else `~/.vst`, `/usr/lib/vst`, `/usr/local/lib/vst` and the `lxvst` folders that exist |
| macOS | `~/Library/Audio/Plug-Ins/{CLAP,VST3,VST}`, `/Library/Audio/Plug-Ins/{CLAP,VST3,VST}` and `CLAP_PATH` |

On Linux, `OPENSTUDIO_WAYLAND=1` asks for native Wayland even with an X server. On macOS, `OPENSTUDIO_FACTORY` overrides
where FL Studio's factory content is looked for (by default the newest `/Applications/FL Studio *.app`).

## Tests

```bash
cargo test --workspace
```

Tests that need an audio device are marked `#[ignore]` and run with `cargo test -p os-app -- --ignored`. Project-file
corpus tests run when `OPENSTUDIO_FLP_CORPUS` points to a text file listing `.flp` paths; project files are never
committed to this repository. The watermark test (`cargo test -p watermark`) checks that every source file opens with its
header: a copyright line for each of its copyright holders, its SPDX licence identifier and the project's name.

Every test binary is compiled and linked on its own, so a crate keeps its integration tests in one:
`tests/integration/main.rs` names each file there as a module (run one with `cargo test -p os-engine --test integration
fx_null`); a new file goes there, not beside it. The crates have no doc tests (`doctest = false`: code in doc comments is
`text` or `ignore`), and a crate without tests has `test = false`; take that out when adding its first test.

## Scripting and assistants

`--control <socket>` lets scripts, tests and AI assistants read and change the open project over a local socket (JSON-RPC
2.0), and `--serve <socket> [project]` does the same without a window. Each change is an undo step named after the
client; clients can be limited to reading or to proposing changes the user accepts. The protocol and its commands are in
[control-plane.md](control-plane.md).

```bash
cargo run -p os-app --release -- --control /tmp/lemon.sock path/to/project.lemon
```

## Command-line tools

| Tool | Purpose |
|---|---|
| `flp-dump` | Print a project file's event stream and verify that it round-trips |
| `os-render` | Render a project or pattern offline to a WAV file with the engine alone (no hosted plugins, so it does not sound like the app; `--render` above does) |
| `os-play` | Play a project through the default audio output |
| `bundle-plugins` | Build Lemon Studio's own plugins as VST3 and CLAP bundles, and optionally install them (`cargo run -p os-app --release` runs it with `--install`) |
| `bundle-app` | Build Lemon Studio as a macOS app, and optionally install it |
| `notices` | Generate the last section of [THIRD-PARTY-NOTICES.txt](../THIRD-PARTY-NOTICES.txt) (below) |
| `updater` | `lemonstudio-updater`, which release builds carry to update themselves (built by the release workflow only; [releasing.md](releasing.md)) |

## Code layout

Lemon Studio is a Cargo workspace of focused crates:

| Crate | Responsibility |
|---|---|
| [`os-model`](../crates/os-model) | Project data model, free of I/O |
| [`os-flp`](../crates/os-flp) | Lossless project event-stream reader and writer, and model mapping |
| [`os-smf`](../crates/os-smf) | Standard MIDI files |
| [`os-engine`](../crates/os-engine) | Audio engine: sampler, mixer, routing, effects, envelopes, automation |
| [`os-dsp`](../crates/os-dsp) | DSP building blocks: level curve, pan split, gain glides, mixer matrix and equaliser |
| [`os-theme`](../crates/os-theme) | Colour palette, skins (Lemon, Citrus), theme files and the HSL adjustment model |
| [`os-ui`](../crates/os-ui) | CPU canvas, text rendering and the shared widget kit |
| [`os-app`](../crates/os-app) | The `lemonstudio` application: windows, editors, transport |
| [`os-host`](../crates/os-host) | Format-neutral plugin hosting layer (CLAP, VST3, VST2) |
| [`os-clap`](../crates/os-clap) | CLAP plugin host |
| [`os-vst3`](../crates/os-vst3) | VST3 plugin host |
| [`os-vst2`](../crates/os-vst2) | VST2 plugin host, on a clean-room description of the interface (no Steinberg SDK) |
| [`os-hostlink`](../crates/os-hostlink) | Host link protocol between Lemon Studio and its own plugins |
| [`os-plugkit`](../crates/os-plugkit) | Shared editor pieces of Lemon Studio's own plugins |
| [`os-paths`](../crates/os-paths) | Where Lemon Studio keeps its settings, themes and presets on each platform |

Each built-in plugin is a crate in [`plugins/`](../plugins), built into Lemon Studio and also as a VST3 and CLAP plugin.
`lemon-small-fx` is what the small Lemon effects share, `lemon-3x-osc` is Lemon Trio and `lemon-parametric-eq-2` is Lemon
Parametric EQ.

## Old plugin names

Lemon Trio and Lemon Parametric EQ were called Lemon 3x Osc and Lemon Parametric EQ 2 before, and earlier all three
plugins were called Open Sampler, Open 3x Osc and Open Parametric EQ 2. They keep their plugin IDs, so sessions saved with
the old names still open with the new plugins. Installing the plugins (by the run from source or `bundle-plugins
--install`) removes bundles installed under the old names; delete any others you copied yourself, or DAWs will list each
plugin twice.

## Copyright headers

Contributors keep the copyright of their work and license it under the GPL, version 3 or later, like the rest of Lemon
Studio. In each file you write or substantially change, add a line with your name or username to the copyright lines at
the top of the header, `Copyright (C) <year> <name>`, and keep the lines already there. The two source files that hold
others' work (`crates/os-vst2/src/abi.rs`, `crates/os-ui/src/tabler.rs`) name it in their header.

## Third-party components

| Component | Use | License |
|---|---|---|
| [Cuprum](https://fonts.google.com/specimen/Cuprum) by Jovanny Lemonad | Interface typeface | [SIL Open Font License 1.1](../crates/os-ui/fonts/Cuprum/OFL.txt) |
| [Barlow](https://github.com/jpt/barlow) by The Barlow Project Authors | Interface typeface on Linux and macOS | [SIL Open Font License 1.1](../crates/os-ui/fonts/Barlow/OFL.txt) |
| [Tabler Icons](https://tabler.io/icons) | Interface icons | [MIT](../crates/os-ui/icons/TABLER-LICENSE.txt) |
| [NIH-plug](https://github.com/robbert-vdh/nih-plug) | Plugin framework for Lemon Studio's plugins | ISC |
| [vst3-sys](https://github.com/RustAudio/vst3-sys) | VST3 interface bindings (plugin hosting and Lemon Studio's plugins) | GPLv3 or later (its com crate MIT) |
| [vst2sdk](https://github.com/Xaymar/vst2sdk) by Xaymar | Clean-room description of the VST2 interface, which `os-vst2`'s bindings are written from | [BSD 3-Clause](../crates/os-vst2/LICENSE-vst2sdk.txt) |
| [FST](https://git.iem.at/zmoelnig/FST) by IOhannes m zmölnig | Reverse-engineered VST2 interface: the parts vst2sdk leaves out (time info, edit gestures) | GPLv3 or later |
| [Signalsmith Stretch](https://github.com/Signalsmith-Audio/signalsmith-stretch) by Geraint Luff / Signalsmith Audio Ltd, with Signalsmith Linear | Time stretching and pitch shifting | MIT |
| [signalsmith-stretch](https://github.com/colinmarc/signalsmith-stretch-rs) by Colin Marc | Rust bindings to Signalsmith Stretch | MIT |
| [ASIO SDK](https://www.steinberg.net/asiosdk) by Steinberg Media Technologies GmbH (host files) | ASIO output on Windows | [GPLv3](../crates/os-app/asiosdk/LICENSE.txt) (its GPL option); a Windows build with it is GPLv3 |
| [Cantarell](https://gitlab.gnome.org/GNOME/cantarell-fonts), built into the sctk-adwaita crate | Window title bars on Wayland (Linux) | SIL Open Font License 1.1 |
| [LAME](https://lame.sourceforge.io) 3.100, built by the mp3lame-sys crate | MP3 export | LGPL version 2 or later |
| Rust crates listed in `Cargo.lock` | Various | Their respective licenses |

Their copyright notices and licence texts are collected in [THIRD-PARTY-NOTICES.txt](../THIRD-PARTY-NOTICES.txt), which
goes with every copy of Lemon Studio next to [LICENSE](../LICENSE). Its last section, the Rust crates with their licence
texts, is generated by `cargo run -p notices` (with [cargo-about](https://github.com/EmbarkStudios/cargo-about):
`cargo install cargo-about --locked --features cli`); `cargo test` fails while a crate that goes into a build is missing
from it, so run it after changing a dependency.
