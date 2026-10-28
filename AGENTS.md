# JumpScreen

Windows utility that adds a **"Move to this display"** task to other apps' taskbar
jump lists. Clicking it moves the app's window to the display where the
right-click happened.

## Commands

- `jumpscreen install` — inject the task into every discovered app's jump list
  (running windows with resolvable AppUserModelIDs + pinned taskbar shortcuts).
  Idempotent; re-run after launching new apps.
- `jumpscreen uninstall` — delete the task from every previously-injected app.
- `jumpscreen guard [--off]` — register/remove a Task Scheduler entry that
  re-runs `install` every 10 min while logged on. Keeps the task alive on apps
  that rewrite their own jump lists (VS, Chrome, WhatsApp) and picks up newly
  launched apps automatically.
- `jumpscreen list` — show discovered apps and registration state.
- `jumpscreen debug` — dump visible HWNDs and resolved AUMIDs.
- `jumpscreen move --aumid X [--exe PATH] [--all]` — hidden; invoked by the
  task itself. Don't run interactively unless testing.

## Build / deploy

`cargo build --release`. Deploy `target\release\jumpscreen.exe` to a stable
location and run `install` **from that path** — task arguments embed
`current_exe()`, so installing from `target\debug` then deleting the tree breaks
the tasks.

State lives in `%LOCALAPPDATA%\JumpScreen\` (`state.json`, `last-error.log`).

## Architecture notes

- The taskbar context menu *is* the Jump List; we inject into other apps' lists
  via `ICustomDestinationList::SetAppID` + `AddUserTasks`. See the Obsidian note
  `Dev Projects/JumpScreen` for COM gotchas (PKEY_Title requirement, COM-owned
  PROPVARIANT buffers, AUMID discovery fallbacks).
- Release exe is `windows_subsystem = "windows"` (no console flash from the
  jump list); `AttachConsole(ATTACH_PARENT_PROCESS)` restores CLI output.
- Per-monitor DPI awareness is set at startup — all window/monitor math is
  physical pixels.
