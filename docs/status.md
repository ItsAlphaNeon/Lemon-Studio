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

## ✅ Piano roll

- Full tool set, with velocity and event lanes
- Quantize, chop, glue, legato, strum, arpeggiate and randomize
- Vertical zoom and ghost channels
- Resizable event panel: drag the splitter, double-click it to hide or show the panel (remembered)
- Pattern time markers, saved with the project

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

- CLAP and VST3 instruments and effects, with their editors and saved state
- On Windows, a plugin that crashes is stopped (its slot passes the sound through) instead of closing
  Lemon Studio
- Not yet: VST2

## ✅ Plugin management

- Plugin menus everywhere a plugin is added. F8 lists instruments and effects: an effect goes to the
  selected mixer track, an instrument to the bottom of the channel rack.
- A plugin manager in Settings with your own search folders
- A background scan at startup; each file is tried in a separate scanner process, so a broken plugin
  cannot take Lemon Studio down
- Favourites and hidden plugins
- One menu entry per plugin installed as both VST3 and CLAP (your preferred format wins)

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
- Still early: always work on copies and keep backups

## 🚧 Built-in instruments and effects

- Lemon Sampler, Lemon 3x Osc (a three-oscillator synth) and Lemon Parametric EQ 2, also available as VST3
  and CLAP bundles for other hosts
- The sampler plays a project's sample settings (start and length, the sample file's loop, the sample
  effects, the channel filter, declicking) and each note's own pan and filter offsets

## 🚧 Exporting audio

- Song or pattern to a WAV file (32-bit float)
- Not yet: MP3 or FLAC

## 🚧 Note input

- Mouse, computer keyboard and MIDI controllers; notes play the selected channel
- Not yet: note recording

## 🚧 Undo

- Per editor (piano roll, playlist)
- Not yet: redo, global history

## 🚧 Audio drivers

- WASAPI (shared mode) on Windows
- ALSA (and PipeWire / PulseAudio through it) on Linux
- Core Audio on macOS
- Output device, sample rate and buffer length in Settings > Audio
- Not yet: ASIO, JACK

## 🚧 Platforms

- Windows 10 / 11
- Linux: X11, and Wayland through XWayland (see the [Linux notes](../README.md#linux))
- macOS 11 or newer, supported at the same level as Linux (see the [macOS notes](../README.md#macos))

## 🚧 Look and themes

- The Lemon design (lemon on neutral greys, rounded-square controls) and the Classic look, switched with
  Ctrl+Shift+D
- Theme files apply to the Classic look
- Not yet: restyled plugin editors

## ❌ Not available yet

- Audio recording
- MIDI file import / export
- Audio editing
- Plugin delay compensation
