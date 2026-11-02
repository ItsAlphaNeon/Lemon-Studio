# Zest

An analog-style synthesizer aiming at the sound of u-he Diva for the models it has, and loading Diva's presets when they are installed.

- `crates/zest-core`: the engine, written from the pages in the specs repository, `zest/engine/`. `src/bin/zest-render.rs` renders MIDI and measures CPU.
- `crates/zest-ui`, `crates/zest-plugin`: the editor and the VST3/CLAP plugin. `crates/zest-preset`: the `.zest` format.
- `banks/`: compiled in, `<Bank>/<nn> <Preset>.zest`. Space holds borrowed Diva presets for showing Zest, to be replaced by Zest's own. Further banks can go in the `banks` folder of Zest's user folder (`~/.local/share/zest`, `~/Library/Application Support/Zest`, `%APPDATA%\Zest`), beside the user's saved presets and the Diva presets folder.

The engine uses SSE and AVX directly on x86-64; other processors get the same operations from `zest-core/src/simd.rs`. The editor runs on Linux, macOS and Windows.
