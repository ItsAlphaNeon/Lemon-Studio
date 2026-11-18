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
| `edit` | everything, including `apply`, `begin`, `end`, `accept`, `discard`, `undo`, `redo` and `save` |

`save` writes a file, an effect outside the project: with a window the user is asked first, and the answer only
says that they were. Without a window (`--serve`) the client saves directly.

## Methods

| Method | Params | Result |
|---|---|---|
| `hello` | `name`, `token` (on a port) | `app`, `version`, `protocol` (1), `name`, `level`. The name labels the client's undo steps and events. |
| `state` | | The project: settings, `channels`, `mixer` (the tracks in use), `patterns`, `playlist` |
| `notes` | `pattern` | That pattern's notes, with their ids |
| `plugins` | | The `instruments` and `effects` that `add_channel` and `add_effect` take, by name |
| `params` | `owner` | The parameters of the plugin playing a channel or a mixer slot: id, name, group, unit, value 0..1, value text, range, default, steps |
| `history` | | The undo history (`steps` with `name`, `undone`) and `at`, where the project is in it |
| `events` | `since` | What changed after event `since` (0: all kept), and `last`, the newest event's number |
| `apply` | `commands` (or `command`), `name`, `gesture` | Each command's `results`. The list is one undo step named `name` (else the command's own name), applied whole or not at all. With `gesture`, it joins that gesture's step |
| `begin` | `name` | `gesture` (its number): the lists applied with it until `end` make one undo step named `name`, as a fader drag does |
| `end` | `gesture` | Nothing. The next list is a step of its own |
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
names nothing, until an undo brings the thing back with its id. Notes, control points, time markers, playlist tracks
and automation points have ids too, numbered from 2147483648 up so the others stay small; `notes` shows a pattern's.
No two things share an id. Ids are not saved in project files, so an opened project gets new ones.

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
| `set_project` | `beats` (1..=32), `steps` (1..=16), `signature`, `panning_law` (`circular` or `triangular`), `play_truncated`, `pdc` (all optional) | Settings > Project: the bar, shown as a time signature or a division, the panning law, whether notes a pattern clip cuts off play, and automatic plugin delay compensation. A bar value the project already has is kept even out of range |
| `add_channel` | `name`, `plugin`, `sample`, `track`, `kind` (`"audio_clip"`, `"automation"`, `"layer"`), `color`, `target`, `points`, `children` (all optional) | A channel of `kind` has no plugin: an automation clip with `points` (as `set_points` takes them) driving `target`, a layer playing `children` (channels' ids). A Lemon Sampler channel (with `sample` loaded), or one played by generator `plugin`, on mixer track `track`. A MIDI Out channel (`"MIDI Out"`) goes on the master track; `"Lemon Container"` makes an empty container channel. Result: `channel` |
| `add_audio_clip` | `path`, `track`, `position`; `mixer` (optional) | An audio file as a clip on a new audio clip channel (on an instrument track, the next free track below). Result: `channel`, `clip` |
| `set_auto_crossfades` | `on` | Automatic crossfades: on, overlapping audio clips crossfade; off, the crossfades they made go |
| `delete_channel` | `channel` | Its notes, clips and automation go too |
| `move_channel` | `channel`, `row` | 0 = top |
| `replace_instrument` | `channel`; `plugin` (optional: none is Lemon Sampler) | As the rack's Replace: the channel keeps its row, notes, mixer track, levels and Misc settings and takes the plugin's name (a MIDI Out channel keeps its own) and a new instance; it gets a new id, so an undo brings the old instrument back. Result: `channel` (the new id) |
| `clone_channel` | `channel`; `at_end` (optional) | A copy right below it (last in the rack with `at_end`), as the rack's Clone: "<name> #2", its settings, mixer track and a copy of its instrument, no notes. Result: `channel` |
| `set_channel` | `channel`; `name`, `color`, `volume` (0..1), `pan` (-1..1), `pitch` (cents), `enabled`, `track`, `declick` (0..5), `swing` (0..1), `truncate_swing`, `cut`, `cut_by` (groups, 0 = none), `locked`, `stretch` (a sampler's time stretch method), `zipped`, `auto_bypass`, `threaded` (optional) | A colour's parts are kept at least `14` (hex), as projects save them. Swing is kept in steps of 1/128. An undo of another step leaves `locked` and `zipped` as they are |
| `load_sample` | `channel`, `path` | A sampler channel; an empty path clears it |
| `add_track` | `at` (optional) | A mixer insert, at number `at` or after the last. Result: `track` |
| `delete_track` | `track` | Later tracks move left; its channels go to the master |
| `set_track` | `track`; `name`, `color`, `volume` (0..1, 0.8 = unity), `pan`, `stereo` (-1..1), `enabled`, `polarity`, `swap`, `effects_enabled`, `separator`, `dock` (`left`, `middle` or `right`), `threaded`, `latency_in`, `latency_out` (ms, -1000..1000), `audio_in`, `audio_out` (place in the device's input or output menu, -1 none), `rec_source` (0 External input only ..= 5 Post track), `monitor` (`off`, `armed` or `on`), `armed` (optional) | |
| `move_track` | `track`, `to` | the track moves to insert number `to`, the ones between moving over |
| `remove_tracks` | `tracks` (ids) | the tracks go and the mixer gets shorter (Trim unused tracks); the others close up in order |
| `load_track_state` | `track`, `path`; `named` (optional) | the saved mixer track state in `path` onto the track, its old effects going; `named`: with its name and colour |
| `move_effect` | `track`, `slot`, `to` | the effects in the two slots change places, with what is linked to them |
| `reset_track` | `track` | back to a new track's settings, routed to the master; its effects stay |
| `set_auto_bypass` | `on` | Auto-bypass when silent for every plugin: the slots', the plugin channels' and the containers' members' |
| `set_send` | `from`, `to`, `level` (optional) | `level` 0..1 routes `from` to `to` (0 = sidechain only); none removes the route |
| `set_eq` | `track`, `band` (0..2); `gain` (-1800..1800, 0.01 dB), `frequency`, `width` (0..65536) (optional) | The track's three-band equaliser, as stored |
| `add_effect` | `track`, `plugin`, `slot` (optional) | Into slot 0..9, else the first empty one; `"Lemon Container"` puts an empty container there. Result: `slot` (the slot plugin's id) |
| `remove_effect` | `track`, `slot` | with the name and colour given to the slot |
| `set_slot` | `track`, `slot`; `enabled`, `mix` (0..1), `name` (empty: the plugin's), `color` (`#rrggbb`, empty: none), `sidechain` (an input port, 0: none), `auto_bypass`, `threaded` (optional) | The sidechain input is for a plugin that has one |
| `add_pattern` | `name`, `color`, `number` (optional) | Result: `pattern` |
| `delete_pattern` | `pattern` | Its clips go; later patterns move down a number |
| `shift_patterns` | `from`, `by` (1 or -1) | patterns from `from` on move by one: 1 opens a gap at `from` (Insert one), -1 closes the empty number below `from` |
| `move_pattern` | `from`, `to` (numbers) | pattern number `from` takes number `to` and the one there `from`, with their clips (by number: a number can have clips before the project holds a pattern for it) |
| `clone_pattern` | `pattern` | a copy right after it named "<name> #k"; later ones move up. Result: `pattern` |
| `copy_pattern` | `from`, `to` (numbers); `name` (optional) | A copy of pattern `from` (empty when the project holds none with that number) as pattern `to`, which none has; nothing else moves. Result: `pattern` |
| `transpose_pattern` | `pattern`, `semitones`; `channels` (optional) | every note of the pattern, or only those channels' notes |
| `split_pattern` | `pattern` | Split by channel: a pattern per channel with notes, clips under the original's |
| `set_pattern_signature` | `pattern`, `numerator`, `denominator` | the time signature at the pattern's start, `numerator` 0 for none; its clips follow |
| `set_pattern` | `pattern`; `name`, `color`, `length` (0 = Auto) (optional) | |
| `set_notes` | `pattern`, `notes`, `channel` (optional) | The notes of `channel` (or all notes) become `notes`. A note keeps its `id` (as `notes` lists it) when the pattern had it, and with it its values not given; others get new ones. Result: `notes` (their ids, in order) |
| `set_groups` | `groups`; `channels` (`[{"channel", "group"}]`), `shown` (optional) | The rack's filter groups: their names in file order ("Unsorted" among them), channels' groups (an index, `null` none), the filter shown (`null` All) |
| `set_loops` | `pattern`; `mode` (0 off, 1 loop step channels, 2 loop all channels, 3 advanced looping), `loops` (`[{"channel", "value"}]`) (optional) | The step sequencer's loops; a value is a length in steps or a loop point as the file keeps it, `null` none |
| `set_step` | `pattern`, `channel`, `step`, `on`; `mute` (optional) | A step sequencer click: on adds a step (or unmutes the step's notes), off removes the step's notes, or mutes them with `mute` |
| `set_controls` | `pattern`, `controls`: `[{"position", "value", "channel" and "param" (0 volume, 1 pan, 4 pitch) or "kind", "id"}]` | The pattern's control events (volume and pan 0..12800, pitch in cents), in this order |
| `add_notes` | `pattern`, `notes` (without `id`) | Result: `notes` (their ids, in order) |
| `add_clip` | `track` (1-based), `position`; `length`, and `pattern` or `channel` | A pattern clip, or an audio or automation channel's clip, in the arrangement shown. Result: `clip` |
| `delete_clip` | `clip` | |
| `set_clip` | `clip`; `position`, `track`, `length`, `muted` (optional) | |
| `set_playlist_track` | `track`; `name`, `color`, `enabled`, `grouped`, `motion`, `press`, `trigger_sync`, `position_sync`, `queue`, `tolerant`, `clip_lock` (`{"pattern"}` (a number) or `{"channel"}`, `{}` none) (optional) | `grouped`: with the track above (not track 1). The performance settings as the project numbers them |
| `set_points` | `channel`, `points`: `[{"beat", "value"; "tension", "mode", "flag", "id"}]` | The automation clip's points (beats from its start, values 0..1, at least two); a point named by its id keeps what is not given |
| `add_arrangement` | `name`, `copy` (an arrangement's id) (optional) | After the others; a copy of `copy`, else empty. Result: `arrangement` |
| `delete_arrangement` | `arrangement` | A project keeps one; the one shown going, the one before it is shown |
| `set_arrangement` | `arrangement`, `name` | |
| `set_clips` | `clips`: `[{"track", "position", "length", "pattern" (a number) or "channel", "muted", "window" ([start, end], [] none), "record", "id"}]` | The shown arrangement's clips become these, in this order (the order they are drawn in; a saved file keeps them in position order). A clip named by the id of one the playlist holds keeps it and its values not given; the others are new |
| `set_markers` | `markers`: `[` a marker as `add_marker` takes one, with its `id` `]` | The markers become these (in position order), kept by id as `set_clips` keeps clips |
| `add_marker` | `marker`: `position`; `name`, `kind`, `numerator`, `denominator`, `root`, `scale` (optional) | A time marker (`kind`: 0 marker, 1 marker loop, 2 skip, 3 pause, 4 loop, 5 start, 8 time signature, 9 punch in, 10 punch out, 12 key; a key marker by `root` 0..11 and `scale`, a place in the Add key marker list). Result: `marker` |
| `set_marker` | `marker`; `position`, `name`, `kind`, `numerator`, `denominator` (optional) | |
| `delete_marker` | `marker` | |
| `move_content` | `marker`, `right` | The marker's section trades places with the next one (else the previous one), with its clips and markers |
| `swap_playlist_tracks` | `track`, `with` | The two tracks change places, with their clips |
| `insert_playlist_track` | `at` | An empty track; the ones from there move down |
| `delete_playlist_track` | `track` | With its clips; the ones below move up |
| `reset_playlist_track` | `track` | A new track's settings; its clips stay |
| `set_clip_lfo` | `channel`; `on`, `add`, `amount` (0..128), `speed` (0..65536, 45819 = a cycle a beat), `tension`, `skew`, `pulse_width` (-128..128) (optional) | The LFO of an automation clip's channel, as stored |
| `set_midi_out` | `channel`; `midi_channel` (0..15), `port`, `patch` (0..128, 0 = none), `bank`, `bank2` (-1..127, -1 = none), `send_levels`, `map_color` (optional) | A MIDI Out channel's settings |
| `set_member` | `container`, `member`; `wet`, `dry` (0..1), `name`, `color`, `icon` (a name from Change icon...), `auto_bypass`, `threaded` (optional) | A container's member levels; `container` is the id of the mixer slot or channel holding it |
| `wrap_effects` | `track`, `slot`; `all` (optional) | Wrap in container (`all`: Wrap all in container): the effects become members of a container in that slot, with their states; automation, recorded events and links on their parameters follow them. Result: `message` |
| `wrap_instrument` | `channel` | Wrap in container on the channel's instrument; the channel takes a new id. Result: `channel`, `message` |
| `move_member` | `container`, `member`, `to` | To place `to` (0 = first) of the container's list |
| `remove_member` | `container`, `member` | |
| `add_member` | `container`, `plugin`; `instrument` (optional) | At the end, from the plugin's defaults (`plugin`: its identity or name; an instrument in a channel's container only). Result: `member` |
| `replace_member` | `container`, `member`, `plugin`; `instrument` (optional) | The member keeps its place, id and levels |
| `set_param` | `owner`, `param`, `value` | A plugin parameter (`params` lists them) to `value` 0..1 |

A note is `{"channel", "position", "length", "key", "velocity", "pan"}`: `key` is a MIDI number (60 = C5, the
default), `velocity` 0..128 (100), `pan` 0..128 (64 = centre). Optional too: `fine_pitch` 0..240 (120 = none),
`release`, `mod_x`, `mod_y` 0..128, `repeat` 0..14 (0 = off), `slide`, `muted`, `porta`, `color` (a colour group 0..15), `raw` (the stored record's other bits, `[flags, byte 19, bytes 14-15]`, kept for a copy; the values above go over them).

## Undo, proposals and events

- Every `apply` is one step in the project's undo history, named after the client ("Beat bot: build a beat"). The
  user can undo it like any other step, and Ctrl+Z after it works as usual.
- A gesture (`begin`, `apply` with `gesture`, `end`) is one step however many lists it applies: its undo puts back
  what each thing was before the gesture. After another step, an undo or a redo, the gesture goes on in a new step.
- An add's undo removes what it added (a channel, mixer track or pattern) by its id, and its redo puts it back with
  the same id; a move's undo moves the channel back. Nothing else changes with them. A delete's undo puts the
  project back as it was.
- The interface's own edits use the same path: mixer edits (faders dragged or typed, polarity, routing,
  colours, names, effect slots and their sidechain input, adding, moving and deleting tracks), channels added (the
  plugin picker, F8, ADD, a browser drop), coloured, named, cloned, moved and deleted, FILE ▸ Import ▸ MIDI file into
  the open project, and pattern colours, names, inserting, cloning, moving and deleting are undo steps too, as are the
  tempo, the project settings and plugin parameters, whether Lemon Studio sets them (Type in value, Set..., Paste
  value) or they change in the plugin's own window. Faders, parameters, the tempo and the channel's own controls follow
  "Undo knob tweaks". A change has the
  same step name whichever made it ("Tempo", "Time signature", "Song info", "Plugin parameter").
- So are the channel's own controls in a plugin window (volume, panning, pitch, pitch range, mixer track), the
  channel rack's mixer-track box, a pattern's time signature, arrangements added, cloned and deleted, arrangement,
  pattern and effect-slot names, effect-slot colours, a clip's random colour and "Remove all audio editors". Each drag
  of a box or knob is one step, and so is a run of wheel notches on one: a click, or a pause of a second, starts a
  new one. An arrangement added and named at once is one step. The pattern length box, the Misc tab, channel
  groups and Performance mode make none, as in FL. Advanced looping switched on turns loop mode on with it.
- A channel muted, soloed or switched on or off is one step of its own ("Channel mute / solo") that follows "Undo knob
  tweaks", and a mixer track's mute and solo make none, as in FL. An undo or redo of any other step leaves every
  channel and track muted or not as it is (a client's `enabled` too); the undo of the step that set a client's
  `enabled` takes it back.
- The mixer tracks commands name are the ones `project` lists: not the mixer's hidden last track.
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
