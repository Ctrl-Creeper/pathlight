A pre-release. Nothing here is signed or notarized, so both desktop platforms
will warn before running it:

- **macOS**: right-click `Pathlight.app` and choose Open, or
  `xattr -dr com.apple.quarantine Pathlight.app`.
- **Windows**: SmartScreen shows "unrecognized app"; More info → Run anyway.

## What is in each archive

- `Pathlight.app` (macOS) — the full app: live and long-term watches, history,
  menu bar survival.
- `pathlight` / `pathlight.exe` (Windows, Linux) — a window that watches folders
  and shows what changed, with byte deltas attributed per change.
- `pathlight-monitor` — records one session to a JSONL journal from a terminal.
  `pathlight-monitor uninstall` lists Pathlight's own storage on this OS and,
  with `--yes`, removes it.

`PLATFORMS.md` in each archive is the per-OS route and what each backend
guarantees — they differ, and each one says so rather than pretending otherwise.
Pathlight never modifies the folders it watches; it reads and records.
