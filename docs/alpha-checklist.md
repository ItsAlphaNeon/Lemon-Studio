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
- [ ] **Patcher.** Slice 1 on 2026-10-30: the mixer slot menu's Patcherize and Patcherize all (Wrap in container,
  Wrap all in container) put the slot's effect, or all of the track's, into a container that plays them in series,
  each with its own wet and dry, with their delays made up and one undo step. The container keeps the slot's name,
  colour and icon. Its window (the slot menu's Container ▸ Edit list..., or opening the slot) opens each effect's
  plugin window, turns and types in its wet and dry levels on knobs that follow automation, and removes, moves and
  adds effects, one undo step each
  ([container.rs](../crates/os-app/src/container.rs), [chain.rs](../crates/os-engine/src/chain.rs)). Containers are
  saved in Lemon Studio's projects. The effects inside are automated and linked as a slot's effect is, their wet and
  dry levels too (clips and controller links name the member by its container's slot and its id, so they follow it
  when it moves and are saved with the project), and their windows have their presets, a slot effect's menu
  (Replace, Delete) and undo steps for their edits and presets (2026-10-30). Left: the original's Patcher slots in
  imported projects, moving a wrapped slot's own clips and links onto its member, the CPU switches per effect, the
  channel's Patcherize (an instrument container, the Patcher's slice 3), and the graph editor
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
- [ ] **Mixer right-click menus.** 4 entries greyed out on 2026-10-30, counted entry by entry after Insert from state
  (and File ▸ Open / Save mixer track state), Plugin delay compensation ▸ Automatic, View ▸ Waveforms and Extra
  properties, the slot menu's Scaling ▸ and Help, Mixer options ▸ Link all parameters... (the current track's own
  controls), the track and slot menus' Change icon..., the slot menu's Link all parameters... and its Patcherize and
  Patcherize all (Wrap in container, Wrap all in container: see Patcher) and Mixer options ▸ Detached (the mixer in a
  native window of its own) were enabled:
  Assign to / Unassign from Audio track(s) and Select ▸ Channels and playlist tracks (no audio tracks in the playlist
  yet), Compensate for master latency (greyed in the original too).
  Scaling ▸ is greyed for a slot without a hosted plugin, "(no saved states)" when none is saved. Not counted: the
  layout combo's seven other layouts ([mixer.rs](../crates/os-app/src/mixer.rs))
- [x] **Playlist right-click menus.** None greyed out since 2026-10-30: the last five, the picker's Change icon and
  the plugin window's Link all parameters..., Change icon..., Save channel state as... and Browse channel states, work
  as Lemon Studio's design (Change icon is kept for the session only)
  ([playlist.rs](../crates/os-app/src/playlist.rs), [channel_states.rs](../crates/os-app/src/channel_states.rs))
- [ ] **Channel rack right-click menus.** 10 of the rack's own entries greyed out on 2026-10-30, counted entry by
  entry after 25 more were enabled (Receive notes from ▸ Unlock and Computer keyboard, Switch auto-bypass for all
  plugins, Change icon ▸, Show waveform preview, MIDI channel through, Restretch all and Detached on 2026-10-30): Loop
  starter and its 3 playlist-track options (it is built on the original's online loop library), Patcherize, Assign
  to new instrument track (instrument tracks, and how the original saves a track's link to its channel, not measured
  yet), Auto fill (greyed on every channel tried in the original; what it does is unknown), Create instrument (no multi-sample instrument to put the samples
  in yet), Burn MIDI to ▸ (2) (no plugin hands its notes back to the host yet). The knob, mixer box and LED menus'
  Automation and Remote control groups add 18 more, after 5 were enabled on 2026-10-30 (Edit automation in piano roll
  on the volume and panning knobs, the swing knob's Set as song start value, MIDI learn... on the mixer box and the
  LED, Edit automation in a window on the volume and panning knobs: a second event editor window): Override global
  mapping... on all four (one kind of link, kept in the program's settings; what the original's override does is
  not described), and on the swing knob, the mixer box and the LED Edit automation, Edit automation in a window,
  Create automation clip and Edit automation in piano roll (the engine plays no automation of them, and the mixer box's and the LED's
  targets in the original's files are not measured), Set as song start value on the mixer box and the LED (the same)
  ([rack.rs](../crates/os-app/src/rack.rs))
- [ ] **Main menu bar.** 2 of its 137 entries greyed out on 2026-10-30, counted entry by entry after Save audio
  clipboard to browser, VIEW > Remote control, HELP > Tutorials ▸, Diagnostics, TOOLS > Set up for live performance,
  Switch auto-bypass for all plugins, Chord progression tool..., Set keyboard focus of all plugins to ▸ (its Enabled
  and Disabled not counted; greyed on Linux), Lock / Unlock all tracks to / from their clips (the last four as Lemon
  Studio's design) and Set up for MIDI export (with the new ADD ▸ MIDI Out, Lemon MIDI Out) were enabled: TOOLS >
  Control builder (no control builder yet); VIEW > Script output (no scripting)
  ([menus.rs](../crates/os-app/src/views/menus.rs))
- [x] **TOOLS ▸ Audio logger and Dump to audio editor.** Enabled (on by default, as in FL Studio) keeps the last 1 to
  10 minutes of what the device plays, the header showing the memory used; Dump puts them into the master track's
  Tesla. At its time limit Tesla now keeps the last minutes, as Edison does. What FL Studio's logger records and how
  long was not measured: Lemon Studio's design
- [x] **Tesla's later tools** (group 5 of Tesla's roadmap in the specs). Time stretch / pitch shift with formant
  preservation (2026-10-29); the envelope modes with Mixdown amp envelopes, heard while playing, the envelope tools
  (with their dialogs: Scale levels, Decimate points, Filter, Smooth up, Create sequence; Turn all points smooth and
  per-point curves), Paste mix (envelope) and Scratch (envelope), Convolution reverb, Equalize, Blur, Clean up
  (denoise with an acquired noise profile, the declipper with its modes and Safe mode, the declicker), auto-slicing /
  grid-slicing, Detect beats, Detect pitch regions, Set first downbeat, Convert to score and dump to piano roll and
  the zero-cross check, Send to new Lemon Slicer channel, drum(loop) stretch and noise gating (Acquire noise
  threshold, Gate noise, Trim side noise, Trim all noise & slice up, the threshold mark) work (2026-10-30). Most of
  it is Lemon Studio's design, with measurement requests open in the specs. Vocal denoise / isolator and Deverb have
  their own item below
- [x] **Tesla's Vocal denoise / isolator and Deverb.** Tools > Spectral's last two entries (group 5 of Tesla's
  roadmap in the specs) work (2026-10-30): Vocal denoise / isolator's MIX goes from the noise alone through the
  original to the voice alone, Deverb takes the late reverb out (General or Vocal, with a MIX) at a reverb time it
  reads from the selection. Both are Lemon Studio's spectral design, not the original's trained model; what the
  entries do is from the original's manual. Edit > Undo mix (envelope) came with them
- [x] **Value boxes.** All of them work, the last two being the autosave Maximum and Metronome track, and MIDI
  settings' Performance mode MIDI channel (2026-10-30)
- [x] **Performance mode.** Launching the zone's clips with the mouse, with the typing keyboard (the original's
  Performance mode layout, measured: four rows of five keys a hand for tracks 1–8) and with notes 0–119 on
  the Performance mode MIDI channel (scrolled and paged with notes 120–124, synced to the global snap while 127 is
  held), column, clip group and track group scenes and +Scenes (Ctrl+click, notes 125 / 126 held), sub-clips (velocity), each track's Performance
  settings, Ctrl+P, Prepare for performance mode, the clip and track progress displays, the clock counting bars
  from the Start marker and recording a performance (Record armed) work (2026-10-30; the typing keyboard last)
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
- [x] **JACK on Linux.** Settings > Audio ▸ Mode ▸ JACK plays as a client of the JACK server (cpal's JACK host, libjack
  loaded at run time; building needs the JACK development files). Not yet tried against a running JACK server
  (2026-10-30)
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
- [ ] **Plugin crashes contained on macOS and Linux.** Calls into plugins run under a signal guard there, with the
  same handling of a stopped plugin as on Windows (its crash tests pass in CI on both), and so do CLAP and VST3
  editors' run loop callbacks on Linux. Left: macOS editors, which the system's run loop calls outside any host
  call: a crash in a plugin's editor on macOS still closes Lemon Studio
