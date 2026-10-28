# The control plane

Scripts, tests, controllers and AI assistants can read and change a Lemon Studio project through the control
plane. Every change goes through one entry point, so each one is an undo step, named after the client that made it,
and clients can be limited to reading or to proposing changes that the user accepts.

The protocol is [JSON-RPC 2.0](https://www.jsonrpc.org/specification) over a local socket, one request per line and
one answer per line. Nothing in it is specific to one assistant or vendor: any client that speaks it can drive the
app.

The design is in the specs repository's `roadmap/control-plane.md`.

## Starting it

| Flag | What it does |
|---|---|
| `--control <socket>` | The app listens beside its window. `<socket>` is a Unix socket path (Linux, macOS) or a port number (any platform; Windows takes ports only). The hint bar says where it listens. |
| `--serve <socket> [project.flp]` | No window: the app opens the project (or a new one) and answers clients until the process ends. |
| `--control-level read\|propose\|edit` | What clients may do; `edit` by default. |
| `--control-token <token>` | The token clients give in `hello`. A port gets a new random one each run without it (the hint bar and the terminal show it); a socket path needs none unless one is given. |

A socket path is made readable and writable by your user only. A port listens on 127.0.0.1, so only this computer
can connect, but every program on it can, so on a port a client must first give the token in `hello`; nothing else is
answered before that. A connection that sends a line that isn't JSON (a web page posting to the port) gets one parse
error and is closed. On Linux and macOS prefer a socket path. Port `0` picks a free one.

```bash
lemonstudio --control /tmp/lemon.sock song.flp
lemonstudio --serve 7777 --control-level propose song.flp
```

## Levels

| Level | Allows |
|---|---|
| `read` | `hello`, `state`, `notes`, `plugins`, `params`, `history`, `events`, `proposals` |
| `propose` | the above, and `propose`: the app asks the user to Apply or Discard each proposal |
| `edit` | everything, including `apply`, `accept`, `discard`, `undo`, `redo` and `save` |

`save` writes a file, an effect outside the project: with a window the user is asked first, and the answer only
says that they were. Without a window (`--serve`) the client saves directly.

## Methods

| Method | Params | Result |
|---|---|---|
| `hello` | `name`, `token` (on a port) | `app`, `version`, `protocol` (1), `name`, `level`. The name labels the client's undo steps and events. |
| `state` | | The project: settings, `channels`, `mixer` (the tracks in use), `patterns`, `playlist` |
| `notes` | `pattern` | That pattern's notes |
| `plugins` | | The `instruments` and `effects` that `add_channel` and `add_effect` take, by name |
| `params` | `owner` | The parameters of the plugin playing a channel or a mixer slot: id, name, group, unit, value 0..1, value text, range, default, steps |
| `history` | | The undo history (`steps` with `name`, `undone`) and `at`, where the project is in it |
| `events` | `since` | What changed after event `since` (0: all kept), and `last`, the newest event's number |
| `apply` | `commands` (or `command`), `name` | Each command's `results`. The list is one undo step named `name` (else the command's own name), applied whole or not at all |
| `propose` | `commands`, `name` | `proposal` (its number) and `changes` (what it would change). The project is not changed |
| `proposals` | | The proposals waiting |
| `accept`, `discard` | `proposal` | The commands' `results`; nothing |
| `undo`, `redo` | | The `step` undone or redone (`null`: none) |
| `save` | `path` (else the project's file) | `saved` or `asked`, with the path |

Errors use JSON-RPC's codes (-32700 parse error, -32600 invalid request, -32601 no such method, -32602 invalid
params) and two of their own: -32000 when a command fails (an id that names nothing, a value out of range; nothing is
changed) and -32001 when the client's level does not allow the method. A request without an `id` is a notification
and gets no answer.

## Ids

Channels, patterns, mixer tracks, playlist clips, arrangements and mixer-slot plugins have ids: numbers that stay
with them when other things are added, moved or deleted. `state` shows them; commands take them. A deleted thing's id
names nothing, until an undo brings the thing back with its id. Ids are not saved in project files, so an opened
project gets new ones.

## Commands

Each command is an object with its `op` and its fields. Fields marked optional can be left out; settings commands
change only the fields they are given. Positions and lengths are ticks (`state.ppq` per quarter note). Colours are
`#rrggbb`, and an empty string means the default colour (or the default name).

| op | Fields | Notes |
|---|---|---|
| `set_tempo` | `bpm` | 10 to 522 |
| `set_swing` | `swing` | 0 to 1 |
| `set_main_volume` | `volume` | 0 to 1, 0.8 = unity |
| `set_main_pitch` | `cents` | -1200 to 1200 |
| `set_time_signature` | `numerator`, `denominator` | denominator 2, 4, 8 or 16 |
| `set_info` | `title`, `author`, `genre`, `comments`, `url` (all optional) | the Song info |
| `add_channel` | `name`, `plugin`, `sample`, `track` (all optional) | A Lemon Sampler channel (with `sample` loaded), or one played by generator `plugin`, on mixer track `track`. Result: `channel` |
| `delete_channel` | `channel` | Its notes, clips and automation go too |
| `move_channel` | `channel`, `row` | 0 = top |
| `set_channel` | `channel`; `name`, `color`, `volume` (0..1), `pan` (-1..1), `pitch` (cents), `enabled`, `track` (optional) | |
| `load_sample` | `channel`, `path` | A sampler channel; an empty path clears it |
| `add_track` | `at` (optional) | A mixer insert, at number `at` or after the last. Result: `track` |
| `delete_track` | `track` | Later tracks move left; its channels go to the master |
| `set_track` | `track`; `name`, `color`, `volume` (0..1, 0.8 = unity), `pan`, `stereo` (-1..1), `enabled`, `polarity`, `swap`, `effects_enabled` (optional) | |
| `set_send` | `from`, `to`, `level` (optional) | `level` 0..1 routes `from` to `to` (0 = sidechain only); none removes the route |
| `set_eq` | `track`, `band` (0..2); `gain` (-1800..1800, 0.01 dB), `frequency`, `width` (0..65536) (optional) | The track's three-band equaliser, as stored |
| `add_effect` | `track`, `plugin`, `slot` (optional) | Into slot 0..9, else the first empty one. Result: `slot` (the slot plugin's id) |
| `remove_effect` | `track`, `slot` | |
| `set_slot` | `track`, `slot`; `enabled`, `mix` (0..1) (optional) | |
| `add_pattern` | `name`, `color` (optional) | Result: `pattern` |
| `delete_pattern` | `pattern` | Its clips go; later patterns move down a number |
| `set_pattern` | `pattern`; `name`, `color`, `length` (0 = Auto) (optional) | |
| `set_notes` | `pattern`, `notes`, `channel` (optional) | The notes of `channel` (or all notes) become `notes` |
| `add_notes` | `pattern`, `notes` | |
| `add_clip` | `track` (1-based), `position`; `length`, and `pattern` or `channel` | A pattern clip, or an audio or automation channel's clip, in the arrangement shown. Result: `clip` |
| `delete_clip` | `clip` | |
| `set_clip` | `clip`; `position`, `track`, `length`, `muted` (optional) | |
| `set_playlist_track` | `track`; `name`, `color`, `enabled` (optional) | |
| `set_param` | `owner`, `param`, `value` | A plugin parameter (`params` lists them) to `value` 0..1 |

A note is `{"channel", "position", "length", "key", "velocity", "pan"}`: `key` is a MIDI number (60 = C5, the
default), `velocity` 0..128 (100), `pan` 0..128 (64 = centre).

## Undo, proposals and events

- Every `apply` is one step in the project's undo history, named after the client ("Beat bot: build a beat"). The
  user can undo it like any other step, and Ctrl+Z after it works as usual.
- The interface's own edits use the same path: mixer edits (faders dragged or typed, mute, polarity, routing,
  colours, names, effect slots and their sidechain input, adding, moving and deleting tracks), channels added (the
  plugin picker, F8, ADD, a browser drop), coloured, named, cloned, moved and deleted, FILE ▸ Import ▸ MIDI file into
  the open project, and pattern colours, names, inserting, cloning, moving and deleting are undo steps too, as are the
  tempo, the project settings and plugin parameters, whether Lemon Studio sets them (Type in value, Set..., Paste
  value) or they change in the plugin's own window. Faders and parameters follow "Undo knob tweaks". A change has the
  same step name whichever made it ("Tempo", "Time signature", "Song info", "Plugin parameter").
- So are the channel's own controls in a plugin window (on/off, volume, panning, pitch, pitch range, mixer track) and
  its Misc tab, channel mute and solo (the LED, the menu, MIDI), mixer solo, the channel rack's mixer-track box,
  the pattern length box, a pattern's time signature, arrangements added, cloned and deleted, arrangement, pattern
  and effect-slot names, a clip's random colour and "Remove all audio editors". A drag or a run of wheel notches on
  one box or knob is one step.
- An undone channel or effect keeps its plugin instance's state: a redo hosts it again from that state.
- A proposal is applied to see what it changes and taken back at once; with a window the app asks the user to Apply
  or Discard it. Accepted, it is one undo step named after its client.
- `events` lists each change: `apply`, `undo`, `redo`, `proposal`, `accepted`, `discarded`, `opened` (a project was
  opened or made), and `edit` for a change made in the interface. Clients poll it with the last number they saw.

## An example

```python
import json, socket
s = socket.socket(socket.AF_UNIX); s.connect("/tmp/lemon.sock"); f = s.makefile("rw")
def call(method, **params):
    f.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}) + "\n"); f.flush()
    return json.loads(f.readline())["result"]

call("hello", name="Beat bot")
r = call("apply", name="Build a beat", commands=[
    {"op": "set_tempo", "bpm": 92},
    {"op": "add_channel", "name": "Kick"},
    {"op": "add_pattern", "name": "Beat"},
])
kick, beat = r["results"][1]["channel"], r["results"][2]["pattern"]
call("apply", commands=[
    {"op": "set_notes", "pattern": beat, "notes": [{"channel": kick, "position": p, "length": 24} for p in (0, 96, 192, 288)]},
    {"op": "add_clip", "track": 1, "position": 0, "pattern": beat},
])
```

## Not yet

- Live commands that reach the playing engine without a rebuild (each change rebuilds it, as edits in the interface
  do); parameter changes go straight to the plugin.
- A native project format: projects are read and saved as FL Studio projects.
- Undo steps for: replacing a plugin's whole state (loading a preset, adding a Tesla), the graph editor, note fields
  edited in place (fine pitch, release, Mod X/Y, slide and porta flags), the Lemon Sampler editor's channel side,
  clip groups, and Make unique / audio clips added (partly undone). Locking, collapsing and loop lengths in the
  channel rack are view state and stay out of history.
- Commands for what the interface edits but clients can't yet: channel envelopes, groups, clip channels; note
  fields; mixer track moves, slot moves, sidechains and Tesla; clip offsets, groups, markers and playlist tracks;
  automation clips and points; project settings beyond tempo, signature and song info; plugin state and presets;
  sample tags.
- Rendering through the control plane, and pushing events to clients (they poll `events`). A parameter changed in a
  plugin's window is an undo step but not an `edit` event.
