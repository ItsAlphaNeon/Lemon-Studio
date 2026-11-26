# Zest

An analog-style synthesizer aiming at the sound of u-he Diva for the models it has, and loading Diva's presets when they are installed.

- `crates/zest-core`: the engine, written from the pages on the specs repository's `cutting-edge` branch, `zest/engine/`. `src/bin/zest-render.rs` renders MIDI and measures CPU.
- `crates/zest-ui`, `crates/zest-plugin`: the editor and the VST3/CLAP plugin. `crates/zest-preset`: the `.zest` format.
- `banks/`: compiled in, `<Bank>/<nn> <Preset>.zest`. Space holds borrowed Diva presets for showing Zest, to be replaced by Zest's own. Further banks can go in the `banks` folder of Zest's user folder (`~/.local/share/zest`, `~/Library/Application Support/Zest`, `%APPDATA%\Zest`), beside the user's saved presets and the Diva presets folder.

Every processor: on x86-64 the engine uses SSE (and AVX when there), elsewhere `zest-core/src/simd/soft.rs`, the same
operations in plain Rust giving the same samples. The `portable` feature uses them on x86-64 too; all 2,589 bench cases render
byte for byte the same either way. On x86-64 they take about 1.5 times as long. The editor runs on Linux, macOS and Windows.
