# Lemon Studio: cutting-edge

This is the `cutting-edge` branch of Lemon Studio. It is [`main`](https://github.com/ItsAlphaNeon/Lemon-Studio/tree/main)
plus what is listed here. For everything else (building, using, contributing, license), read `main`'s README.

> **Anything here can be dropped at any moment.** This branch is where ideas are tried. A feature listed below may
> change or be removed without notice, and nothing here is promised to reach `main`. Don't build on it, and keep
> copies of what you make with it (`.lemon` files included).

> **Linux only, Wayland first, for now.** This branch is built, tested and used on Linux under Wayland (Hyprland).
> X11 still works (`OPENSTUDIO_WAYLAND=0`). Windows and macOS may build, but nothing added here is tested there, and
> some of it is Linux-only (plugin editors drawn in the app's window, the Hyprland plugin-window handling).

`main` is merged into this branch with `tools/merge-main.sh`, which keeps this README and `docs/` to this branch's
own content. Side branches are named `cutting-edge-<topic>`.

## What this branch adds

- **Control plane.** Every edit goes through one command layer with undo. Projects have stable ids, and scripts and
  assistants can drive the app over a local socket (`--control`, `--serve`). See [docs/control-plane.md](docs/control-plane.md).
- **A project journal.** Every edit, undo, save and plugin state is written to disk as it happens, saved or not, and
  readable through the control plane: a sound made in a plugin's window survives an unsaved project. See
  [docs/control-plane.md](docs/control-plane.md) "Journal".
- **Lemon project files (`.lemon`), the default for new projects.** A project file of Lemon Studio's own: the project as an FLP plus its
  whole history across sessions (every change and every plugin state), so Recover from journal reaches any sound the
  project ever had. See [docs/native-format.md](docs/native-format.md).
- **Older FL projects open.** Projects saved by FL 20 to 24.1 are converted to the newer layout as they are read.
  Save asks for a new file.

- **Wayland first.** On Linux the main window is native Wayland in a Wayland session; plugin windows open detached.
  `OPENSTUDIO_WAYLAND=0` brings back the X11 window, where plugin windows dock. CI runs on Linux only
  (`.github/workflows/cutting-edge.yml`); Windows and macOS are not tested on every push.
- **Resizable plugin windows (Linux).** Besides Lemon's own resizable plugins, any plugin whose editor lets the host
  resize it (CLAP `gui.can_resize`, VST3 `IPlugView::canResize`) gets the frame's edges and maximize button. The
  editor is asked for the new size and the frame takes the size it accepts. Such an editor larger than the monitor's
  work area opens shrunk to fit it. Detached plugin windows open centred on the monitor under the pointer.
- **Tiling layout, the default.** General settings ▸ Display ▸ Layout: Tiling or Classic. The tiling layout has:
  - a toolbar of one row (menus, transport, time, pattern, main volume and pitch, windows, utilities);
  - a status bar along the bottom (Lemon's mark, the title and hint, global snap, mode buttons, pickers, CPU and
    memory): the dropouts since the program started show in a red pill, and the audio load turns yellow, orange and
    red as it climbs;
  - the workspace and the browser as rounded cards over a darker backdrop; the browser's splitter is the gap between
    them, and the mixer's routing cables hang over the gap below. With Animations on "Playful", the cables between
    tracks at most three apart (bus 3 into the master, say) hang on in front of the split window below the mixer;
    longer ones stay behind it.
  - split views instead of floating ones: the arranger, the piano roll and the mixer are maximized or snapped into a
    split of the workspace, each its own card. The status bar's split button picks the preset (top and bottom, or left
    and right); a title bar dragged onto another window swaps the two, the gaps drag to resize (a double click resets
    one), and windows keep a least size. F5, F7 and F9 show and hide their window on their own, and a window opening
    goes into the split (one maximized gives way). The toolbar's window buttons are lit while their window is open.
    There is no minimize; the maximize button shows a full square, or two panes to go back to the split, and a window
    alone has only its close button. The instrument rack, plugin and tool
    windows still float. With Animations on "Playful", windows glide to new places as Omarchy's Hyprland ones
    do; a window opening is at its place at once, under them as they move away, and one closing leaves its
    last look there for them to move over, and maximizing is instant.
  - the browser docked left or right: its title bar drags it to the other side.
  - window keys: split windows other than the one last in front are muted a little, fading as Omarchy's borders do
    (floating windows change nothing). Ctrl+Alt+arrows move the keyboard to the split window that way (floating windows
    are no stops; while one is maximized, the one that way is maximized instead), Ctrl+Alt+Shift+arrows swap split
    windows, and F maximizes the window under the mouse or brings the split back. The window manager's close shortcut
    closes the floating windows first, the one in front first (plugin windows, the instrument rack, the channel window,
    tool windows), whichever window has the keyboard; the app closes only when none is open.

  The tiling and Classic layouts each keep their own toolbar and, for the session, their own window arrangement.
  Every other keyboard shortcut works the same. Classic is FL's arrangement and unchanged.
- **Its own names.** What FL calls by its own words has the usual or a plain name here; the files (`.flp`, plugin ids)
  keep FL's, so projects still open both ways. Among them: Arranger (FL's Playlist), Instrument rack (Channel rack),
  Bus N (Insert N), Plugin palette and Clip palette (the pickers), Plugin library (Plugin database), Status bar (Hint
  bar), Multi MIDI learn (Multilink to controllers), Computer MIDI keyboard (Typing keyboard to piano), Auto-bypass when
  silent (Smart disable), A/B compare (spare state), Collapse / Expand (Zip), choke groups (cut groups), Mod A / Mod B
  (Mod X / Mod Y), Click removal (Declicking), Count-in (Recording precount), Tap tempo, Step parameters (Graph editor),
  MIDI capture and Retro audio capture (Score and Audio logger), Project clean-up (Macros), MIDI learn... (Link to
  controller...), Enter value... (Type in value...) and plainer settings labels. The menu bar's titles are spread evenly over
  its panel. Every menu keeps its place and order.
- **The piano roll finds its notes.** When the channel has notes in the pattern but none is in view (octaves above
  or below, or further along), a round arrow at the note grid's edge points at the nearest one and names its key; a
  click scrolls it into view. When notes also lie the opposite way, a second, smaller grey arrow points at the nearest
  of those.
- **Plugin editors in the app's window (Linux).** The plugins compiled into Lemon Studio (the stock instruments and
  effects) draw their editors inside Lemon Studio's own window, as floating windows like the instrument rack: no X11
  window, no XWayland, no OpenGL. Their menus and value boxes are drawn above the app; knob drags hold the cursor as the
  app's own knobs do; the close shortcut closes them. In the Lemon skin their window is a card like the channel
  rack's, in the theme's colours: rounded, the editor edge to edge under one title row (menu arrow, name, page tabs,
  Presets, close). FL's frame is gone there: no minimize button, no gear strip (the channel settings window has those
  controls), no frame around the editor. Third-party plugins keep windows of their own.
  `LEMONSTUDIO_PLUGIN_WINDOWS=1` gives the built-in ones their own windows again.
- **Tiling desktops leave the window to the window manager.** On a tiling desktop (Hyprland, sway, i3, niri, river
  and the like, read from `XDG_CURRENT_DESKTOP`), the tiling layout's toolbar has no minimize, maximize and close
  buttons: the window manager places, sizes and closes the window with its own keys. The flexible spaces take their
  room. A window made floating gets them back (on Hyprland, which says so). The Classic layout and the toolbar editor
  keep them.
- **Follow Omarchy's theme.** On Omarchy, the theme follows the system's: Omarchy's current theme (`colors.toml`) is
  applied over the skin in use, its backgrounds as the surfaces exactly, its accent as the selection, its foreground
  as the text, and it changes when the system theme changes. Theme ▸ Options ▸ "Follow Omarchy's theme" shows only
  on Omarchy and is on there by default; choosing another theme turns it off. A theme file can give its surfaces
  outright with `LemonStudioSurfaces=#darker,#dark,#background,#lighter` (FL ignores the key).
- **Stock plugins follow the theme.** With the Lemon skin, the editors of the plugins compiled into Lemon Studio
  (drawn in the app's window) take the applied theme's colours too: Lemon's greys become its surfaces, light greys its
  text, the lemon yellows its selection; signal colours stay. Theme ▸ Options ▸ "Stock plugins follow the theme", on
  by default.
- **Closing costs nothing: the session comes back.** General settings ▸ Miscellaneous ▸ Startup project is on "Last
  used project" by default, and a start puts back the windows that were open in it when the app closed (the arranger,
  mixer, piano roll, instrument rack, channel window, tool windows and plugin windows, back to front; each plugin's
  window once its plugin has loaded). Linux: the audio stream is named "Lemon Studio" in the sound server, not
  "PipeWire ALSA [lemonstudio]" (unless `PIPEWIRE_ALSA` is set).
- **A media player to the desktop (MPRIS).** Lemon Studio is on the session bus as `org.mpris.MediaPlayer2.lemonstudio`:
  media keys, a bar's player widget and `playerctl` play, pause and stop the transport, and show whether it plays and
  the project's name.
- **Third-party plugin windows under Hyprland.** They drag by their own title bar (Hyprland ignores the X11 move
  request) and have no compositor border, shadow or move animation.
- **Branch marks.** The workspace watermark, the version string ("cutting-edge pre-alpha") and the status bar's mark
  are red.

## What this branch removes

Nothing yet.
