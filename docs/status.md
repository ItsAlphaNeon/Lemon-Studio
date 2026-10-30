# Feature status

The details behind the [Capabilities](../README.md#capabilities) table, area by area. Controls for anything
that is not implemented yet are greyed out in the application: hovering one says "not available yet", and
clicking it does nothing. The full, item-by-item roadmap lives on the
[Discord server](https://discord.gg/yC2uCuGs3M).

✅ usable · 🚧 partial · ❌ not available yet

## ✅ Step sequencing and patterns

- Channel rack with steps and mini piano rolls
- Swing and per-channel loops
- ADD ▸ Layer: a layer channel whose children are the channels selected in the rack
- Select unused channels and Purge unused audio clips (TOOLS menu and the rack's menu; purge asks first)
- Deleting channels or a pattern asks first, unless the box's Remember my choice was ticked or the question
  is turned off in General settings ▸ Hints and warnings ▸ Manage messages
- PATTERNS ▸ Quick render as audio clip: the current pattern rendered (as the export renders a pattern, with its
  progress panel) to a WAV in the data folder's `Rendered` folder and made a new audio clip channel named after it;
  Render and replace... also covers every clip of the pattern in the playlist with a clip of it and mutes the
  pattern clips. One undo step each. Render as audio clip... opens the Export form on the pattern first (folder,
  name, format and the other choices), then makes the channel the same way

## ✅ Piano roll

- Full tool set, with velocity and event lanes
- Quantize (FL's templates and Snap grids, sensitivity, four duration modes, levels), chop (FL's patterns, tied rows, levels, groups), glue, legato, strum,
  arpeggiate, flam, claw machine (trash, stretch, time distribution), flip, scale levels, limit (key / scale, range,
  transpose), LFO (sine, triangle or square into any event lane, Start to End ramps, a channel lane's control points)
  and randomize (a new melody from a key or scale, or random levels, each with its own seed)
- Vertical zoom and ghost channels
- Resizable event panel: drag the splitter, double-click it to hide or show the panel (remembered)
- Pattern time markers, saved with the project
- Step edit (toolbar, OPTIONS menu, Ctrl+E): with the transport stopped, typing-keyboard and MIDI notes are
  written at the song position a chord at a time, one main-snap step long, and the position moves on a step; in
  SONG mode they go where the current pattern plays at the song position

## ✅ Song arrangement

- Playlist with pattern, audio and automation clips
- Clip groups, markers and several arrangements
- Time markers with FL's actions (right-click a marker): Start, Loop, Marker loop, Marker skip and Marker pause
  play in SONG mode; exports start at the Start marker and jump at skips. Move content left / right, Place loop
  (Shift+T), Add auto, Add one every, Add jump to next bar, time signature markers, and Delete or Change to marker
  loop in the time selection. A click on a marker moves the song position there. Not yet: Add two..., key
  markers, quantized song jumps, the Start / Stop recording markers' punch
- Enable groups (toolbar, OPTIONS menu, Shift+Alt+G): turned off, grouped clips select and move one by one
- Track height, zoom and scroll saved with the project
- Project picker (VIEW, Ctrl+F8 or the toolbar's project button): a menu of the patterns and channels; picking a
  pattern makes it current, picking a channel opens its window
- Song position slider in the toolbar: drag, click to step, right-click to jump, wheel; it follows playback
  (through the pattern in PAT mode)
- Auto scrolling (toolbar, OPTIONS menu): while playing, the playlist follows the playhead (the piano roll in
  pattern mode), continuously or a quarter page at a time (right-click on the button)

## ✅ Mixing and routing

- Mixer tracks, sends, effect slots and meters
- Polarity and stereo swap, main volume and main pitch
- Sidechain inputs: a project's Fruity Limiter slot listens to the track routed into its track that its sidechain
  number names (Sidechain to this track routes count), heard after that track's fader and delayed to line up
- CLAP and VST3 effects get a sidechain input too: pick it in the slot menu's Sidechain input submenu, which lists
  the tracks routed into the track. The choice is saved where FL saves it, and projects saved by FL play their
  plugins' sidechain and input choices as FL plays them

## ✅ Plugin hosting

- CLAP, VST3 and VST2 instruments and effects, with their editors and saved state
- A plugin's right-click menu on a parameter offers "Create automation clip" (CLAP and VST3); a VST3 plugin that asks
  the host to show its menu gets one of Lemon Studio's own menus, docked or detached
- The parameter menu (a parameter in the browser after "Browse parameters", TOOLS ▸ Last tweaked): create or edit
  the parameter's automation clip, link a controller, copy, paste, type in or set a value (a submenu of its steps, or
  its default and minimum to maximum), remove an initial value read from a file
- Not yet in the parameter menu: Init song with this position on a plugin parameter
- VST2 shells (one file offering several plugins); a VST2 plugin's state is saved in the layout FL Studio's projects use
- On Windows, a plugin that crashes is stopped (its slot passes the sound through) instead of closing
  Lemon Studio
- Not yet: 32-bit plugins, and Intel-only plugins on Apple silicon (they need a bridge)

## 🚧 Plugin delay compensation

- The delay a plugin reports (CLAP, VST3, VST2 and FL Studio's native plugins), as an instrument or in a mixer
  slot, is made up on every other path through the mixer, sends and a slot's dry part included, so everything
  meets in time
- An export starts late by the whole delay, so the file shows none and keeps its length; live playback skips the
  same amount, and the playhead follows what is heard
- The mixer tracks' input and output latency settings are read from projects, count as FL Studio counts them, and
  are saved back
- Automation of a mixer track or slot behind late audio waits for it, as in FL Studio
- A sidechain input waits for the slot it feeds, so it lines up with the audio there
- The latency settings are edited from the input clock, the track latency combo and the strip's latency clock (right
  click): Reset, or a value typed in ms, samples or beats; the clocks turn orange when set

## ✅ Plugin management

- Plugin menus everywhere a plugin is added. F8 lists instruments and effects: an effect goes to the
  selected mixer track, an instrument to the bottom of the channel rack.
- A plugin manager in Settings with your own search folders
- A background scan at startup; each file is tried in a separate scanner process, so a broken plugin
  cannot take Lemon Studio down
- Favourites and hidden plugins
- The browser's Favorites: right-click a file or a plugin ▸ Favorite; favorite rows show a star, and the Favorites
  tab lists favorite plugins, then files. A plugin's favorite is its plugin-manager star, also toggled by "Add to
  plugin database (flag as favorite)" in plugin windows and mixer slots
- The browser's Plugin database: plugins by category, and by format under Installed; ADD ▸ Browse all installed
  plugins / Browse presets and the mixer's Plugin database entry open it
- Browser find (Ctrl+F): search finds files anywhere below the dropped folders and the data folder through a
  background index; Expand folders, the indexing options and Configure extra folders work, the options are kept
  between runs, and a × clears the search
- One menu entry per plugin installed in several formats (VST3, CLAP, VST2: your preferred format wins)
- Presets in the plugin window's title bar: your own presets, then factory presets (compiled in for Lemon Studio's
  own plugins), stepped with the arrows or the wheel; Save preset as..., Rename, Delete and Browse presets
- Import of `.ospreset`, `.vstpreset` and VST2 `.fxp` / `.fxb` files (a bank goes into a folder of its own); a preset
  file dropped onto a plugin window, rack row or mixer slot loads there, and anywhere else adds its plugin

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
- Multi-core mixing: the mixer's tracks, the generator channels and chunks of the sampler's voices render on a
  pool of worker threads (the physical cores less one), along the routing's order, bit for bit as on one thread.
  Audio ▸ CPU's two switches, Multithreaded mixer processing and Multithreaded generator processing, turn each
  part on or off; dense projects render about 4.5 times faster on 8 cores

**CPU panel.** Shows the audio load, the voices playing (the engine's and every plugin's), memory and
dropouts. Its right-click menu has refresh rates, a frames-per-second view and handle / thread / font
counts. Double-clicking it opens the plugin performance monitor: each plugin's processing time, peak, total
and voices, with sorting, find and freeze.

**Metronome.** The toolbar button, OPTIONS > Metronome or Ctrl+M: a click on every quarter note while playing,
accented as FL does (the first beat of each bar in 4/4 and 3/4), through the master track or the insert track that
Audio settings ▸ Mixer ▸ Metronome track picks (the count-in and the Tempo tapper's clicks too). Right-click the
button for five sounds and the volume (-12 to +12 dB; above 0 dB the click is soft-limited, not clipped). It is never in an export, and it is remembered between sessions. It follows the time-signature markers: the
arrangement's in SONG mode, the current pattern's in PAT mode, each marker starting a bar of its own beats.

**Tempo tapper.** VIEW > Tempo tapper, Ctrl+F5 or the kick drum button on the toolbar: click the pad (or press
a key on a MIDI keyboard, play a note on the typing keyboard, or press a Touch controller pad or key) in time. The average of the taps, in whole BPM, becomes the project tempo while Apply
tempo is on; the beat bar counts the taps in groups of four. Each tap clicks with the metronome's Tick sound,
the first of each four accented. Apply tempo is remembered between sessions. − nudge / nudge +, held while playing, run playback 2 % slower / faster
until let go (the tempo stays). Not yet: Sync, Mute and now (drawn greyed out), nudging with the arrow keys.

**Closing windows.** VIEW > Close all windows (F12), Close all unfocused windows (Ctrl+F12: all but the one
in front) and Close all plugin windows (Alt+F12: channel settings and plugin editors); the browser stays.
Not yet: Align all channel editors, Arrange windows.

**Window positions.** Where the windows were when Lemon Studio closed comes back at the next start: the main
window (its place and size, and whether it was maximized; brought back onto a screen if its screen is gone) and the
Playlist, Mixer, Piano roll (floating, maximized or collapsed), Channel rack, Plugin performance monitor, Tempo tapper
and Touch controller. Kept with the program's settings, not the project. Which windows are open is not kept.

## 🚧 Automation

- Automation clips for channel, mixer and plugin parameters
- A clip drives every target it is linked to, including mixer slot switches and mix levels, track EQ, send
  levels, stereo separation and the channel's level adjustments
- A pattern's control points drive mixer volumes, pans, separation, equalisers, slot switches and mix levels, the
  tempo and plugin parameters, besides the channel's own settings (sends and the main pitch not yet)
- All thirteen curve shapes are walked exactly; values are taken one tick ahead and passed on only when
  they change
- A clip's range (its channel's MIN and MAX knobs, minimum above maximum turning the curve upside down) and its
  LFO (on / off, multiply or add, amount, speed, tension, skew, pulse width, and its own envelope) shape what it
  hands on, in the original's arithmetic; the clip's channel window sets them, each change an undo step, and
  projects keep every LFO field
- The event editor (Edit events on a control's menu: channel volume, pan and pitch, mixer controls, the tempo, plugin
  parameters) shows the control's events in the selected pattern as its level over time, in the setting's own unit: left
  drag draws freehand on the global snap, right drag a straight line, Erase and Select tools, the wheel scrolls and
  Ctrl+wheel zooms; its Edit menu has Cut, Copy, Paste, Select all, Delete, Insert current controller value and Turn
  into automation clip, which moves the events into a new clip channel linked to the control, as the original does
  (the control's earlier value until the first event, then the events, a recorded ramp as one straight segment, ending
  on the last event). Each edit is an undo step
- Not yet in the event editor: the Paint, Interpolate and Zoom tools, Scale levels and LFO, inserting and deleting time,
  a second window

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
- FILE lists the ten projects last opened or saved (Alt+1 .. Alt+0), More ▸ the older ones, fifty in all
- File settings ▸ Backup: Autosave backs a changed project up at its interval into the user folder's
  `Projects/Backup` (or the data folder's `Backup`), keeping the newest Maximum (100 unless changed) per project;
  saving over a file keeps a copy of the old one there
- FILE ▸ Revert to last backup opens the project's newest backup (asking about unsaved changes first, as Open
  does); Save as template... keeps a copy in the user folder's `Templates`, and New from template ▸ starts an
  untitled project from one (or Empty); Save to new project folder... saves into a folder named after the project,
  which becomes its data folder; Delete unused files and save project (FILE and TOOLS) asks, then deletes the data
  folder's unused samples, recordings and renders and saves
- Still early: always work on copies and keep backups

## 🚧 Built-in instruments and effects

- Lemon Sampler, Lemon Trio (a three-oscillator synth) and Lemon Parametric EQ, also available as VST3
  and CLAP bundles for other hosts
- Citrus, a six-operator synthesizer (a recreation of Sytrus) with its editor: it plays every Sytrus channel of a
  project and Sytrus patches, keeping their state byte for byte; also a VST3 and CLAP plugin
- Lemon Limiter (Fruity Limiter: limiter, compressor and noise gate), Lemon Reverb (Fruity Reeverb 2) and Lemon
  Delay (Fruity Delay 3), also VST3 and CLAP plugins. A project's Fruity Reeverb 2, Fruity Delay 3 and Fruity Limiter
  slots play through the same sound (the Limiter's compressor, gate, look-ahead and sidechain included)
- Every Lemon plugin is built into Lemon Studio, so none needs a plugin file; the VST3 and CLAP bundles are for other
  hosts. A project stores the built-in effects other than the EQ as wrapped CLAP plugins, as it stored their bundles
- Lemon Soft Clipper, Lemon Fast Distortion, Lemon Balance, Lemon Stereo Enhancer, Lemon Transient Processor, Lemon
  Chorus and Lemon Stereo Shaper as VST3 and CLAP plugins; a project's Fruity Soft Clipper, Fruity Fast Dist, Fruity
  Balance, Fruity Stereo Enhancer, Transient Processor, Fruity Chorus and Fruity Stereo Shaper play through the same
  sound in the mixer. Six match FL Studio 2026's exports sample for sample in the tests; the Chorus, with the true pi
  where the original's is a hair off, in 10 of 21 cases and the rest at least 37 dB below the test signal (once each
  export's LFO start phases are known: the original draws them anew for every render)
- Lemon Filter (Fruity Filter: a resonant low, band and high pass mix, X 2 and the DC remover) as a VST3 and CLAP
  plugin, with its response drawn in the editor; a project's Fruity Filter slots play through the same sound, which
  matches FL Studio 2026's exports in the tests (10 of 13 cases sample for sample, the rest below -180 dB)
- Pulp Filter (Fruity Love Philter: a pattern-controlled multi-filter) and Press (Maximus: a multiband maximiser) as
  VST3 and CLAP plugins; a project's Fruity Love Philter and Maximus slots play and open in them, and are saved back
  as the original effects with their state
- Lemon Flanger and Lemon Phaser as VST3 and CLAP plugins, each with the original's 27 presets as factory presets; a
  project's Fruity Flanger and Fruity Phaser play through the same sound in the mixer. Their sine waves use the true
  pi, where the original's is a hair off: the flanger still matches FL Studio 2026's exports sample for sample in the
  tests (36 cases), the phaser in 11 of 35 and the rest at least 86 dB below the test signal; the phaser's LFO starts
  as after 512 idle frames, one of the two starts (512 or 1024) FL's exports make
- Peak Controller: a project's Fruity Peak Controller slots open as Lemon Peak Controller (VST3 and CLAP, Pith editor)
  and save back unchanged; their controller links (kept byte for byte, formulas included) drive mixer volumes, pans,
  sends and plugin parameters one tick late, as in the original, within the fader's resolution of FL Studio 2026's
  exports (93 cases). The link's smoothing is approximated; links can't be made or edited in the app yet, and the
  links of rack controllers (Envelope and Keyboard Controllers, Control Surface) are kept but not played
- The Stereo Shaper's difference output plays in the mixer (added to the track it names, from the first tick, without
  delay compensation); Lemon Stereo Shaper as a plugin keeps the setting but has no output for it
- Lemon Parametric EQ plays FL's Parametric EQ 2 in every quality mode, in a project's mixer slots and as a plugin:
  plain, HQ Legacy, HQ+, HQ Enhanced and linear phase (LIN) with its sizes and crossfade speeds. The plugin tells the
  host LIN's and HQ Enhanced's delay, and Lemon Studio compensates it in a project's mixer slots as for any plugin
- The sampler plays a project's sample settings (start and length, the sample file's loop, the sample
  effects, the channel filter, declicking) and each note's own pan and filter offsets
- A file of 30 s or more loaded into a sampler channel turns its Keep on disk on (General settings ▸
  Advanced ▸ Auto keep long audio on disk)
- Lemon Trio's custom shape plays a sample as one period of the oscillator: choosing it opens a file dialog, and
  its sample page shows the wave and loads another. The sample is the channel's (saved with the project, and the
  channel is named after it while Auto name channels is on). Not yet: dropping a file on the sample page
- Keep on disk streams the sample from its file while it plays: only the first moments after each start point and a
  small buffer per voice stay in memory. It applies to uncompressed WAV and AIFF files played as they are; compressed
  files, the sample tools and effects, reverse and the Stretch / Stretch Pro methods keep the sample in memory, and
  the sampler says which ("in RAM" or "on disk"). Exports wait for the disk, so they sound exactly as from memory
- Inside Lemon Studio the playback engine keeps only the length of a WAV or AIFF sample that the sampler plays, not
  a decoded copy of its own, so a sample playing from disk takes no memory for its file there either. Opening a project still decodes each
  sampler file once while it loads; that copy is let go when loading ends

## ✅ Audio editing

- Tesla, Lemon Studio's audio editor and recorder (its counterpart to FL's Edison), sits in a mixer slot and is also
  a VST3 and CLAP plugin for other hosts. It records what reaches its slot (now, on input, while input is present,
  or while the song plays, for a set time or size), holds one sample with markers, regions and a loop, and edits it:
  cut, copy, paste, trim, silence, normalize, fades and declicks, reverse, channel tools, Amp, with undo
- WAV, AIFF and FLAC in; WAV out with Edison's chunks (markers, regions, loop), 24-bit too. Drag the sample or the
  selection out as a file, or send it to the playlist or the selected channel
- A project's Edison slots open as Tesla with their audio, and are saved back as Edison so the file still opens in
  FL Studio
- The toolbar's audio editor button and the mixer's Open audio editor show Tesla on the track; Open audio editor and
  record sets one up as an audio logger (records while the song plays, keeping the last 5 minutes); One-click audio
  recording and the Record button's question can record an input into the master track's Tesla; Remove all audio
  editor instances (mixer options, TOOLS) clears them after asking
- At its time limit Tesla keeps recording and holds the last minutes (a rolling log, as Edison)
- TOOLS ▸ Audio logger: on by default, it keeps the last 1 to 10 minutes of what the audio device plays (Settings ▸;
  the header shows the memory used); Dump to audio editor puts them, silence at either end left out, into a WAV in
  the Recorded folder and into the master track's Tesla
- Not yet: envelopes, the spectral and clean-up tools, time stretch and pitch shift in Tesla, slicing

## ✅ Exporting audio

- FILE ▸ Export audio (Ctrl+R) and Export MP3 (Shift+Ctrl+R) open the Export form: a name and folder, then the song
  or the current pattern, the whole of it or the time selection, and the tail cut, rung out or wrapped to the start
- Several formats at once: WAV (16 or 24-bit, 32-bit float), FLAC (16 or 24-bit, levels 0-8), MP3 and OGG Vorbis
  (64-320 kbps)
- Sample rate (the device's, or 44.1-192 kHz by resampling the mix), stereo or mono (merged, left, right),
  interpolation from 2-point linear to 512-point sinc, triangular dither on integer formats, plugins' HQ mode
- Split mixer tracks into a stems folder (WAV and FLAC), with or without the master; insert and master effects can be
  left out
- WAV files can carry markers, loop points, note markers and the tempo (`cue `, `smpl`, `acid` chunks)
- The choices are remembered; the folder can open when the export is done
- Not on purpose: FL's online and sharing exports

## ✅ MIDI files

- FILE ▸ Import ▸ MIDI file... (or a .mid dropped on the window): the Import MIDI data form picks the tracks, the
  MIDI channels, the instrument (Lemon Trio, Citrus or Lemon Sampler) and the options; a new project, or the file
  added to this one as one undo step. A channel per track (or per MIDI channel) with its notes, volume, pan and pitch
  bends, the tempo changes, time signatures and markers, in a channel group named after the file
- The modifier held while choosing the file picks whether the form shows (General ▸ Input ▸ Manage shortcuts, MIDI
  import window)
- FILE ▸ Export MIDI... (Shift+Ctrl+M): the current pattern, or the song's pattern clips, with a track per channel
- The piano roll's Tools menu: Import MIDI file... into its channel (replacing or blending), Export as MIDI file...
- Not yet: FL's FLEX mapping of the mod wheel and sustain pedal

## 🚧 Note input

- Mouse, computer keyboard and MIDI controllers; notes play the selected channel
- Touch controller (VIEW, Alt+F7 or the toolbar's touch button): 4 x 4 pads or a two-octave keyboard, played
  with the mouse, with octave steps
- Note recording (the toolbar's Record button): armed and playing, notes from the typing keyboard, MIDI or the Touch
  controller go into the current pattern, quantized to the main snap; a take replaces the notes that start inside it
  and is one undo step; in SONG mode a clip of the pattern is placed where none plays, and made longer when the
  take runs past it. A click or R (typing keyboard off) asks what to record (notes and automation, notes, automation,
  or audio into the playlist or the audio editor); armed, the whole button fills red. Right-click for the options
- Time selection loop: a piano roll (PAT) or playlist (SONG) time selection loops playback; armed with Loop record
  off it punches in and out (plays it once and stops back at its start)
- Score logger (TOOLS): every note played is kept for 30 minutes; Dump score log writes the last 2 to 30 minutes into
  the pattern at their real timing (Merge / Replace when it has notes); Clear log
- Automation recording: moves of a channel's volume and pan, a mixer track's fader, pan, stereo separation, equaliser
  and slot mix knobs, the tempo and any plugin parameter (in its own window) become the current pattern's control
  points while recording, each in its setting's own unit, as in the original
- Record options (toolbar, OPTIONS, Ctrl+I / Ctrl+P / Ctrl+B): Start on input (Play waits for the first note, which
  is recorded at the start), Recording precount (1 or 2 bars of metronome count-in, "REC IN n" on the clock; also
  set from the metronome's right-click menu) and Blend (a take keeps the notes it overlaps)
- Loop record (toolbar, OPTIONS): off, recording runs on past the pattern's or song's end and the pattern grows;
  on, playback loops and each pass is a take (one undo step) that replaces what the pass before recorded
- One-click audio recording (the toolbar's microphone, TOOLS): pick an input (an ASIO driver's inputs, else the
  system's input devices), the song plays with Record armed and precount, and on stop the take becomes a 32-bit WAV
  in the data folder's `Recorded` folder plus a new audio clip channel and clip where playback started; or, picked
  through Into the audio editor, the take goes into Tesla on the master track instead (see Audio editing)
- TOOLS ▸ Cancel recording: while recording, playback stops and the takes of that playback are undone (Redo brings
  them back)
- Recording into mixer tracks: arm tracks with their disk-record button or the track menu's Arm selected track(s)
  (arming turns Audio on in Record's filter); with Record armed, every armed track records while the song plays,
  taken at its record source (the input alone, with the mixer input, after the effects, after the EQ, after the
  fader). On stop each take is a WAV in the `Recorded` folder named after the track and, with the mixer menu's Disk
  recording ▸ Auto-create audio clips on, an audio clip channel on that track with its clip where the take started,
  the input's delay taken out (one undo step)
- Input monitoring: a track's input plays through it while armed, or always, per its Monitor external input setting

## 🚧 Undo

- One history for the whole project, with redo: Ctrl+Z, Ctrl+Alt+Z and the EDIT menu, which names the step
- Covers the piano roll, the playlist, step-sequencer edits and knob turns (channel, main, mixer panning and
  stereo separation, effect mix, send levels, swing), sample loads, the project's initial values, PATTERNS ▸ Split
  by channel and ADD's audio and automation clip channels
- The mixer: faders (a drag is one step, a typed value too), polarity and swap buttons, the equaliser graph,
  routing, colours, names, resets, effect slots (adding, removing, moving, naming, their lights, sidechain inputs),
  audio inputs and outputs, record source, monitoring, latency settings, and adding, moving, deleting and trimming
  tracks. A track's mute and solo make no step, as in FL Studio
- The channel rack: generators added, at the bottom or inserted higher up (the plugin picker, F8, ADD, Insert, a
  browser drop; undone, the instance is kept and comes back with a redo), Replace (with the old instrument's state),
  colours, names, cloning, moving, sorting, deleting (with notes, clips and the instruments'
  state), mixer track assignments, groups and Burn loops, and the PATTERNS menu's colours, names, inserting, cloning,
  moving and deleting
- FILE ▸ Import ▸ MIDI file into the open project
- The tempo (the toolbar's box; the tempo tapper's taps, one step per count), Settings ▸ Project's timebase (its
  ticks come back exactly), time signature, panning law, Play truncated notes in clips and Automatic crossfades, and Settings ▸ Info's boxes
- Plugin parameters Lemon Studio sets ("Plugin parameter": Type in value, Set..., Paste value), and those changed in a
  plugin's own window: one step per gesture (mouse down to up on its knob), or per run of values from a plugin that
  sends them without one; mixer faders and parameters follow "Undo knob tweaks"
- Presets loaded into a plugin ("Plugin preset": its window's Presets box, ◀ ▶ and window menu, Import preset..., a
  preset file dropped on the window, its channel or its slot), one step each, by the owner's id so it follows moves:
  the undo brings back the plugin's state, the preset its window showed and the name the load gave its channel or
  slot; the redo the state as the undo found it. A preset that goes into an instance made for it is part of that
  step. Lemon Studio's choice; the original's behaviour was not looked at
- Changes made through the control plane (see Scripting and assistants), named after the client that made them
- The browser's Current project ▸ History lists the steps from "Last reset"; undone steps are grey, and a click
  on one goes back or forward to it
- General settings ▸ Undo history: Maximum undo levels, Undo knob tweaks, Alternate undo mode, and showing the
  history in the browser, newest first if wanted
- Not yet: "Put undone recordings in the recycle bin", which waits for recording. A plugin's own undo inside its
  window is the plugin's, apart from the app's history; a state a plugin changes by itself without parameter edits
  (its own preset browser) makes no step, as the host is not told

## 🚧 Scripting and assistants

- The control plane ([control-plane.md](control-plane.md)): `--control <socket>` beside the window, `--serve` without
  one; JSON-RPC 2.0 over a Unix socket or a local port
- Reads the project (channels, mixer, patterns, notes, playlist, plugin parameters) with stable ids, and changes it
  with commands: one undo step per request, applied whole or not at all
- Read-only, propose-only (the user applies or discards each proposal) and edit levels; saving to a file asks the
  user while there is a window
- Events for every change, including edits in the interface
- Not yet: live commands to the playing engine, a native project format, rendering through it

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
- An ASIO driver's first 8 inputs open with its outputs (full duplex) for audio recording
- Not yet: ASIO output pairs, JACK

<img alt="ASIO Compatible logo" src="../crates/os-app/asiosdk/logo/ASIO-compatible-logo-Steinberg-TM-BW.png" width="96">

ASIO is a registered trademark of Steinberg Media Technologies GmbH.

## 🚧 Platforms

- Windows 10 / 11
- Linux: X11, and Wayland through XWayland (see the [Linux notes](../README.md#linux))
- macOS 11 or newer, supported at the same level as Linux (see the [macOS notes](../README.md#macos))

## 🚧 Look and themes

- Two skins, picked in Settings ▸ Theme: Lemon (lemon on neutral greys, rounded-square controls) and Citrus (the
  look formerly called Classic)
- 25 built-in themes, your own themes and `.flstheme` files colour either skin; Settings ▸ Theme ▸ Adjustments edits
  the applied theme live with five knobs and eight colour circles, each with a colour picker
- A theme file's workspace picture (PNG or JPEG, on either design) and its playlist and piano roll grid line
  strength; not yet the HTML background, the event editor grid and the Option colour
- Lemon Studio's own plugin editors are drawn in the Lemon design only, whatever design the app shows
- Smooth scrolling: the wheel and the scroll bars' arrows ease the editors' views (General settings ▸ Display ▸
  Smoothing, and Animation refresh rate for how often animations redraw)

## 🚧 Settings

- The General, Audio, MIDI, File, Theme, Project and Info pages work: the controls that are not greyed out act, but for
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
- Not yet: the recording settings
