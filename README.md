# Lemon Studio: cutting-edge

This is the `cutting-edge` branch of Lemon Studio. It is [`main`](https://github.com/ItsAlphaNeon/Lemon-Studio/tree/main)
plus what is listed here. For everything else (building, using, contributing, license), read `main`'s README.

`main` is merged into this branch with `tools/merge-main.sh`, which keeps this README and `docs/` to this branch's
own content. Side branches are named `cutting-edge-<topic>`.

## What this branch adds

- **Control plane.** Every edit goes through one command layer with undo. Projects have stable ids, and scripts and
  assistants can drive the app over a local socket (`--control`, `--serve`). See [docs/control-plane.md](docs/control-plane.md).
- **Older FL projects open.** Projects saved by FL 20 to 24.1 are converted to the newer layout as they are read.
  Save asks for a new file.

- **Wayland first.** On Linux the main window is native Wayland in a Wayland session; plugin windows open detached.
  `OPENSTUDIO_WAYLAND=0` brings back the X11 window, where plugin windows dock. CI runs on Linux only
  (`.github/workflows/cutting-edge.yml`); Windows and macOS are not tested on every push.
- **Resizable plugin windows (Linux).** Besides Lemon's own resizable plugins, any plugin whose editor lets the host
  resize it (CLAP `gui.can_resize`, VST3 `IPlugView::canResize`) gets the frame's edges and maximize button. The
  editor is asked for the new size and the frame takes the size it accepts. Such an editor larger than the monitor's
  work area opens shrunk to fit it. Detached plugin windows open centred on the monitor under the pointer.
- **Lemon layout, the default.** General settings ▸ Display ▸ Layout: Lemon or Classic. The Lemon layout has:
  - a toolbar of one row (menus, transport, time, pattern, main volume and pitch, windows, utilities);
  - a status bar along the bottom (Lemon's mark, the title and hint, global snap, mode buttons, pickers, CPU and
    memory);
  - the workspace and the browser as rounded cards over a darker backdrop; the browser's splitter is the gap between
    them, and the mixer's routing cables hang over the gap below.

  Every keyboard shortcut works the same. Classic is FL's arrangement and unchanged.
- **Branch marks.** The workspace watermark, the version string ("cutting-edge pre-alpha") and the status bar's mark
  are red.

## What this branch removes

Nothing yet.
