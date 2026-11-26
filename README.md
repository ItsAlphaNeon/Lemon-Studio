# Lemon Studio: cutting-edge

This is the `cutting-edge` branch of Lemon Studio. It is [`main`](https://github.com/ItsAlphaNeon/Lemon-Studio/tree/main)
plus what is listed here. For everything else (building, using, contributing, license), read `main`'s README.

> **Anything here can be dropped at any moment.** This branch is where ideas are tried. A feature listed below may
> change or be removed without notice, and nothing here is promised to reach `main`. Don't build on it, and keep
> copies of what you make with it (`.lemon` files included).

> **Wayland first, not tested on every platform.** This branch is used daily on Linux under Wayland (Hyprland); X11
> still works (`OPENSTUDIO_WAYLAND=0`). Other platforms may work, but not everything added here is tested on each, and
> some of it is Linux-only (the Hyprland plugin-window handling, MPRIS, following Omarchy's theme).

`main` is merged into this branch with `tools/merge-main.sh`, which keeps this README and `docs/` to this branch's
own content. Side branches are named `cutting-edge-<topic>`.

## What this branch adds

- **A project journal.** Every edit, undo, save and plugin state is written to disk as it happens, saved or not, and
  readable through the control plane: a sound made in a plugin's window survives an unsaved project. See
  [docs/journal.md](docs/journal.md).
- **A converted older project keeps its original.** An FLP in the layout before 24.2 opens without its path, so Save
  asks for a new file.
- **After a crash, the backup is offered.** When the last run ended without closing while its project had unsaved
  changes, the next start says so and offers the newest autosave written since the project was last saved. Opening it
  works as Revert to last backup.

- **Wayland first.** On Linux the main window is native Wayland in a Wayland session; plugin windows open detached.
  `OPENSTUDIO_WAYLAND=0` brings back the X11 window, where plugin windows dock. CI runs on Linux on every push
  (`.github/workflows/cutting-edge.yml`); other platforms are not tested on every push.
- **Resizable plugin windows (Linux).** Besides Lemon's own resizable plugins, any plugin whose editor lets the host
  resize it (CLAP `gui.can_resize`, VST3 `IPlugView::canResize`) gets the frame's edges and maximize button. The
  editor is asked for the new size and the frame takes the size it accepts. Such an editor larger than the monitor's
  work area opens shrunk to fit it.
- **Tiling layout, the default.** General settings ▸ Display ▸ Layout: Tiling or Classic. The tiling layout has:
  - a toolbar of one row (menus, transport, time, pattern, main volume and pitch, windows, utilities);
  - a status bar along the bottom (Lemon's mark, the title and hint, global snap, mode buttons, pickers, CPU and
    memory): the dropouts since the program started show in a quiet colour, and in a red pill for a few seconds after
    one cuts into sound, marked `!` when a plugin arrived late (the audio log says why each happened), and the audio load turns yellow, orange and red as it climbs;
  - the workspace and the browser as rounded cards over a darker backdrop; the browser's splitter is the gap between
    them, and the mixer's routing cables hang over the gap below. With Animations from "Polished" up, the cables between
    tracks at most three apart (bus 3 into the master, say) hang on in front of the split window below the mixer;
    longer ones stay behind it.
  - split views instead of floating ones: the arranger, the piano roll and the mixer are maximized or snapped into a
    split of the workspace, each its own card. The status bar's split button picks the preset (top and bottom, or left
    and right); a title bar dragged onto another window swaps the two, the gaps drag to resize (a double click resets
    one), and windows keep a least size. F5, F7 and F9 show and hide their window on their own, and a window opening
    goes into the split (one maximized gives way). The toolbar's window buttons are lit while their window is open.
    There is no minimize; the maximize button shows a full square, or two panes to go back to the split, and a window
    alone has only its close button. The instrument rack, plugin and tool
    windows still float. In the Lemon skin the arranger, piano roll, mixer and browser have round buttons in their
    title bars; every other window, floating ones and plugin windows too, has a plain ×. With Animations from "Polished" up, windows glide to new places as Omarchy's Hyprland ones
    do; a window opening is at its place at once, under them as they move away, and one closing leaves its
    last look there for them to move over, and maximizing is instant.
  - the browser docked left or right: its title bar drags it to the other side.
  - window keys: split windows other than the one last in front are muted a little, fading as Omarchy's borders do
    (floating windows change nothing). Ctrl+Alt+arrows move the keyboard to the split window that way (floating windows
    are no stops; while one is maximized, the one that way is maximized instead), Ctrl+Alt+Shift+arrows swap split
    windows, F maximizes the window under the mouse or brings the split back, and Ctrl+Shift+F closes the window under
    the mouse (a plugin window with tabs: all of them). Shift+F, or the status bar's button
    next to the split button, slides the floating windows and plugin windows down out of sight and back up, together, as
    Omarchy's scratchpad does (Classic has the key). A floating window that opens meanwhile brings them back. The window manager's close shortcut
    closes the floating windows first, the one in front first (plugin windows, the instrument rack, the channel window,
    tool windows), whichever window has the keyboard; the app closes only when none is open.

  The tiling and Classic layouts each keep their own toolbar and, for the session, their own window arrangement.
  Every other keyboard shortcut works the same. Classic is FL's arrangement and unchanged.
- **Menu bar titles spread evenly** over its panel; where they do not fit with a gap between them, all eight are
  condensed one step further. Every menu keeps its place and order.
- **Status messages say what happened.** "The arranger is empty", not "The arranger is empty: nothing to play as a
  song"; no clause or parenthesis explaining why. Errors keep their cause, and a message that says how to go on with a
  mode just started stays.
- **The piano roll finds its notes.** When the channel has notes in the pattern but none is in view (octaves above
  or below, or further along), a round arrow at the note grid's edge points at the nearest one and names its key; a
  click scrolls it into view. When notes also lie past the opposite side of the grid (above and below, or left and
  right), a second, smaller grey arrow points at the nearest of those, in a narrow grid too. Notes on one key at the
  same time look stacked: a note lying over others gets a thin layer above it for each one under it, up to three, only
  over the stretch they share.
- **Plugin editors in the app's window (Linux and macOS).** The plugins compiled into Lemon Studio (the stock instruments and
  effects) draw their editors inside Lemon Studio's own window, as floating windows like the instrument rack: no X11
  window, no XWayland, no OpenGL. Their menus and value boxes are drawn above the app; knob drags hold the cursor as the
  app's own knobs do; the close shortcut closes them. In the Lemon skin their window is a card like the channel
  rack's, in the theme's colours: rounded, the editor edge to edge under one title row (menu arrow, name, page tabs,
  Presets, close). FL's frame is gone there: no minimize button, no gear strip (the channel settings window has those
  controls), no frame around the editor. Third-party plugins keep windows of their own.
  `LEMONSTUDIO_PLUGIN_WINDOWS=1` gives the built-in ones their own windows again.
- **Zest.** An analog-style synthesizer, compiled in on every processor (`plugins/zest`: SSE on x86-64, the same operations in plain Rust elsewhere). It aims at the sound of u-he Diva and loads Diva's presets when they are installed. Banks go in `plugins/zest/banks/` or `~/.local/share/zest/banks/` as `.zest` files.
- **Windows of one plugin as tabs (Linux and macOS).** A second window of a plugin drawn in the app's joins the first, with a row of
  tabs on top of the window: each narrows upwards between slanted edges, the window keeps its size (no row is added
  to it), and the row starts clear of its rounded corners. Each tab has its own close button, and the one left shows at
  once; the window's close closes them all. A tab dragged out of the row stands alone, to compare side by side. Held for a moment over another window of its plugin's title, or just below it, a dragged window shows in its own row of tabs the group it would make, its own tab outlined, and letting go there makes it where it was let go. General ▸ Miscellaneous "Show windows of
  one plugin as tabs" turns it off, and "Stack plugin tabs" lays them partly over each other.
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
  text, the lemon yellows its selection; signal colours stay. The edges of shapes blend into the recoloured surfaces,
  so a light theme leaves no dark fringes. Theme ▸ Options ▸ "Stock plugins follow the theme", on by default.
- **Closing costs nothing: the session comes back.** General settings ▸ Miscellaneous ▸ Startup project is on "Last
  used project" by default, and a start puts back the windows that were open in it when the app closed (the arranger,
  mixer, piano roll, instrument rack, channel window, tool windows and plugin windows, back to front; each plugin's
  window once its plugin has loaded).
- **A media player to the desktop (MPRIS).** Lemon Studio is on the session bus as `org.mpris.MediaPlayer2.lemonstudio`:
  media keys, a bar's player widget and `playerctl` play, pause and stop the transport, and show whether it plays and
  the project's name.
- **Novation Launchkey MK4, with no setup.** Plugged in, it goes into its DAW mode and works as FL Studio's script for
  it does (Novation's): transport, undo, metronome, loop record, capture and quantise buttons; pads that play and pick
  channels in the rack's colours, or drums; encoders on the focused plugin, the mixer (five pages) or the transport;
  faders and their buttons on the mixer bank (arm or mute), the master fader always; the screen naming what each
  control turns. In Plugin mode the encoder arrows page through all of a plugin's parameters, eight at a time (FL
  has no pages there), Zest has a map of its own (cutoff, resonance, filter envelope, ADSR, output), and a switch or
  choice moves a step per detent. The tracks and channels it takes are outlined in the mixer and the channel rack. Its DAW port is not a
  plain MIDI input; its keys still are, and its pitch wheel turns the selected channel's pitch knob. The Launchkey Mini
  MK4 uses the same driver.
- **Branch marks.** The workspace watermark, the version string ("cutting-edge pre-alpha") and the status bar's mark
  are red.

## What this branch removes

Nothing yet.
