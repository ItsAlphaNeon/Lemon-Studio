# Feature status

The details behind the [Capabilities](../README.md#capabilities) table, area by area. Controls for anything
that is not implemented yet are greyed out in the application: hovering one says "not available yet", and
clicking it does nothing. The full, item-by-item roadmap lives on the
[Discord server](https://discord.gg/yC2uCuGs3M).

✅ usable · 🚧 partial · ❌ not available yet

## ✅ Step sequencing and patterns

- Channel rack with steps and mini piano rolls
- Swing and per-channel loops
- ADD ▸ Layer: a layer channel whose children are the channels selected in the rack; the rack's Create layer with
  selected channels does the same
- The rack's menu sets the selected channels' swing mix (0 to 100% or a typed value), turns their Truncate swing
  notes on or off (a swung note keeps its end, so it gets shorter) and gives them one colour (Color selected ▸
  All...); a channel's Cut itself gives it a cut group of its own, as the original does. One undo step each
- The mixer box's and the mute LED's right-click menus type in, copy, paste and set their value (a mixer track, on or
  off); Burn selected loops to pattern; Channel button width ▸ Custom (a typed percent); the options Auto switch
  display filter, Focus selected channel on playback, Mute removed steps (a removed step keeps its note, muted), Show
  complete piano roll preview, Show a mute button on the piano roll preview and the loop button's Always show advanced
  controls
- A channel's Receive notes from ▸ locks it to the computer keyboard or the Touch controller: their notes play it
  whichever channel is selected (a padlock on its name button; Unlock ends the lock)
- A channel's menu: Change icon... puts an icon on its name button (the same icon as its plugin window and clips, saved
  in `.lemon` projects); Show waveform preview draws a sampler or audio clip channel's sample across its name button; MIDI
  channel through (ticked by default) gives a hosted VST, VST 3 or CLAP instrument a MIDI device's notes on the MIDI
  channel they came on, and a recorded note takes that channel as its colour. The two checks are saved in `.lemon`
  projects
- The rack's Restretch all (Ctrl+Alt+R) fits every stretched sample and audio clip to the project tempo at once
  (they follow the tempo by themselves too, also with tempo automation)
- The rack's knob, mixer box and LED menus: Edit automation in piano roll opens the piano roll on the channel volume
  or panning lane, the swing knob takes a song start value, and the mixer box and the LED link to a MIDI controller
- The mixer options' Link all parameters... links the current track's volume, pan, separation, equaliser, sends and
  slot mix levels to consecutive MIDI controllers in one go
- Advanced looping (the loop button's third loop type): each row's loop box takes a loop point, Step, Beat or Bar,
  and the row loops at the end of its last used step rounded up to it
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
  play in SONG mode; exports start at the Start marker and jump at skips, and the clock's bars count from the Start
  marker. Move content left / right, Place loop
  (Shift+T), Add auto, Add one every, Add jump to next bar, time signature markers, and Delete or Change to marker
  loop in the time selection, Add two... (markers at the time selection's ends) and Add key marker (root and
  scale, saved as FL saves them). A click on a marker moves the song position there. A Stop recording marker ends
  recording in SONG mode, a Start recording marker after it resumes it. The playlist's snap has Events and Markers
  (clip edges or markers within 7 px)
- Quantize song jumps (the playlist's ▸ menu): while the song plays, a click in the ruler waits for the next beat,
  bar, or 2, 4 or 8 bars, then jumps there exactly on it
- Enable groups (toolbar, OPTIONS menu, Shift+Alt+G): turned off, grouped clips select and move one by one
- Track height, zoom and scroll saved with the project
- The clip source picker's item menu: Cut itself (the rack's, one undo step) and Edit sample..., which opens an audio
  clip's sample in Tesla on the master track
- Project picker (VIEW, Ctrl+F8 or the toolbar's project button): a menu of the patterns and channels; picking a
  pattern makes it current, picking a channel opens its window
- Song position slider in the toolbar: drag, click to step, right-click to jump, wheel; it follows playback
  (through the pattern in PAT mode)
- Auto scrolling (toolbar, OPTIONS menu): while playing, the playlist follows the playhead (the piano roll in
  pattern mode), continuously or a quarter page at a time (right-click on the button)
- Performance mode (Ctrl+P, the arranger's ▸ menu; Prepare for performance mode or TOOLS ▸ Set up for live
  performance asks first, then lays out the Marker settings' columns, 16 four bars apart by default, and a Start
  marker): a click on a clip before the Start marker launches it on its
  track, looping, from the next sync point (Shift queues, Alt at once); a right click or the track header stops it.
  Notes 0–119 on Settings ▸ MIDI's Performance mode MIDI channel launch the first 10 tracks' 12 columns (a key's
  release ends Hold & stop; notes 120–124 scroll the grid, outlined in the zone; while 127 is held, launches sync
  to the global snap). Ctrl+click on a zone clip launches a scene (a Scene stops the other tracks, a +Scene keeps them):
  its clip group's clips, else its column on its track group's tracks, else its whole column, as do notes while 125
  (Scene) or 126 (+Scene) is held (columns). A playing clip's zone clip is outlined and fills as it plays, and the
  track header can show the pass's progress (the ▸ menu's Performance clip / track progress; these checks, Ctrl+click triggers +Scene and Velocity picks
  sub-clips are saved in `.lemon` projects); the clock's bars are
  negative in the zone. Each track's Performance settings ▸ (Press, Motion, Trigger sync, Queue, Tolerant,
  Position sync) are saved as FL saves them. With Record armed, stopping the song writes what the launched tracks
  played after the Start marker as clips on their tracks (one undo step). A slot's later clips are sub-clips: the
  mouse plays them in turn, a MIDI note's velocity picks one (the ▸ menu's Velocity picks sub-clips), and Motion
  plays each slot's first clip. In Performance mode the typing keyboard launches slots as those notes do (the
  original's layout: 1–5, Q–T, A–G and Z–B are tracks 1–4, 6–0, Y–P, H–; and N–/ tracks 5–8, five columns; it
  follows the grid's scroll, and a key's release is the note's); its other note keys do nothing

## ✅ Mixing and routing

- Mixer tracks, sends, effect slots and meters
- Polarity and stereo swap, main volume and main pitch
- Sidechain inputs: a project's Fruity Limiter slot listens to the track routed into its track that its sidechain
  number names (Sidechain to this track routes count), heard after that track's fader and delayed to line up
- CLAP and VST3 effects get a sidechain input too: pick it in the slot menu's Sidechain input submenu, which lists
  the tracks routed into the track. The choice is saved where FL saves it, and projects saved by FL play their
  plugins' sidechain and input choices as FL plays them
- The mixer track menu's Insert multiple... (type how many tracks to insert), Gradient color... (over the selected
  tracks) and File ▸ Render to wave file... (the Export form for the selected tracks, writing each track's output to
  its own file)
- The mixer track menu's Track routing ▸ routes or sidechains the other selected tracks into the track clicked (only,
  or besides their other routes), and an effect slot can be given a colour (Change color..., Random color), shown as a
  bar at the slot's edge and saved in `.lemon` projects, as are the names slots are given
- The mixer track menu's Group section: Separator (a gap before the strip), Create group... (names the track and puts
  separators around the selected tracks), Auto color group and Select ▸ Group; Dock to ▸ Left / Middle / Right keeps a
  strip after the master or at the right edge while the others scroll; Allow threaded processing off keeps the track
  on the rendering thread. All saved in the project as the original saves them
- Mixer track states: the track menu's File ▸ Save mixer track state keeps a track's levels, switches, equaliser and
  effects with their settings in the user folder's Mixer states; File ▸ Open mixer track state ▸ puts one on a track
  (it keeps its name and colour) and Insert from state ▸ inserts a new track from one. Lemon Studio's own files
  (`.lemontrack`), not the original's
- Mixer options ▸ View ▸ Waveforms (Alt+W, also the title bar's button) draws each track's recent peaks as a scrolling
  waveform in place of its meter; Extra properties (the title bar's other button) shows or hides the polarity, swap
  and separation controls. Both saved in `.lemon` projects
- The mixer slot menu's Help opens the plugin's own help page (CLAP and VST3 plugins that name one; Lemon Studio's
  own effects open the list of built-in effects), and Scaling ▸ sizes a hosted plugin's window: Main (the program's
  scale) or 100 % to 200 %, saved in `.lemon` projects
- Change icon... in the mixer track menu draws an icon under the track's name, and in the slot menu before the
  slot's name and on its plugin window; the icons move with their tracks and slots and are saved in `.lemon` projects. The slot menu's
  Link all parameters... links the effect's parameters to consecutive controllers, as its window menu's
- Containers (a first part of the original's Patcher, in Lemon Studio's own form): the slot menu's Wrap in container
  moves the slot's effect into a container in the same slot, and Wrap all in container moves all of the track's
  effects into one container in that slot, in slot order. One undo step each. A container plays its effects one
  after the other, each with its own wet and dry levels (taken from the slot's mix knob and switch), and its delay
  is the sum of theirs, made up as any plugin's. The container keeps the slot's name, colour and icon. Its list
  (the slot menu's Container ▸ Edit list..., or opening the slot) opens each effect's own plugin window, sets its
  wet and dry levels in the value box (in percent), and moves, removes and adds effects (the effect list of an empty
  slot), one undo step each. Containers are saved in Lemon
  Studio's projects with their effects' settings. The effects inside are listed with their parameters in the
  browser's Effects (after their container), and their parameters' menu makes automation clips, controller links and
  event editors as a slot's effect's does; each level in the list has that menu too. A clip or link names the effect
  by its container's slot and its id in the container (at most 15 effects), so it follows the effect when the list
  is reordered, plays in playback and export, and is saved with the project. A member's window lists, steps, saves
  and loads its presets. Not yet: undo steps for a member's window edits and presets, moving a wrapped slot's own
  clips and links onto the member, and the original's Patcher slots in imported projects

## ✅ Plugin hosting

- CLAP, VST3 and VST2 instruments and effects, with their editors and saved state
- A plugin's right-click menu on a parameter offers "Create automation clip" (CLAP and VST3); a VST3 plugin that asks
  the host to show its menu gets one of Lemon Studio's own menus, docked or detached
- The parameter menu (a parameter in the browser after "Browse parameters", TOOLS ▸ Last tweaked): create or edit
  the parameter's automation clip, link a controller, copy, paste, type in or set a value (a submenu of its steps, or
  its default and minimum to maximum), remove an initial value read from a file
- Not yet in the parameter menu: Init song with this position on a plugin parameter
- VST2 shells (one file offering several plugins); a VST2 plugin's state is saved in the layout FL Studio's projects use
- A plugin that crashes is stopped (its slot passes the sound through) instead of closing Lemon Studio. On macOS
  and Linux this is new, and on macOS a crash in a plugin's editor still closes Lemon Studio
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
- Mixer options ▸ Plugin delay compensation ▸ Automatic (on): unticked, plugins' delays are no longer made up, while
  the tracks' latency settings still are. Saved in `.lemon` projects
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
- The mixer's slot menu lists the effect's presets and has Browse presets and Save preset as..., as its window
  menu does
- Spare state in plugin windows and mixer slots: Store in spare state, then Flip with spare state swaps the two to
  compare them (one undo step; the spare state is kept while the plugin is open, not in the project). Display preset
  name in preset selector hides or shows the preset's name in the window
- The plugin window's menu acts on its own channel (Piano roll, Delete..., Rename, color and icon..., Change color...,
  Random color) or effect slot (Replace ▸, Delete..., Rename, color and icon..., Change color..., Random color); an
  effect's window menu has no Piano roll or Channel column, as in the original
- Allow threaded processing and Auto-bypass when silent (the original's Smart disable) in a plugin's window menu and a
  mixer slot's menu: per plugin switches, one undo step each, saved in the project the way the original saves them.
  With Auto-bypass when silent on (and Audio settings ▸ CPU ▸ Auto-bypass when silent, on by default), a mixer slot
  stops running its effect once its input and output have been silent for a hold time (a second for the built-in
  effects, the whole delay line for Delay 3, four seconds for hosted plugins; never for the Peak and Formula
  Controllers) and runs it again as soon as sound reaches it; what is heard is the same. Switch auto-bypass for all
  plugins (TOOLS, the mixer's and the rack's options menus) turns it on for every plugin when one is off, and off for
  all when all are on. A plugin channel with the switch on stops running its instrument once it has had nothing to
  play (no note held or arriving, no controller message, parameter value or audio clip) and been silent for four
  seconds, and runs it again with the next one
- The plugin window's menu: Link all parameters... links every parameter of the plugin to consecutive MIDI
  controllers in one go (the MIDI learn box); Change icon... gives a channel or effect an icon, shown in its window's
  title, the playlist picker and its clips (saved in `.lemon` projects); Save channel state as... saves a channel with its plugin
  and settings into Channel presets, which Browse channel states opens in the browser, and dropping one adds the
  channel. The picker's Change icon... too
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
Not yet: Align all channel editors.

**Arranging windows.** VIEW > Arrange windows: Realign (Shift+Ctrl+H) and Desktop (default) put the Playlist, Mixer,
Piano roll and Channel rack back as a new install has them; Desktop (alternate) floats the Playlist over the top of
the workspace with the Piano roll and Mixer side by side under it; Save custom layout keeps the current layout and
Custom brings it back. The Tablet layouts are greyed (they need the toolbar at the bottom). VIEW > Background > Set
image wallpaper... makes a PNG or JPEG picture the theme's workspace wallpaper; the HTML document and the background
colour are greyed. VIEW > Align all channel editors (Shift+F12) puts the channel window back right of the Channel
rack and cascades the docked plugin windows of rack channels from there, each title showing. HELP > Help index and
What's new open the project's README and its merged pull requests in the web browser, and Tutorials ▸ the README's
how-to sections; HELP > Diagnostics writes a plain-text report (build, system, audio settings, plugins found, open
project) to Diagnostics.txt in the user folder and opens it. TOOLS > Save MIDI clipboard to browser writes the piano
roll's copied notes to a numbered MIDI file in the user folder's Clipboard files, which the browser then shows; Save
audio clipboard to browser does the same with the audio on the clipboard (what Tesla copies; Windows and macOS).
VIEW > Remote control opens the browser's Remote control section, one row per controller link.
TOOLS > Set up for live performance prepares the arranger for performance mode. TOOLS > Switch auto-bypass for all
plugins switches every plugin's Auto-bypass when silent.
TOOLS > Chord progression tool... (Lemon Studio's design) asks for a progression, one chord a bar, as chord names
(C G Am F7 Dm/F) or numerals (I V vi IV, with "Am:" first for the key) and writes it into the piano roll's pattern
after its last note, as one undo step. TOOLS > Set keyboard focus of all plugins to ▸ Disabled keeps plugin editors
from taking typed keys, which then play and steer Lemon Studio instead (Windows and macOS; saved in `.lemon` projects). TOOLS >
Lock all tracks to their clips locks each playlist track whose clips share one source to it, so Draw and Paint place
that source there whatever the current one is (a lock shows in the track header); Unlock all tracks from their
clips clears the locks. Both are one undo step, and the locks are saved in `.lemon` projects.
Not yet (greyed in the menu bar): TOOLS > Control builder, Set up for MIDI export; VIEW > Script output.

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
  drag draws freehand on the global snap, right drag a straight line, Paint (drawing off the snap), Erase,
  Interpolate (a drag lays the events under it on a straight line, right drag a smooth one), Select and Zoom (Z:
  a click zooms in about it, a drag on the stretch dragged over, a right click out) tools, the wheel scrolls and Ctrl+wheel zooms; its Edit menu has Cut, Copy, Paste, Select all, Select previous / next
  (Ctrl+Left / Right: the selection moves by its length), Delete, Duplicate (Ctrl+B), Insert space (Ctrl+Ins), Delete
  space (Ctrl+Del), Insert current controller value and Turn into automation clip, then Zoom in / out (PgUp / PgDn),
  Zoom on selection (Shift+5) and Relocate events (Paste makes room, Cut and Delete close the gap), and its Tools
  open the piano roll's Scale levels and LFO (Ctrl+L) dialogs on the control's events in the selection. Turn
  into automation clip moves the events into a new clip channel linked to the control, as the original does
  (the control's earlier value until the first event, then the events, a recorded ramp as one straight segment, ending
  on the last event). Each edit is an undo step
- Edit automation in a window, on the same menus, opens a second event editor window on that control, in front of
  the first, which keeps its control; a click on the one behind brings it in front, the editors share a clipboard, and
  closing one leaves the other (Lemon Studio's design)
- A knob's menu (channel volume and pan, mixer and main knobs): Edit automation clip... shows the clip that drives it,
  as on a plugin parameter, and Set... puts it at its default, the minimum, 25, 50 or 75% or the maximum, each listed
  with the knob's value there (one undo step)
- Not yet in the event editor: Auto smoothing, Center

## 🚧 Project files

- Saves projects as `.lemon` files: Lemon Studio's own format, laid out like an FLP with room for Lemon Studio's own
  data (the channel, mixer track and slot icons, the slots' names and colours, Performance mode's ▸ menu checks, the
  mixer's Plugin delay compensation ▸ Automatic and View ▸ checks, plugin windows' Scaling ▸, the channels' waveform
  preview and MIDI channel through checks, the plugins' keyboard focus and the playlist tracks' locks; saving to a
  `.flp` name leaves them out). The original is not expected to open them
- Opens `.lemon` files and `.flp` projects saved by FL Studio 24.2 and later, 25 and 26 included. A project opened
  from an FLP is never saved over: Save asks for a name, offering the FLP's name with `.lemon`. Backups, new versions,
  templates and zipped projects are `.lemon` too; FLP backups and templates still open. General ▸ System ▸
  Associate project files covers `.lemon` along with `.flp`
- Projects saved by FL Studio 10 to 24.1 open too: they are converted to the current layout as they open (channel
  blocks, step sequencer dots as notes, playlist clip and track records, mixer places, send levels and slot
  markers, a 105-track mixer grown to 127) and are saved in that layout, as the original does with an older file.
  Fruity effects that an old project hosts through the plugin wrapper stay wrapped, and a 3x Osc saved before FL
  Studio 21 opens with its envelope and filter settings at their defaults
- Anything in a file that Lemon Studio does not understand yet is kept and written back unchanged
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
- Lemon Slicer (Fruity Slicer) as a VST3 and CLAP plugin with a Pith editor: it loads a loop and slices it by the
  file's markers, by beats or by transients, plays slice n on C5 + n (pitch, attack, decay, declick, reverse, play to
  end), and dumps the slices to the piano roll as notes. A project's Fruity Slicer channels play and open in it and
  are saved back byte for byte; a loop newly loaded and sliced in it opens in the original too. The time stretch and
  the original's other dump modes aren't played or offered yet, and Slicex channels aren't supported
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
- Vocoder: Lemon Vocoder (VST3 and CLAP, Pith editor) is a band vocoder with a sidechain input: the voice on the
  track and the synth from another track, or the other way round. A project's Vocodex slots open as it and save back
  unchanged. It plays the wet level, the pass-throughs, added noise, hold, attack and release (peak or RMS), band
  count, width and order; the original's built-in synth carrier, unison, pitch shift, contour and the band curves
  (other than where the bands start and end) are kept but not played. Its sound is Lemon Studio's own, not compared
  with FL Studio's exports. Changing its input selectors reroutes the track only after the project is saved and
  opened again
- Peak Controller: a project's Fruity Peak Controller slots open as Lemon Peak Controller (VST3 and CLAP, Pith editor)
  and save back unchanged; their controller links (kept byte for byte, formulas included) drive mixer volumes, pans,
  sends and plugin parameters one tick late, as in the original, within the fader's resolution of FL Studio 2026's
  exports (93 cases). The link's smoothing is approximated; links can't be made or edited in the app yet, and the
  links of rack controllers (Envelope and Keyboard Controllers, Control Surface) are kept but not played
- Drum sampler: a project's FPC channels open as Lemon Drum Sampler (VST3 and CLAP, Pith editor with the 4×4 pad
  grid, banks A and B, the chosen pad's and layer's key, cut groups, volume, pan and tune) with their pads and
  samples, and save back unchanged. It is in the generator list too. Pads play their layers by velocity through
  Lemon Sampler's voices with the original's level, pan and tune laws, cut groups, mute, solo and layers taking
  turns, measured from FL Studio 2026's renders; a sample is loaded with LOAD or dropped on a pad. The pads'
  envelopes and the multi-output offsets are kept but not played, and the original's FPC presets aren't listed
- Formula Controller: a project's Fruity Formula Controller slots open as Lemon Formula Controller (VST3 and CLAP,
  Pith editor with a formula field and its compile status) and save back with their knobs, formula and comments; the
  formula is worked out once per tick and its controller links drive their targets, matching FL Studio 2026's
  exports within the fader's resolution (158 cases: every function, the original's precedence and rounding rules). A
  formula that doesn't compile leaves its targets alone, as in the original. Rand and SeededRand use our own
  generators, Time counts from the program's start, Date is the UTC day and the mouse reads 0; the comments are kept
  but not shown
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
- Tesla's Time stretch / pitch shift... (Alt+T, the tool grid's stretch button) changes the selection's pitch by up
  to 24 semitones either way and its length from a quarter to four times, with a preview, on Lemon Studio's
  stretcher, with formant preservation (the formants kept, or moved by their own knob); the drum(loop) stretch comes
  later
- TOOLS ▸ Audio logger: on by default, it keeps the last 1 to 10 minutes of what the audio device plays (Settings ▸;
  the header shows the memory used); Dump to audio editor puts them, silence at either end left out, into a WAV in
  the Recorded folder and into the master track's Tesla
- Tesla draws panning, volume, stereo separation and all-purpose envelopes over the sample (the bottom bar's
  modes): drag, add and delete points, flip, normalize, add points at the selection or regions, cancel them all, and
  Mixdown amp envelopes (Ctrl+E) applies the first three to the audio, with undo
- Tesla's Convolution reverb... (Ctrl+R, the tool grid's reverb button) adds a reverb to the selection from a
  generated impulse (its decay set by a knob) or from what was copied, with dry and wet levels and the tail
- Tesla's Equalize... (Ctrl+Q), Blur... (Ctrl+B) and Clean up (denoise)... (Ctrl+U) work on the selection:
  Equalize draws a gain curve over the selection's spectrum, Blur smears each frequency over time by an impulse
  curve, and Clean up takes away noise matching a profile acquired from a noise-only selection (Acquire noise
  profile, Alt+U), with a declipper that redraws flattened tops; all three preview, and mix with the original
- Tesla slices: Regions > Auto-slice (dull, medium (Alt+S, the auto-slicing tool button) and sharp, with the
  transient detector Lemon Slicer uses; small, medium and large grids at the sample's tempo), Detect beats (sets the
  tempo and makes a region per beat) and Zero-cross check all regions; the slices are regions, so a saved file opens
  in Lemon Slicer with them
- Tesla's Send to new Lemon Slicer channel (Tools > Sequencing, and the auto-slicing button's menu) puts the selection
  or the whole sample in a new Lemon Slicer channel, sliced at its regions (auto-sliced when it has none); dragging
  the sample out of Tesla carries its regions too
- Tesla's Drum(loop) stretch... (Ctrl+T, the stretch button's right click) makes a loop longer or shorter by moving
  its slices (its regions, or the detected hits) apart or together, each keeping its sound; the gaps are filled with
  echo grains or a time-stretched slice, or left silent, under a tail fade
- Tesla's noise gating: Acquire noise threshold (Alt+N, from a noise-only selection, else -40 dBFS), Gate noise (the
  gate button's right click), Trim side noise (its left click) and Trim all noise & slice up (each sound left a
  region); the bottom bar's threshold button shows the threshold over the waveform
- Tesla's Clean up (Ctrl+U) has a declicker (with a sensitivity) that redraws clicks and pops, and the declipper has
  three modes (General, Digital for flat tops only, Soft) and a Safe mode; either one alone is enough to preview
- Tesla's envelope tools with a dialog, in an envelope mode: Scale levels..., Decimate points..., Filter..., Smooth
  up... and Create sequence... (steps over the selection), each with a preview
- Tesla's Regions > Detect pitch regions makes a region per held note, named after it
- Tesla's Regions > Set first downbeat starts bar 1 at the selection or cursor: the Bar : Beat : Tick ruler, grid
  slicing and Convert to score count from it
- Tesla's Tools > Convert to score and dump to piano roll puts the held notes of the selection into the selected
  channel's piano roll (current pattern), at the project's tempo
- Tesla's envelope points can be smooth (Alt+click one, or Tools > Turn all points smooth); the amp envelopes are
  heard while Tesla plays, before a mixdown; Scratch (envelope) plays the selection at the position the all-purpose
  envelope draws (bottom the start, top the end), and Edit > Paste mix (envelope) (Shift+Ctrl+V) mixes the clipboard
  in by it
- Tesla's volume envelope runs from silence to 200% (+6 dB) and its stereo separation envelope from double width at
  the bottom to mono at the top, as the original's manual gives them
- Tesla's Tools > Vocal denoise / isolator... keeps the voice (centred, harmonic, in the voice band, over the steady
  noise) and takes out the rest, or the other way round, by one MIX knob; Deverb... takes the late reverb out of the
  selection (General or Vocal) at a reverb time it measures there; both preview, with undo
- Tesla's Edit > Undo mix (envelope) ▸ mixes an earlier state of the sample back in by the all-purpose envelope (the
  top undoes, the bottom keeps the change)
- Not yet: Edit > Undo mix (spectral)

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
- On Linux: the sound server (PulseAudio, or PipeWire through pipewire-pulse), ALSA directly, or JACK (Settings > Audio
  ▸ Mode ▸ JACK: a JACK client at the server's rate and buffer length, its ports connected to the system's; with no
  JACK server running it plays as in shared mode)
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

- Every page works: MIDI, Audio, Recording, General, File, Plugins, Theme, Project and Info. The controls that are not greyed out act, but for
  a few that are only kept for now (Resampling quality and others Lemon Studio has nothing to change
  with yet); the program's choices are kept in `settings.txt` in the user folder
- General ▸ Input ▸ Manage shortcuts: the F8 keys (browser, plugin picker, plugin database) and the audio clip
  double-click can be swapped
- File ▸ Browser extra search folders: the folders added to the browser, renamed or replaced from here
- General ▸ Display ▸ High visibility: a white record dot, brighter channel names and light scroll bar thumbs
- General ▸ Display ▸ Toolbars scaling: the main window's top band at its own size; panels that don't fit
  leave an arrow
- General ▸ System ▸ Associate project files (Windows): `.lemon` and `.flp` files open with Lemon Studio, for your user
  only, and unticking puts the previous program back
- Recording, Lemon Studio's own page (the original has none): the recording filter, the Record button's options and
  quantize, Start on input and its checks, Loop record, Blend, Count-in and its length, Auto-create audio clips and
  the External input question's answer, each the same setting as the menu or button it is also set from, and where
  recordings go
