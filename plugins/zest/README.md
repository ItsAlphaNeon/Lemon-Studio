# Zest

An analog-style synthesizer aiming at the sound of u-he Diva for the models it has, and loading Diva's presets when they are installed.

- `crates/zest-core`: the engine, written from the pages on the specs repository's `cutting-edge` branch, `zest/engine/`. `src/bin/zest-render.rs` renders MIDI and measures CPU.
- `crates/zest-ui`, `crates/zest-plugin`: the editor and the VST3/CLAP plugin. `crates/zest-preset`: the `.zest` format.
- `banks/`: compiled in, `<Bank>/<nn> <Preset>.zest`. Space holds borrowed Diva presets for showing Zest, to be replaced by Zest's own. Further banks can go in `~/.local/share/zest/banks/`, beside the user's saved presets and the Diva presets folder.

x86-64 only: the engine uses SSE and AVX directly.
