# The Lemon project file (`.lemon`)

Lemon Studio's own project file, and what a new project is saved as (Save as's first choice, "Lemon project, with its
history"; the Save as form names `<name>.lemon`). Open and a drop onto the window open it. A project opened from a
`.flp` keeps saving as one (Save new version and renaming keep the file's type), and Save as offers "FL Studio project"
too.

> Like everything on this branch, the format may change or be dropped. Its `project.flp` is the way out: taken out of
> the zip, it opens in Lemon Studio's `main` and in FL.

## What it holds

A zip with these files:

| File | What |
|---|---|
| `lemon.json` | `format: "lemon"`, `version` (1), `app` (the version that saved it), `saved` (Unix ms), `snapshot` and `journal` (the names below), `entries` |
| `project.flp` | The project as it is now, in FL's layout, byte for byte what saving it as a `.flp` writes. Taken out of the zip, FL opens it. |
| `journal.jsonl` | The project's whole history: the journal's entries ([journal.md](journal.md)) from every session it was edited in, oldest first, each session starting with its `session` entry |
| `ids.json` | Each channel's, pattern's, mixer track's, arrangement's, clip's and slot plugin's id, with what finds the thing again (index, number, place), and the next id to give out |
| `history.json` | The undo history: each step's name and what it puts back, and how many are done |
| `versions.json`, `versions/<saved>.flp` | Every earlier save's snapshot, and when it was saved: each save keeps the one before it (unless nothing changed) |

A file of a newer `version` is not opened; one without a journal (or with a damaged line in it) opens with no
history.

## What it gives

- **Nothing made is lost between sessions.** The history goes with the file, not with the computer: every state each
  plugin had in it, in any session, is in Recover from journal for as long as the file exists (session journals are
  kept 30 days; the file keeps them for good).
- **The project can be read back.** What changed, when and by whom (the user, or a client of the control plane by
  name), down to each note added and removed: a client opening the file sees how the project came to be, not only
  what it is.
- **Undo goes on past closing.** Opened again, Ctrl+Z takes back the steps made before the save, and redo gives them
  again, as if the project had never closed. The browser's History lists them.
- **Any earlier save can come back.** The browser's Current project ▸ Versions lists the file's earlier saves by when
  they were made; one clicked (after the unsaved-changes question) becomes the project, in the same file and with the
  same history and ids, unsaved until saved, and that save keeps the newest as a version too. Revert to last backup
  keeps a Lemon project's history and ids the same way.
- **Backups are Lemon project files too.** A Lemon project's autosaves and overwritten copies are `.lemon` files with
  the undo history, ids and journal as they were then; Revert to last backup (and the backup offered after a crash)
  brings those back with it.
- **The same things keep the same ids** from session to session, so the journal and the control plane's clients name
  them alike over the project's whole life, and an id that is gone is never given to something else.

## Not yet

- A plugin instance an undone step brings back (a deleted channel) starts from the state the step holds, which may be
  older than the instance's last state in that session.
- The snapshot is in FL's layout, so it holds only what an FLP holds; what Lemon Studio adds on top (anything not in
  an FLP) is not saved yet.
- Samples are not inside the file (FILE ▸ Export ▸ Zipped project still gathers them).
