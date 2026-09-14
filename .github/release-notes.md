A pre-release. Nothing here is signed or notarized, so both desktop platforms
will warn before running it:

## Changes in v1.5.0-rc.6

- Remembers each folder's confirmed minimum change and reporting interval.
- Applies the minimum byte-delta threshold consistently to known deletions,
  moves, and aggregated changes; `0 KB` records every measurable change.
- Uses the same supported reporting range (250 ms to 5 minutes) in every host.
- Reports macOS watcher startup failures without leaving a finished live
  monitor active or hiding the failure after a successful journal write.

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
