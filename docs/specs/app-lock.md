# App lock (Phase 10, beads `kuality-hvz`)

## Goal

Put a Kuality password on the programs the user picks, like app lockers on Android. Example: Brave is in the list. When Brave starts, its windows are hidden and a prompt asks for the password. With the right password the windows come back, and Brave stays unlocked until it exits. Starting it again asks again.

This keeps out someone using the unlocked PC; it is **not** real security. Anyone using the same Windows account can end `kuality.exe` in Task Manager, copy the program to another folder, or edit `config.json`. The tab says so.

## Behavior (user-visible)

- **Settings → App lock** (new sidebar tab):
  - **Enable App lock** (off by default).
  - **Password**: with App lock on and no password yet, "Set a password" (new + confirm, at least 4 characters) and a warning that nothing is locked until one is set. With a password, "Change password" (new + confirm).
  - **Locked apps**: one row per app (icon, name, path, remove button). **Add** from a list of the apps running now (apps with a window; refresh button), or **Browse…** for an `.exe`. Kuality itself, Explorer and Windows shell processes cannot be added.
  - **Protected tab**: while App lock is on and a password is set, the tab shows only a password field until the right password is typed. It stays open for as long as the Settings window stays open.
  - Notes: what it does not protect against, apps running as administrator, and how to reset a forgotten password (quit Kuality, delete `app-lock.bin` next to `config.json`, start Kuality, set a new one).
- **Locking**: when a window of a locked app appears (the app starts, or a hidden app shows a window) and the app is locked, the window is hidden at once and the prompt opens. Hidden windows are gone from the screen, the taskbar, Alt+Tab and thumbnails. Every process of the same exe shares one state (Brave runs many `brave.exe`).
- **Prompt** (`kuality.exe --unlock --app <path>`, a GPUI popup like the clipboard one): the app's icon and name, "Enter your Kuality password to open it", a password field, **Unlock** and **Cancel**. Enter = Unlock, Esc = Cancel. Centered on the monitor of the mouse, takes the focus, stays open when it loses the focus.
  - Right password: the prompt closes, the windows come back and the app's main window comes to the front.
  - Wrong password: "Wrong password." and the field is cleared. After 5 wrong passwords, every next try is refused for 30 s, then 60 s, 120 s… (at most 15 min), counted across the prompt, the tab and the tray confirmations. The counter starts over when the right password is typed or Kuality restarts.
  - Cancel: the windows stay hidden. Starting the app again (or the app showing a window) asks again.
  - One prompt at a time. While it is open, another locked app showing a window is hidden too and the open prompt comes to the front.
- **Unlocked until it exits**: an unlocked app stays usable (new windows, new processes of the same exe, Win+L, sleep, any time in the background) until the last process Kuality saw for it exits. Apps that keep running in the background after their last window closes therefore stay unlocked until they really exit.
- **Already running**:
  - When Kuality starts (logon, after an update, after a crash), running locked apps are locked: their windows are hidden and the prompt opens.
  - When App lock is turned on, a password is set, or an app is added while it runs, the running app counts as unlocked until it exits (the user is the one setting it up).
- **Tray**: while App lock is on and a password is set, **Quit** and **Pause** ask for the password first (the same prompt: "Quit Kuality?" / "Pause Kuality?"). Resuming from pause does not. Pause now also pauses App lock: hidden windows come back, and apps started while paused stay unlocked until they exit. Closing Kuality from outside (installer, updater, `taskkill`, sign-out) never asks.
- Turning App lock off, removing an app, pausing, or quitting Kuality brings back every window it hid.

## Config

`config.json`:

```json
"app_lock": {
  "enabled": false,
  "apps": [ { "path": "C:\\Program Files\\BraveSoftware\\Brave-Browser\\Application\\brave.exe", "name": "Brave Browser" } ]
}
```

- `path`: full path of the exe, matched case-insensitively (`/` and `\` are the same). `name`: shown in the tab and the prompt (the exe's file description when added; the file name when empty).
- No password data in `config.json`.

`app-lock.bin` (data dir): a DPAPI blob (current user, fixed entropy `Kuality.AppLock.v1`) of `KALK` magic, `u32` version 1, `u32` iterations, 16-byte salt, 32-byte PBKDF2-HMAC-SHA256 hash. Written atomically by the core only. Missing = no password.

`app-lock-hidden.json` (data dir): the windows hidden right now (`pid`, process creation time, `hwnd`), rewritten when that set changes and removed when it is empty. After a crash the next core start shows them again (then locks the app again if it is still locked).

## Implementation notes

- **Core, while `app_lock.enabled`**: thread `kuality-applock` with an out-of-context `EVENT_OBJECT_SHOW` WinEvent hook (no injection, no polling). For a top-level, non-tool window of another process it reads the exe path (`QueryFullProcessImageNameW`) and asks the lock book (`core/src/app_lock/book.rs`, portable, unit-tested) what to do: hide (`ShowWindowAsync(SW_HIDE)`) and open the prompt, track the process, or nothing. Tracked processes (hidden windows' and unlocked apps') are waited on with `MsgWaitForMultipleObjectsEx`; an exit updates the book (an unlocked app with no process left is locked again).
- **Password** (`core/src/app_lock/password.rs`): PBKDF2-HMAC-SHA256 with Windows BCrypt (`BCryptDeriveKeyPBKDF2`, 600 000 iterations, salt from `BCryptGenRandom`), constant-time compare, throttle state in memory. No new dependency.
- **IPC**: the existing pipe `\\.\pipe\Kuality.Core.<pid>` now runs while the clipboard history **or** App lock is on and serves both (`protocol::ipc::Request` = clipboard or App lock request). App lock requests: `app_lock_status`, `app_lock_unlock {app, password}`, `app_lock_verify {password}` (tab), `app_lock_confirm {action, password}` (tray Quit / Pause), `app_lock_set_password {current, new}`. Passwords never appear in logs (redacted `Debug`). The prompt calls `AllowSetForegroundWindow(core)` before `unlock`, so the core can bring the app to the front.
- **Prompt process**: mutex `Local\Kuality.Unlock`, window title `Kuality unlock`; the core focuses it or starts it (1.5 s start guard).
- **Crash**: the panic hook shows the hidden windows again; `app-lock-hidden.json` covers a killed process.

## Edge cases

- **Elevated apps**: a non-elevated core cannot hide their windows (UIPI). Such an app is logged once and left alone; with "Run as administrator" (General) it works.
- Apps with only a tray icon (no window) are never asked about; their background processes keep running. Notifications from a locked app still show.
- A window may flash for a few milliseconds before it is hidden (the hook is out of context).
- Tool windows (tooltips, menus) are not hidden; dialogs owned by the app are.
- Store / UWP apps (hosted by `ApplicationFrameHost.exe`) and anything that is not an exe: out of scope for now.
- Explorer restarts and monitor changes do not matter (no taskbar windows involved).

## Out of scope

- Windows Hello, per-app passwords, relock on Win+L / sleep / timer (user decision 2026-11-15: unlocked until the app exits).
- Store apps, blocking the program file itself, protection against an administrator or someone killing Kuality.

## Test plan

- Automated: lock book (hide on show while locked, track while unlocked, relock on last exit, unlock returns hidden windows, pause / disarm restores, app added while running = unlocked, removed app restores); password record encode / decode, PBKDF2 test vectors, throttle schedule; config defaults and round trip; IPC request untagged decoding, redacted `Debug`; refused apps and path normalization.
- Manual: App lock on, set a password, add Notepad; start Notepad → hidden, prompt; wrong password ×5 → wait message; right password → Notepad back in front; second Notepad window while unlocked → not asked; close all Notepads, start again → asked; Cancel → hidden, start again → asked; tray Quit asks; turn App lock off with Notepad hidden → back; kill the core with a hidden window → next start shows it, then locks again; Settings tab gate; idle CPU / RAM of the core.
