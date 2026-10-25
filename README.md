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
- **Resizable plugin windows (Linux).** A detached plugin window whose editor lets the host resize it (CLAP
  `gui.can_resize`, VST3 `IPlugView::canResize`) can be resized by dragging its border: the editor is asked for the
  new size and the window follows the size it takes, one request per UI tick. Such an editor larger than the
  monitor's work area opens shrunk to fit it; editors that cannot be resized open as before.

- **Local built-in plugins.** CLAP files in `~/.config/Lemon Studio/builtin` (or `LEMONSTUDIO_BUILTIN_DIR`) load at
  startup and are listed with the built-in plugins: no scan, no format to choose. Projects save them as CLAP plugins.

## What this branch removes

Nothing yet.
