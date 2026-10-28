# The Lemon project file (`.lemon`)

Lemon Studio's own project file. Save as offers it ("Lemon project, with its history"); Open and a drop onto the
window open it. A `.flp` is still saved and opened as before.

## What it holds

A zip with three files:

| File | What |
|---|---|
| `lemon.json` | `format: "lemon"`, `version` (1), `app` (the version that saved it), `saved` (Unix ms), `snapshot` and `journal` (the names below), `entries` |
| `project.flp` | The project as it is now, in FL's layout, byte for byte what saving it as a `.flp` writes. Taken out of the zip, FL opens it. |
| `journal.jsonl` | The project's whole history: the journal's entries ([control-plane.md](control-plane.md) "Journal") from every session it was edited in, oldest first, each session starting with its `session` entry |

A file of a newer `version` is not opened; one without a journal (or with a damaged line in it) opens with no
history.

## What it gives

- **Nothing made is lost between sessions.** The history goes with the file, not with the computer: every state each
  plugin had in it, in any session, is in Recover from journal for as long as the file exists (session journals are
  kept 30 days; the file keeps them for good).
- **The project can be read back.** What changed, when and by whom (the user, or a client of the control plane by
  name), down to each note added and removed: a client opening the file sees how the project came to be, not only
  what it is.
- **A base for more:** going back to any earlier point by replaying the journal over the snapshot, undo that survives
  closing the project, and branches of one project.

## Not yet

- Undo history does not survive closing the project; the journal says what each step did, but steps are not rebuilt
  from it.
- Ids (`os_model::Id`) are not stored, so the journal's ids name things only within their own session.
- The snapshot is in FL's layout, so it holds only what an FLP holds; what Lemon Studio adds on top (anything not in
  an FLP) is not saved yet.
- Samples are not inside the file (FILE ▸ Export ▸ Zipped project still gathers them).
- Autosaves of a Lemon project are `.flp` files.
