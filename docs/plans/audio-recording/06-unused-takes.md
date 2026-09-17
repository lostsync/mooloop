# 06 — Deleting takes nobody used

Adam, 2026-09-17: the recordings folder is fine, but there has to be a way to
delete takes that were never used. Every take is kept (step 03 writes each one
to a file), so an evening of retakes leaves a pile of files that nothing
refers to.

## Where unused takes build up

1. **The shared recordings folder** (`<data dir>/recordings/`). This holds
   takes made before the project they belong to was saved, and takes that were
   replaced before a save.
2. **A project's own `recordings/` assets folder.** A take moved there by one
   save and replaced before the next one is still in the folder, but the song
   no longer uses it.

## What counts as "referenced"

A file is still in use if any of these point at it:

- **the open project's current state**, meaning any channel's sample;
- **its undo and redo history** (`ProjectSnapshot.samples` and the sample
  paths in snapshots). Undoing back to a take must still find its file.
- **for the project assets folder, the project as saved on disk**, meaning the
  saved file's sample references. The in-memory project may be unsaved.

A file in the shared folder that is not referenced by the open project could
still belong to **another song that was never saved**, but only if the app
crashed during that session. There is one app instance, and step 04 moves a
take into its project on save. So the shared folder only ever holds this
session's takes, plus leftovers from a crash. The crash case is exactly the one
where deleting silently would be wrong.

## Build

- **One command, `recording.clean-up`** (register it in `ACTIONS.md`), under
  File or wherever `ACTIONS.md` puts project housekeeping. It opens a dialog
  with two lists:
  - **Not used by this song:** unreferenced files in this project's assets
    folder, and unreferenced files in the shared folder made during this
    session.
  - **Left from earlier sessions:** shared-folder files older than this
    session. These are only there after a crash; the dialog says so and they
    are unticked by default.

  Each row shows the name, length, size and date. **Nothing is deleted
  without that confirmation**, and the dialog shows the total space it will
  free.
- **Deleting moves files to the desktop trash** (freedesktop Trash on Linux,
  the Finder trash on macOS), not a permanent delete. Use a crate that does
  this on both platforms, so a mistake can be recovered.
- **Quitting:** on a clean quit, shared-folder takes from this session that no
  saved song refers to are listed in the quit prompt next to "unsaved
  changes". Quitting without saving offers to move them to the trash. That
  handles the common case without ever opening the dialog.
- **Undo history:** clearing it (a project close) is the moment takes only
  history refers to become unused. The quit prompt covers that; there is no
  other automatic deletion.

## Test

- **Referenced files:** a take referenced only by the undo history is not
  offered for deletion. After the history is cleared, it is.
- **Moved takes:** a take moved into the assets folder by a save, then
  replaced and saved again, is offered in "not used by this song".
- **Crash leftovers:** a shared-folder file older than the session is listed
  under earlier sessions and is unticked by default.
- **Trash:** confirming deletes nothing permanently; the file shows up in the
  trash. Test this through a trash implementation the test can substitute.

## Docs

- `ACTIONS.md`: the command.
- `PROJECT_FORMAT.md`: the `recordings/` assets folder.
- `CURRENT.md`: the dialog and the quit prompt.
