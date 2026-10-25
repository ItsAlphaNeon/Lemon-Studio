# Alpha checklist

What stands between Lemon Studio and its first alpha: a fully functional DAW at FL Studio parity. Third-party plugins
(CLAP, VST3, VST2) are not part of the alpha bar; hosting them already works, and the rest of that work comes later.

The list started from an assessment of `main` at `a637e72` on 2026-10-24. Tick an item in the change that finishes it;
for one under way, add "(in progress, #PR)" after it. Add items here as they turn up. What already works is in
[status.md](status.md).

## Alpha blockers

Systems a fully functional DAW can't ship without.

- [x] **Audio editor.** Tesla, after FL's Edison, in a mixer slot (#158), with the app's audio-editor entries wired to
  it: Open audio editor and record, One-click recording and the Record question into it, Remove all instances.
  TOOLS ▸ Audio logger and Tesla's later tools are listed under Gaps and polish
- [ ] **Audio recording into mixer tracks.** Arm and disarm tracks and record their inputs; today only one-click
  recording into the playlist works ([mixer.rs](../crates/os-app/src/mixer.rs))
- [ ] **Input monitoring.** Hear an input through its mixer track while it is armed or recording
- [ ] **Automation recording into clips.** Only channel volume and pan moves record, and only into the pattern
- [ ] **Automation clip LFO and min / range.** Clips walk all thirteen curve shapes but have neither yet
- [ ] **Projects saved before FL Studio 24.2.** They are refused with a dialog that asks for a re-save in a newer
  version
- [ ] **Multi-core mixing.** The engine renders on one audio thread, which limits dense projects
- [ ] **Engine rework (sampler voice, automation).** The README's roadmap names it as the next big step; land it
  before the alpha rather than after

## Built-in instruments and effects

Without third-party plugins, the built-in ones are the whole sound palette. Lemon Studio has three instruments and
twelve effects; these are the gaps a project made in FL Studio runs into most.

- [ ] **Drum sampler (FPC).** A pad-based drum instrument
- [ ] **Slicer (Slicex, Fruity Slicer).** A loop chopped into slices that the piano roll plays
- [ ] **Filter effect (Fruity Filter, Fruity Love Philter)**
- [ ] **Flanger and phaser.** Lemon Chorus exists; the other modulation effects don't
- [ ] **Vocoder (Vocodex)**
- [ ] **Peak Controller.** Common for sidechain pumping in projects; nothing in the code handles it yet
- [ ] **Formula Controller**
- [ ] **Patcher.** The mixer slot menu's Patcherize and Patcherize all are greyed out
- [ ] **Transient Processor, Chorus and Stereo Shaper compared with FL Studio 2026's exports.** The other four small
  effects already match sample for sample in the tests

## Gaps and polish

Greyed-out controls and partial features. These can land alongside the blockers.

- [ ] **Event editor window.** Edit events is greyed out in five menus (playlist, channel rack, parameter menus)
- [ ] **Undo for a plugin editor's own changes**
- [ ] **Undo for mixer track commands.** Mute, routing, names and slots are not in the history yet
- [x] **Undo for the remaining edits.** A generator inserted higher up the rack, Replace, project info and the
  timebase are each one step
- [ ] **Mixer right-click menus.** 49 entries greyed out on 2026-10-24: the routing menu, groups, Render to wave
  file, spare states, smart disable ([mixer.rs](../crates/os-app/src/mixer.rs))
- [ ] **Playlist right-click menus.** 36 entries greyed out on 2026-10-24: key markers, Add two..., quantized song
  jumps, the Start / Stop recording markers' punch ([playlist.rs](../crates/os-app/src/playlist.rs))
- [ ] **Channel rack right-click menus.** 25 entries greyed out on 2026-10-24: Type in value, Burn selected loops to
  pattern, instrument tracks, Advanced looping ([rack.rs](../crates/os-app/src/rack.rs))
- [ ] **Main menu bar.** 24 of its 134 entries greyed out on 2026-10-24: 13 in TOOLS, 5 in VIEW, 4 in HELP, 2 in ADD
  ([menus.rs](../crates/os-app/src/views/menus.rs))
- [ ] **TOOLS ▸ Audio logger and Dump to audio editor.** FL keeps the master's last minutes in memory; not measured
  yet. Tesla's rolling recording goes with it: at its time limit Tesla stops, where Edison keeps the last minutes
- [ ] **Tesla's later tools.** Envelopes, equalize, blur and convolution reverb, clean-up, time stretch and pitch
  shift, slicing and beat detection (group 5 of Tesla's roadmap in the specs)
- [x] **Value boxes.** All of them work, the last two being the autosave Maximum and Metronome track. MIDI settings'
  Performance mode MIDI channel stays greyed out until Performance mode is built
- [ ] **Performance mode.** Live clip launching on the playlist (Ctrl+P says it is not available yet), with MIDI
  settings' Performance mode MIDI channel and TOOLS ▸ Macros ▸ Prepare for performance mode
- [ ] **Recording settings page.** The only settings page that doesn't work yet
- [ ] **Editable mixer latency settings.** Plugin delay compensation reads, uses and saves them, but they can't be
  edited
- [ ] **Browser: Favorites, the plugin database and Browser find**
- [ ] **JACK on Linux**
- [ ] **ASIO output pairs.** The master always plays to the driver's first two outputs

## Shipping a build

What has to exist before anyone outside can run the alpha.

- [ ] **Prebuilt binaries for Windows, macOS and Linux.** Today Lemon Studio is built from source only
- [x] **Complete [THIRD-PARTY-NOTICES.txt](../THIRD-PARTY-NOTICES.txt).** Every crate that goes into a build is
  listed with its licence text, generated by `cargo run -p notices`; `cargo test` fails when one is missing
- [ ] **macOS signing and notarization.** Builds and plugin bundles are signed ad hoc only
- [ ] **Plugin crashes contained on macOS and Linux.** Windows only today; it matters once third-party plugins are in
  play
