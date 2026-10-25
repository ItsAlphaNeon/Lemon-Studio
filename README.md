# Lemon Studio: cutting-edge

This is the `cutting-edge` branch of Lemon Studio. It is [`main`](https://github.com/ItsAlphaNeon/Lemon-Studio/tree/main)
plus what is listed here. For everything else (building, using, contributing, license), read `main`'s README.

`main` is merged into this branch regularly. Side branches are named `cutting-edge-<topic>`.

## What this branch adds

- **Control plane.** Every edit goes through one command layer with undo. Projects have stable ids, and scripts and
  assistants can drive the app over a local socket (`--control`, `--serve`). See [docs/control-plane.md](docs/control-plane.md).
- **Older FL projects open.** Projects saved by FL 20 to 24.1 are converted to the newer layout as they are read.
  Save asks for a new file.

## What this branch removes

Nothing yet.
