# Feature status

The details behind the [Capabilities](../README.md#capabilities) table, area by area. Controls for anything
that is not implemented yet are greyed out in the application: hovering one says "not available yet", and
clicking it does nothing. The full, item-by-item roadmap lives on the
[Discord server](https://discord.gg/yC2uCuGs3M).

✅ usable · 🚧 partial · ❌ not available yet

## ✅ Step sequencing and patterns

- Channel rack with steps and mini piano rolls
- Swing and per-channel loops
- Select unused channels and Purge unused audio clips (TOOLS menu and the rack's menu; purge asks first)
- Deleting channels or a pattern asks first, unless the box's Remember my choice was ticked or the question
  is turned off in General settings ▸ Hints and warnings ▸ Manage messages

## ✅ Piano roll

- Full tool set, with velocity and event lanes
- Quantize, chop, glue, legato, strum, arpeggiate and randomize
- Vertical zoom and ghost channels
- Resizable event panel: drag the splitter, double-click it to hide or show the panel (remembered)
- Pattern time markers, saved with the project
- Step edit (toolbar, OPTIONS menu, Ctrl+E): with the transport stopped, typing-keyboard and MIDI notes are
  written at the song position a chord at a time, one main-snap step long, and the position moves on a step; in
  SONG mode they go where the current pattern plays at the song position

## ✅ Song arrangement

- Playlist with pattern, audio and automation clips
- Clip groups, markers and several arrangements
- Enable groups (toolbar, OPTIONS menu, Shift+Alt+G): turned off, grouped clips select and move one by one
- Track height, zoom and scroll saved with the project
- Song position slider in the toolbar: drag, click to step, right-click to jump, wheel; it follows playback
  (through the pattern in PAT mode)
- Auto scrolling (toolbar, OPTIONS menu): while playing, the playlist follows the playhead (the piano roll in
  pattern mode), continuously or a quarter page at a time (right-click on the button)

## ✅ Mixing and routing

- Mixer tracks, sends, effect slots and meters
- Polarity and stereo swap, main volume and main pitch
- Not yet: sidechain inputs

## ✅ Plugin hosting

- CLAP, VST3 and VST2 instruments and effects, with their editors and saved state
- VST2 shells (one file offering several plugins); a VST2 plugin's state is saved in the layout FL Studio's projects use
- On Windows, a plugin that crashes is stopped (its slot passes the sound through) instead of closing
  Lemon Studio
- Not yet: 32-bit plugins, and Intel-only plugins on Apple silicon (they need a bridge)

## ✅ Plugin management

- Plugin menus everywhere a plugin is added. F8 lists instruments and effects: an effect goes to the
  selected mixer track, an instrument to the bottom of the channel rack.
- A plugin manager in Settings with your own search folders
- A background scan at startup; each file is tried in a separate scanner process, so a broken plugin
  cannot take Lemon Studio down
- Favourites and hidden plugins
- One menu entry per plugin installed in several formats (VST3, CLAP, VST2: your preferred format wins)

## ✅ Time-stretching

- All twelve stretch methods of sampler channels and audio clips, with the length and pitch rules
  projects expect
- Audio clips in the playlist take their length (and redraw their waveform) whenever the sampler's
  stretch settings, the channel pitch or the tempo change
- Playlist stretch mode (Shift+M)
- One open-source stretcher (Signalsmith Stretch) plays every method, so the sound differs from the
  original stretchers

## 🚧 Playback engine

- Renders whole ticks ahead of the audio device, so edits and busy moments do not leave gaps
- A plugin the interface is using is waited for briefly instead of skipped, and the interface only touches
  a plugin when it has work for it
- Cut groups, voice limits, mono and portamento glides
- Slide notes (silent; they glide the channel's sounding notes)
- Layer channels play their children
- The channel's level adjustments (volume multiplier, pan and mod X / Y offsets) and each note's own pan
  and mod X / Y reach the instrument
- Audio clips play at the sample's own level
- Audio clip fades follow the measured fade curve. Automatic crossfades (Project settings) fade overlapping
  audio clips on a track into each other and follow moves, the tempo and undo; the playlist draws each one
  as an X with a handle that bends both fades. Lemon Sampler gives each clip its own fades, so two clips of
  one sample crossfade flat; another instrument on an audio clip channel shares one gain between overlapping
  clips, so their crossfade dips
- MP3 samples are placed as the original encoder output (the info frame plays as silence)
- The sampler's closing fades
- A fast resampler while playing, and the exact one for export
- Idle plugins are skipped; Stop ends every voice, also in hosted plugins
- Not yet: multi-core mixing (there is one audio thread)

**CPU panel.** Shows the audio load, the voices playing (the engine's and every plugin's), memory and
dropouts. Its right-click menu has refresh rates, a frames-per-second view and handle / thread / font
counts. Double-clicking it opens the plugin performance monitor: each plugin's processing time, peak, total
and voices, with sorting, find and freeze.

**Metronome.** The toolbar button, OPTIONS > Metronome or Ctrl+M: a click on every quarter note while playing,
accented as FL does (the first beat of each bar in 4/4 and 3/4), through the master track. Right-click the
button for five sounds. It is never in an export, and it is remembered between sessions. Not yet: the
"Metronome track" setting, and following time-signature markers (it follows the project's signature).

**Tempo tapper.** VIEW > Tempo tapper, Ctrl+F5 or the kick drum button on the toolbar: click the pad (or press
a key on a MIDI keyboard) in time. The average of the taps, in whole BPM, becomes the project tempo while Apply
tempo is on; the beat bar counts the taps in groups of four. Each tap clicks with the metronome's Tick sound,
the first of each four accented. Apply tempo is remembered between sessions. Not yet: nudge, Sync, Mute and now
(drawn greyed out).

**Closing windows.** VIEW > Close all windows (F12), Close all unfocused windows (Ctrl+F12: all but the one
in front) and Close all plugin windows (Alt+F12: channel settings and plugin editors); the browser stays.
Not yet: Align all channel editors, Arrange windows.

## 🚧 Automation

- Automation clips for channel, mixer and plugin parameters
- A clip drives every target it is linked to, including mixer slot switches and mix levels, track EQ, send
  levels, stereo separation and the channel's level adjustments
- All thirteen curve shapes are walked exactly; values are taken one tick ahead and passed on only when
  they change
- Not yet: recording of movements, clip LFO, min / range

## 🚧 Project files

- Opens and saves `.flp` projects saved by FL Studio 24.2 and later, 25 and 26 included
- Anything in a file that Lemon Studio does not understand yet is kept and written back unchanged
- A project in the layout used before version 24.2 is refused with a dialog that explains how to re-save
  it; support for older versions is on the roadmap
- An opened project's samples load behind a progress panel, one file at a time or on up to 4 threads
  (General settings ▸ Advanced ▸ Threaded project sample loading)
- A save that writes a file over General settings ▸ Hints and warnings ▸ Project data size warning (100 MB
  unless changed) says how big the file is
- Data folders (Project settings): a project's own folder for its backups and copied samples, Copy used
  samples, Rename (the folder and the file), and the browser's Project data. The first save of a new project
  opens a Save as form that can make that folder (General ▸ Ask for data folder decides when; File ▸ New can
  open a New project form)
- Still early: always work on copies and keep backups

## 🚧 Built-in instruments and effects

- Lemon Sampler, Lemon Trio (a three-oscillator synth) and Lemon Parametric EQ, also available as VST3
  and CLAP bundles for other hosts
- The sampler plays a project's sample settings (start and length, the sample file's loop, the sample
  effects, the channel filter, declicking) and each note's own pan and filter offsets
- A file of 30 s or more loaded into a sampler channel turns its Keep on disk on (General settings ▸
  Advanced ▸ Auto keep long audio on disk)

## 🚧 Exporting audio

- Song or pattern to a WAV file (32-bit float)
- Not yet: MP3 or FLAC

## 🚧 Note input

- Mouse, computer keyboard and MIDI controllers; notes play the selected channel
- Not yet: note recording

## 🚧 Undo

- One history for the whole project, with redo: Ctrl+Z, Ctrl+Alt+Z and the EDIT menu, which names the step
- Covers the piano roll, the playlist, step-sequencer edits and knob turns (channel, main, mixer panning and
  stereo separation, effect mix, send levels, swing), sample loads, the project's initial values, PATTERNS ▸ Split
  by channel and ADD's audio and automation clip channels
- The browser's Current project ▸ History lists the steps from "Last reset"; undone steps are grey, and a click
  on one goes back or forward to it
- General settings ▸ Undo history: Maximum undo levels, Undo knob tweaks, Alternate undo mode, and showing the
  history in the browser, newest first if wanted
- Not yet: mixer faders and plugin parameters, other ways of adding or deleting channels, project settings;
  "Put undone recordings in the recycle bin" waits for recording

## 🚧 Audio drivers

- WASAPI on Windows, in shared mode (with the shorter periods a device offers there, where Windows says) or
  exclusive mode (Settings > Audio ▸ Mode: Lemon Studio alone on the device, at its own rate and down to its smallest
  period); a device that refuses exclusive mode plays in shared mode, and the page says why
- ASIO® on Windows, output only: the installed drivers are in Settings > Audio's Output device list; the master plays
  to the driver's first two outputs, and Show ASIO panel opens the driver's own settings
- ALSA (and PipeWire / PulseAudio through it) on Linux
- Core Audio on macOS
- Output device, sample rate and buffer length in Settings > Audio
- Auto close (Settings > Audio): the output closes while another program is in front, and a playing song pauses
  until Lemon Studio is back in front
- A device that fails (unplugged, or a driver asking to be reset) is opened again; an ASIO driver that does not
  open gives way to the default device
- Not yet: ASIO inputs and output pairs, JACK

<img alt="ASIO Compatible logo" src="../crates/os-app/asiosdk/logo/ASIO-compatible-logo-Steinberg-TM-BW.png" width="96">

ASIO is a registered trademark of Steinberg Media Technologies GmbH.

## 🚧 Platforms

- Windows 10 / 11
- Linux: X11, and Wayland through XWayland (see the [Linux notes](../README.md#linux))
- macOS 11 or newer, supported at the same level as Linux (see the [macOS notes](../README.md#macos))

## 🚧 Look and themes

- The Lemon design (lemon on neutral greys, rounded-square controls) and the Classic look, switched with
  Ctrl+Shift+D
- Theme files apply to the Classic look
- Lemon Studio's own plugin editors are drawn in the Lemon design only, whatever design the app shows
- Smooth scrolling: the wheel and the scroll bars' arrows ease the editors' views (General settings ▸ Display ▸
  Smoothing, and Animation refresh rate for how often animations redraw)

## 🚧 Settings

- The General, Audio, File, Theme, Project and Info pages work: the controls that are not greyed out act, but for
  a few that are only kept for now (Smart disable, Resampling quality and others Lemon Studio has nothing to change
  with yet); the program's choices are kept in `settings.txt` in the user folder
- General ▸ Input ▸ Manage shortcuts: the F8 keys (browser, plugin picker, plugin database) and the audio clip
  double-click can be swapped
- File ▸ Browser extra search folders: the folders added to the browser, renamed or replaced from here
- General ▸ Display ▸ High visibility: a white record dot, brighter channel names and light scroll bar thumbs
- General ▸ Display ▸ Toolbars scaling: the main window's top band at its own size; panels that don't fit
  leave an arrow
- General ▸ System ▸ Associate project files (Windows): `.flp` files open with Lemon Studio, for your user
  only, and unticking puts the previous program back
- Not yet: the MIDI page, the recording settings and the theme adjustments (the themes are being redone)

## ❌ Not available yet

- Audio recording
- MIDI file import / export
- Audio editing
- Plugin delay compensation
