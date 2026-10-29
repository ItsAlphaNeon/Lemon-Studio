# Alpha checklist

What stands between Lemon Studio and its first alpha: a fully functional DAW at FL Studio parity. Third-party plugins
(CLAP, VST3, VST2) are not part of the alpha bar; hosting them already works, and the rest of that work comes later.

The list started from an assessment of `main` at `a637e72` on 2026-10-24 and was checked again against `fb4382e` on
2026-10-29. Tick an item in the change that finishes it;
for one under way, add "(in progress, #PR)" after it. Add items here as they turn up. What already works is in
[status.md](status.md).

## Alpha blockers

Systems a fully functional DAW can't ship without.

- [x] **Audio editor.** Tesla, after FL's Edison, in a mixer slot (#158), with the app's audio-editor entries wired to
  it: Open audio editor and record, One-click recording and the Record question into it, Remove all instances.
  TOOLS ▸ Audio logger and Tesla's later tools are listed under Gaps and polish
- [x] **Audio recording into mixer tracks.** With Audio in Record's filter, armed tracks record at their record source
  while the song plays; each take becomes a WAV and an audio clip where it started, the input's delay taken out. The
  mixer menu's Arm and Disarm selected track(s) and Disk recording ▸ Auto-create audio clips work
- [x] **Input monitoring.** A track's input plays through it while it is armed, or always, per its Monitor external
  input setting (Off / When armed / On), also while stopped (#182)
- [ ] **Automation recording into clips.** Only channel volume and pan moves record, and only into the pattern
- [x] **Automation clip LFO and min / range.** The clip channel's MIN / MAX knobs set its range and its LFO
  (switch, mode, amount, speed, tension, skew, pulse width, its own envelope) plays as the original's, in the clip's
  channel window; files keep every LFO field
- [ ] **Projects saved before FL Studio 24.2.** They are refused with a dialog that asks for a re-save in a newer
  version
- [x] **Multi-core mixing.** The engine renders on one audio thread, which limits dense projects
- [ ] **Engine rework (sampler voice, automation).** The README's roadmap names it as the next big step; land it
  before the alpha rather than after

## Built-in instruments and effects

Without third-party plugins, the built-in ones are the whole sound palette. Lemon Studio has three instruments,
sixteen effects and the Tesla audio editor; these are the gaps a project made in FL Studio runs into most.

- [ ] **Drum sampler (FPC).** A pad-based drum instrument
- [ ] **Slicer (Slicex, Fruity Slicer).** A loop chopped into slices that the piano roll plays
- [x] **Filter effect (Fruity Filter, Fruity Love Philter).** Lemon Filter and Pulp Filter, each also playing the
  original's slots in opened projects
- [x] **Flanger and phaser.** Lemon Flanger and Lemon Phaser recreate Fruity Flanger and Fruity Phaser
- [ ] **Vocoder (Vocodex)**
- [ ] **Peak Controller.** Common for sidechain pumping in projects; nothing in the code handles it yet
- [ ] **Formula Controller**
- [ ] **Patcher.** The mixer slot menu's Patcherize and Patcherize all are greyed out
- [x] **Transient Processor, Chorus and Stereo Shaper compared with FL Studio 2026's exports.** Six of the seven
  small effects match sample for sample in the tests (98 cases for these three: `os-engine/tests/fx_null.rs`); the
  Chorus uses the true pi where the original's is a hair off, so 10 of its 21 cases match and the rest are at least
  37 dB below the test signal

## Gaps and polish

Greyed-out controls and partial features. These can land alongside the blockers.

- [ ] **Event editor window.** Edit events is greyed out in five menus (playlist, channel rack, parameter menus)
- [ ] **Undo for a plugin editor's own changes**
- [x] **Undo for mixer track commands.** Routing, names, colours, resets, effect slots, audio input / output and
  latency settings, adding, moving, deleting and trimming tracks. Mute and solo make no step, as in FL Studio
- [x] **Undo for the remaining edits.** A generator inserted higher up the rack, Replace, project info and the
  timebase are each one step
- [ ] **Mixer right-click menus.** 41 entries greyed out on 2026-10-29: the routing menu, groups, Render to wave
  file, spare states, smart disable ([mixer.rs](../crates/os-app/src/mixer.rs))
- [ ] **Playlist right-click menus.** 36 entries greyed out on 2026-10-29: key markers, Add two..., quantized song
  jumps, the Start / Stop recording markers' punch ([playlist.rs](../crates/os-app/src/playlist.rs))
- [ ] **Channel rack right-click menus.** 25 entries greyed out on 2026-10-29: Type in value, Burn selected loops to
  pattern, instrument tracks, Advanced looping ([rack.rs](../crates/os-app/src/rack.rs))
- [ ] **Main menu bar.** 19 of its 136 entries greyed out on 2026-10-29: 10 in TOOLS, 5 in VIEW, 4 in HELP
  ([menus.rs](../crates/os-app/src/views/menus.rs))
- [x] **TOOLS ▸ Audio logger and Dump to audio editor.** Enabled (on by default, as in FL Studio) keeps the last 1 to
  10 minutes of what the device plays, the header showing the memory used; Dump puts them into the master track's
  Tesla. At its time limit Tesla now keeps the last minutes, as Edison does. What FL Studio's logger records and how
  long was not measured: Lemon Studio's design
- [ ] **Tesla's later tools.** Envelopes, equalize, blur and convolution reverb, clean-up, time stretch and pitch
  shift, slicing and beat detection (group 5 of Tesla's roadmap in the specs)
- [x] **Value boxes.** All of them work, the last two being the autosave Maximum and Metronome track. MIDI settings'
  Performance mode MIDI channel stays greyed out until Performance mode is built
- [ ] **Performance mode.** Live clip launching on the playlist (Ctrl+P says it is not available yet), with MIDI
  settings' Performance mode MIDI channel and TOOLS ▸ Macros ▸ Prepare for performance mode
- [ ] **Recording settings page.** The only settings page that doesn't work yet
- [x] **Editable mixer latency settings.** The input clock, the track latency combo and the strip's latency clock
  (right click) set them in ms, samples or beats, one undo step each
- [x] **Browser: Favorites, the plugin database and Browser find.** A Favorite check in the row menus of files and
  plugins, starred rows and the Favorites tab; the Plugin database by category, and Installed by format; search finds
  files in closed folders through a folder index (Enable indexing, Re-index, Expand folders). Tags and the "New"
  folder come later
- [ ] **JACK on Linux**
- [x] **ASIO output pairs.** Every track has an output menu with the pairs of the driver's first 16 outputs, and the
  master plays on the pair chosen for it (#182)

## Shipping a build

What has to exist before anyone outside can run the alpha. Builds ship once the rest of this checklist is done
(decided 2026-10-29), so these come last: until then nothing is packaged or released, and contributions that add
installers or release packaging are declined.

- [ ] **Prebuilt binaries for Windows, macOS and Linux.** Today Lemon Studio is built from source only
- [x] **Complete [THIRD-PARTY-NOTICES.txt](../THIRD-PARTY-NOTICES.txt).** Every crate that goes into a build is
  listed with its licence text, generated by `cargo run -p notices`; `cargo test` fails when one is missing
- [ ] **macOS signing and notarization.** Builds and plugin bundles are signed ad hoc only
- [ ] **Plugin crashes contained on macOS and Linux.** Windows only today; it matters once third-party plugins are in
  play
