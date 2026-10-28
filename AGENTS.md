# JumpScreen

Windows utility that adds a **"Move to this display"** task to other apps' taskbar
jump lists. Clicking it moves the app's window to the display where the
right-click happened.

## Commands

- `jumpscreen install` — inject the task into every discovered app's jump list
  (running windows with resolvable AppUserModelIDs + pinned taskbar shortcuts).
  Idempotent; re-run after launching new apps.
- `jumpscreen uninstall` — delete the task from every previously-injected app.
- `jumpscreen watch` — daemon that re-injects within ~1s of an app wiping its
  jump list. Watches `Recent\{Automatic,Custom}Destinations` via
  `ReadDirectoryChangesW`; our link's `move --aumid "<id>"` args are stored
  UTF-16 inside the `*-ms` file, so a byte-scan per registered AUMID detects
  the wipe (own commits retrigger the scan but the marker is found — no loop).
  Backstop scan every 60s; per-app re-inject throttle 5s. Logs to
  `%LOCALAPPDATA%\JumpScreen\watch.log`.
- `jumpscreen guard [--off]` — HKCU `...\Run` entry launching `watch` at logon
  (ONLOGON schtasks need admin for an all-users trigger) + a scheduled
  `install` every 10 min (picks up newly launched apps, failsafe if the
  watcher isn't running). `--off` removes both and kills a running watcher.
- `jumpscreen list` — show discovered apps and registration state.
- `jumpscreen debug` — dump visible HWNDs and resolved AUMIDs.
- `jumpscreen move --aumid X [--exe PATH] [--all]` — hidden; invoked by the
  task itself. Don't run interactively unless testing.

## Build / deploy

`cargo build --release`. Deploy `target\release\jumpscreen.exe` to a stable
location and run `install` **from that path** — task arguments embed
`current_exe()`, so installing from `target\debug` then deleting the tree breaks
the tasks.

State lives in `%LOCALAPPDATA%\JumpScreen\` (`state.json` holds registered
AUMIDs + per-app exe paths for `--exe` fallback args; also `watch.log`,
`last-error.log`).

## Architecture notes

- The taskbar context menu *is* the Jump List; we inject into other apps' lists
  via `ICustomDestinationList::SetAppID` + `AddUserTasks`. Commits land in
  `%APPDATA%\Microsoft\Windows\Recent\CustomDestinations\<hash>.customDestinations-ms`
  (verified by hex inspection: ANSI LinkInfo path + UTF-16 args). There is **no
  supported way to stop an app rewriting its own list** — `watch` re-injects
  instead of preventing. See the Obsidian note `Dev Projects/JumpScreen` for
  COM gotchas (PKEY_Title requirement, COM-owned PROPVARIANT buffers, AUMID
  discovery fallbacks).
- Release exe is `windows_subsystem = "windows"` (no console flash from the
  jump list); `AttachConsole(ATTACH_PARENT_PROCESS)` restores CLI output.
- Per-monitor DPI awareness is set at startup — all window/monitor math is
  physical pixels.
