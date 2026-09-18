A pre-release. Nothing here is signed or notarized, so both desktop platforms
will warn before running it:

## Changes in v1.8.2

Replaces v1.8.1, which could stop recording for good: the macOS app took the
storage lock while creating its key file and the journal reader already held
it while asking for the key, so every row waited out a timeout and the
"could not access its activity encryption key" banner never left. The key
file is now created without the lock, a Keychain item this build cannot read
no longer holds recording off, and the Keychain is asked once per launch.

Also, on every platform: a file the watch never touched is sized from the
walk that opened the watch, so deleting it reports the bytes freed and a
first modification reports growth rather than the whole file; a Finder
duplicate is a creation, not an unsized modification; a watch asked for
through a symlink spells every change the way it was asked for; what a run
recorded live is not recovered again as a gap by the next run; a large write
that spans two batches is one file, not two; the Windows and Linux window
follows folders the terminal adds or removes while it is open; the macOS
dashboard is laid out for macOS 26 (one scrolling column, readable dialog,
no pause switch before the first folder).

## Changes in v1.8.1

Replaces the v1.8.0 pre-release, which should not be used: on Windows, Linux
and the terminal, the last-known state each watch keeps on disk listed every
file name in the clear even when the journal was encrypted, and its folder
took the process umask rather than owner-only permissions. It is now sealed
under the journal's key whenever the journal is, and owner-only like the
journal. Also: a Linux login item quotes its `Exec=` line the way the
desktop-entry spec says, a gap that arrives during a rescan gets another
rescan after it, and a replaced file after a gap is one recovered row rather
than two.

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
