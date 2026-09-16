A pre-release. Nothing here is signed or notarized, so both desktop platforms
will warn before running it:

## Changes in v1.8.0

- Recovers what changed while Pathlight was closed: each watch keeps its last
  known state on disk and compares against it on the next start (Windows,
  Linux and the terminal; macOS already replayed from its FSEvents cursor).
  That state now lives on disk rather than in memory, so a background watch
  on a whole disk holds roughly none of it.
- Gap recovery on a whole disk works: a folder that cannot be read sets aside
  only what is under it instead of cancelling the whole comparison, files
  that grew under the same name are recovered, nothing already recorded is
  counted twice, and the walk never blocks the watch.
- The recording threshold and the reporting interval are editable while a
  watch runs, in every host, with the interface saying what each one drops.
  The default threshold stays at 1 KB; `0` records every change there is.
- Storage on macOS no longer deletes history it cannot judge: an unreadable
  budget or an undecodable row is kept, not treated as zero.
- "Last activity" reports the folder's newest change, not the newest row on
  the filtered page.
- Less memory for the same monitoring: journals are read a line at a time on
  macOS and each baseline path is stored in an allocation its own size.

- **macOS**: right-click `Pathlight.app` and choose Open, or
  `xattr -dr com.apple.quarantine Pathlight.app`.
- **Windows**: SmartScreen shows "unrecognized app"; More info → Run anyway.

## Which archive to take

Two per platform. `pathlight-<version>-<platform>` is the app and bundles the
command line; `pathlight-cli-<version>-<platform>` is the command alone, for a
machine that will never open a window.

- `Pathlight.app` (macOS) — the full app: live and long-term watches, history,
  menu bar survival. The command lives inside the bundle, so it survives
  dragging the app to `/Applications`:
  `/Applications/Pathlight.app/Contents/MacOS/pathlight-monitor install-cli`.
- `pathlight` / `pathlight.exe` (Windows, Linux) — a window that watches folders
  and shows what changed, with byte deltas attributed per change.
- `pathlight-monitor` — records one session to a JSONL journal from a terminal.
  `pathlight-monitor install-cli` puts it in your own home directory — no
  administrator, nothing outside your account — and prints how to add that
  directory to `PATH` if it is not there yet. `pathlight-monitor uninstall`
  lists Pathlight's own storage on this OS plus an installed command, and with
  `--yes` removes them.

`PLATFORMS.md` in each archive is the per-OS route and what each backend
guarantees — they differ, and each one says so rather than pretending otherwise.
Pathlight never modifies the folders it watches; it reads and records.
