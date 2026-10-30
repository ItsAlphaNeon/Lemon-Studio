# The project journal

Main's [control plane](https://github.com/ItsAlphaNeon/Lemon-Studio/blob/main/docs/control-plane.md)
is the same here, plus what this page lists.

Everything that happens to the project is written as it happens, whether or not a client is connected and whether or
not the project is saved: one file per session, `Journal/<start time>-<pid>.jsonl` in the user folder
(`~/.config/Lemon Studio` on Linux), one JSON object per line, kept 30 days. Each has `seq`, `t` (Unix ms) and `kind`:

| `kind` | Fields | When |
|---|---|---|
| `session` | `version`, `pid` | The app started |
| `opened`, `new`, `saved` | `path` | A project was opened, made new or saved |
| `step`, `undo`, `redo` | `name`, `done` | An undo step was made (its name: who and what), taken back or given again |
| `changes` | `changes`: `path`, `before`, `after` | What the project's reading (`state`) changed, by id; at most four times a second |
| `notes` | `pattern`, `number`, `added`, `removed` | A pattern's notes added and removed, with every stored field (a moved note is one removed and one added) |
| `plugin_state` | `owner`, `at`, `plugin`, `size`, `hash`, `state` (base64) | A hosted plugin's whole state: after it reported a change while its window was open, as its window closes, and on save. Over 4 MB only `size` and `hash` |

A sound made in a plugin's window is in the journal even if the project is never saved. A plugin window's Presets box
has **Recover from journal**: the states that plugin had, in this session and earlier ones, newest first by time and
channel; choosing one puts it back. `journal` reads this session's entries back, `plugin_states` and `restore_state`
do what Recover from journal does.

## Control plane methods

| Method | Params | Result |
|---|---|---|
| `journal` | `since` | The journal's entries after `since` (0: all kept this session), `last`, and the journal's `file` |
| `plugin_states` | `owner` | The states that plugin had in the journals (this session's and earlier ones'), newest first: `file`, `seq`, `t`, `at`, `hash` |
| `restore_state` | `owner`, `seq`, `file` (else this session's) | Puts that state back into the plugin (edit level) |

## Undo

The Misc tab of a channel's plugin window and the instrument rack's pattern length box make undo steps here (a drag
or a run of wheel notches on one box or knob is one step); on main they make none.

## Not yet

- Ids, the undo history and the journal stay with a project only in a Lemon project file (`.lemon`,
  [native-format.md](native-format.md)); a project saved as an FLP keeps none of them.
- The journal has no automation points or event lanes yet, and nothing in the app replays it beyond plugin states.
