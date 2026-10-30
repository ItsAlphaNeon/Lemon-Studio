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
- [x] **Automation recording into clips.** Every automatable control records into the pattern, as in the original,
  and patterns play those points; the event editor's Turn into automation clip makes a clip of them
- [x] **Automation clip LFO and min / range.** The clip channel's MIN / MAX knobs set its range and its LFO
  (switch, mode, amount, speed, tension, skew, pulse width, its own envelope) plays as the original's, in the clip's
  channel window; files keep every LFO field
- [x] **Projects saved before FL Studio 24.2.** Projects from FL Studio 10 to 24.1 (and the 24.2 betas) open: they
  are converted to the current layout as they open, as the original does, and save in that layout. Not yet: wrapped
  Fruity effects stay wrapped instead of becoming the native plugins, and a 3x Osc saved before FL Studio 21 has
  its envelope and filter settings at their defaults
- [x] **Multi-core mixing.** The engine renders on one audio thread, which limits dense projects
- [ ] **Engine rework (sampler voice, automation).** The README's roadmap names it as the next big step; land it
  before the alpha rather than after

## Built-in instruments and effects

Without third-party plugins, the built-in ones are the whole sound palette. Lemon Studio has three instruments,
sixteen effects and the Tesla audio editor; these are the gaps a project made in FL Studio runs into most.

- [x] **Drum sampler (FPC).** A pad-based drum instrument. Lemon Drum Sampler plays projects' FPC channels with their
  pads and samples (layers by velocity, the original's level, pan and tune laws, cut groups) and saves them back
  unchanged
- [x] **Slicer (Slicex, Fruity Slicer).** A loop chopped into slices that the piano roll plays. Lemon Slicer
  recreates Fruity Slicer: it slices a loop by its markers, by beats or by transients, plays slice n on C5 + n, dumps
  the slices to the piano roll, and opens and saves a project's Fruity Slicer channels so the original still opens
  them. Slicex is not done: its channels (28 in 16 of the corpus projects, states of about 27 KB) need their own study
- [x] **Filter effect (Fruity Filter, Fruity Love Philter).** Lemon Filter and Pulp Filter, each also playing the
  original's slots in opened projects
- [x] **Flanger and phaser.** Lemon Flanger and Lemon Phaser recreate Fruity Flanger and Fruity Phaser
- [x] **Vocoder (Vocodex).** Lemon Vocoder plays projects' Vocodex slots, with the carrier or the voice from another
  track through its sidechain input; the original's built-in synth carrier isn't played yet
- [x] **Peak Controller.** Common for sidechain pumping in projects. Lemon Peak Controller plays projects' Fruity Peak
  Controller slots and the engine plays their controller links (mixer and plugin targets, formulas)
- [x] **Formula Controller.** Lemon Formula Controller plays projects' Fruity Formula Controller slots, with the
  original's formula language, and their controller links drive linked controls as the Peak Controller's do
- [ ] **Patcher.** The mixer slot menu's Patcherize and Patcherize all are greyed out
- [x] **Transient Processor, Chorus and Stereo Shaper compared with FL Studio 2026's exports.** Six of the seven
  small effects match sample for sample in the tests (98 cases for these three: `os-engine/tests/integration/fx_null.rs`); the
  Chorus uses the true pi where the original's is a hair off, so 10 of its 21 cases match and the rest are at least
  37 dB below the test signal

## Gaps and polish

Greyed-out controls and partial features. These can land alongside the blockers.

- [x] **Event editor window.** Edit events opens it from the parameter, channel rack and knob menus: a control's
  events in the pattern, drawn, erased, selected and deleted, and turned into an automation clip
- [x] **Undo for a plugin editor's own changes.** Parameters moved in a plugin's window and presets loaded from its
  title bar or dropped on it are one step each; a preset's undo brings back the state, the preset and the name
- [x] **Undo for mixer track commands.** Routing, names, colours, resets, effect slots, audio input / output and
  latency settings, adding, moving, deleting and trimming tracks. Mute and solo make no step, as in FL Studio
- [x] **Undo for the remaining edits.** A generator inserted higher up the rack, Replace, project info and the
  timebase are each one step
- [ ] **Mixer right-click menus.** 17 entries greyed out on 2026-10-30, counted entry by entry after the track menu's
  Group section (Separator, Create group..., Auto color group), Select ▸ Group, Dock to ▸ Left / Middle / Right and
  Allow threaded processing, the slot menu's Allow threaded processing and Auto-bypass when silent and the options'
  Switch auto-bypass for all plugins were enabled: track menu Change icon, Assign to / Unassign from Audio track(s), Select ▸
  Channels and playlist tracks, Compensate for master latency (greyed in the original too), Insert from state; Mixer
  options Plugin delay compensation ▸ Automatic, Link all parameters..., View ▸ Waveforms and Extra
  properties, Detached; slot menu Patcherize, Patcherize all, Link all parameters..., Change icon..., Scaling ▸ 100%,
  Help. Not counted: the
  layout combo's seven other layouts ([mixer.rs](../crates/os-app/src/mixer.rs))
- [x] **Playlist right-click menus.** None greyed out since 2026-10-30: the last five, the picker's Change icon and
  the plugin window's Link all parameters..., Change icon..., Save channel state as... and Browse channel states, work
  as Lemon Studio's design (Change icon is kept for the session only)
  ([playlist.rs](../crates/os-app/src/playlist.rs), [channel_states.rs](../crates/os-app/src/channel_states.rs))
- [ ] **Channel rack right-click menus.** 15 of the rack's own entries greyed out on 2026-10-30, counted entry by
  entry after 20 more were enabled (Receive notes from ▸ Unlock and Computer keyboard, Switch auto-bypass for all
  plugins on 2026-10-30): Loop starter and its 3 playlist-track options, Restretch all, Detached, Change icon, Show
  waveform preview, Patcherize, Assign to new instrument track, Auto fill, MIDI channel through, Create instrument,
  Burn MIDI to ▸ (2). The knob, mixer box and LED menus' Automation and Remote control groups add 23 more (events in a new
  window or the piano roll, initial values, automation clips, controller links) ([rack.rs](../crates/os-app/src/rack.rs))
- [ ] **Main menu bar.** 7 of its 136 entries greyed out on 2026-10-30, counted entry by entry after Save audio
  clipboard to browser, VIEW > Remote control, HELP > Tutorials ▸, Diagnostics, TOOLS > Set up for live performance
  and Switch auto-bypass for all plugins were enabled: TOOLS > Chord progression tool... and Control builder (no
  chord generator or control builder yet), Set up for MIDI export (not observed yet), Set keyboard focus of all
  plugins to ▸ (no such per-plugin setting yet), Lock / Unlock all tracks to / from their clips (tracks
  don't lock to a clip yet); VIEW > Script output (no scripting) ([menus.rs](../crates/os-app/src/views/menus.rs))
- [x] **TOOLS ▸ Audio logger and Dump to audio editor.** Enabled (on by default, as in FL Studio) keeps the last 1 to
  10 minutes of what the device plays, the header showing the memory used; Dump puts them into the master track's
  Tesla. At its time limit Tesla now keeps the last minutes, as Edison does. What FL Studio's logger records and how
  long was not measured: Lemon Studio's design
- [ ] **Tesla's later tools.** Set first downbeat, Turn all points smooth and Convert to score (group 5 of Tesla's roadmap in the specs).
  Time stretch / pitch shift with formant preservation works (2026-10-29); the envelope modes with Mixdown amp
  envelopes, the envelope tools (with their dialogs: Scale levels, Decimate points, Filter, Smooth up, Create
  sequence), Convolution reverb, Equalize, Blur, Clean up (denoise with an acquired noise profile, the declipper with
  its modes and Safe mode, the declicker), auto-slicing / grid-slicing, Detect beats, Detect pitch regions and the
  zero-cross check, Send to new Lemon Slicer channel, drum(loop) stretch and noise gating (Acquire noise threshold,
  Gate noise, Trim side noise, Trim all noise & slice up, the threshold mark) work (2026-10-30)
- [x] **Value boxes.** All of them work, the last two being the autosave Maximum and Metronome track, and MIDI
  settings' Performance mode MIDI channel (2026-10-30)
- [ ] **Performance mode.** The typing keyboard, the MIDI channel's scene and snap keys (notes 125–127), scenes,
  recording a performance and the View menu's progress options. Launching the zone's clips with the mouse and with
  notes 0–119 on the Performance mode MIDI channel (scrolled and paged with notes 120–124), each track's Performance
  settings, Ctrl+P, Prepare for performance mode, the playing clip's progress and the clock counting bars from the
  Start marker work (2026-10-30)
- [x] **Recording settings page.** Settings ▸ Recording, Lemon Studio's own page (the original keeps these in the
  Record button's menus): the recording filter, the Record options and quantize, Start on input, Loop record, Blend,
  Count-in and its length, Auto-create audio clips and the External input question's answer, shared with the menus
  and buttons that also set them
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
