# Architecture

> Living document. Update it when the real code diverges from this.

## Processes

```
kuality.exe  (core, always running, few MB)   — one exe, two processes (docs/specs/distribution.md)
├── Keyboard hook thread   → Hyperkey + shortcut detection (hook must return instantly);
│                            also injects `send_hotkey` chords (it owns the Hyper modifiers)
├── Action worker thread   → runs `launch` / `open_url` (ShellExecuteExW), spawned on first use
├── Overlay thread         → widget windows attached to each taskbar, Direct2D rendering
├── Data providers         → media session, (later) extensions' data
├── Update thread          → checks GitHub releases (60 s after start, every 6 h), downloads and
│                            starts the installer on request (docs/specs/updates.md)
├── Tray icon              → Settings / Pause / Check for updates or Install update / Quit,
│                            notifications (balloon / toast)
├── Config watcher         → hot-reloads config.json
└── IPC server             → (later) named pipe for the settings window

kuality.exe --settings  (GPUI via gpui-kit, only alive while open; started by tray/widget "Settings")
├── UI thread              → window, auto-save (300 ms), 1 s check for outside edits of config.json
├── Core watch thread      → waits on the core process; when it exits the window saves and quits
└── Recorder thread        → WH_KEYBOARD_LL hook, only while a Record button is active
    Started with --core-pid by the core, or on its own (then it attaches to / starts the core)
    (Phase 5: talks to the core only through config.json, which the core hot-reloads)

extensions/*  (later, separate processes in any language)
└── JSON over stdin/stdout with the core
```

## Cargo workspace

```
crates/
├── core/       → package/binary `kuality` (the only exe; `--settings` runs the settings library)
├── settings/   → package `kuality-settings`, library: the settings window
├── update/     → package `kuality-update`, library: update check (GitHub releases, SemVer), installer
│                 download + SHA-256 check + start; used by core and settings
└── protocol/   → package `kuality-protocol`, library: config types, key names, data-dir rule (`paths`),
                  atomic config write (`store`), Windows argument quoting (`quote`); later IPC and extension types
```

Inside `core`, OS-specific code lives in a `platform` module (e.g. `platform::windows::keyboard`, `platform::windows::taskbar`) behind traits, so logic stays portable.

Current `core` modules (Phases 0–3):

| Module | Role |
|---|---|
| `main.rs` | Mode by first argument (`--settings` → `kuality_settings::run`), else the core's startup sequence: data dir → single instance → logging → config → tray → watcher → main loop |
| `app.rs` | Main-thread state; handles `Event::{Wake, OpenSettings, TogglePause, Quit}`, applies reloaded configs (Hyperkey + compiled shortcuts, widgets when their section changed), reconciles the Office key |
| `hyperkey/engine.rs` | Portable Hyperkey state machine + shortcut lookup, consumed keys and the `send_hotkey` sequence (no OS types, no allocation), unit-tested |
| `hyperkey/mod.rs` | `KeyboardController` (start / apply / set_paused / shutdown) and `KeyboardCommand` |
| `shortcuts/mod.rs` | `compile(&Shortcuts) -> CompiledBindings`: validation, duplicates, warnings, generation |
| `actions/mod.rs` | `ActionWorker`: lazy `kuality-actions` thread running `launch` / `open_url` jobs in order |
| `overlay/mod.rs` | `resolve(&Widgets) -> OverlayConfig` (runtime ids, duplicates, lenient fields, warnings) and `OverlayController` (start / apply / shutdown) |
| `overlay/layout.rs` | Pure layout math: anchor, x/y, origin, width/height (`px` = DIPs, `%`), unit-tested |
| `overlay/widgets/` | `Renderer` trait, `Theme`, `Content`; `text.rs` resolves and draws the text widget |
| `updates.rs` | `Updates` (main-thread side: tray item, notifications, install / release page) and the `kuality-update` thread (`docs/specs/updates.md`) |
| `config_store.rs` | Load/create/save of `config.json` (atomic write from `protocol::store`), `Reloader` (change detection), section diff |
| `logging.rs` | `log` backend: `logs\kuality.log`, rotation, panic hook |
| `platform/mod.rs` | Traits `SingleInstance`, `DirWatcher`, `TrayIcon`, `Waker`, `KeyboardBackend`, `OverlayBackend` + free functions |
| `platform/windows/` | Win32 implementations (`instance`, `tray`, `watcher`, `shell` (incl. `launch` / `open_url`), `actions` (action thread message loop, WM_CLOSE watch), `folders`, `time`, `dpi`, `keyboard`, `input`, `hook_watchdog`, `office_key`, `crash`, `overlay/`) |
| `platform/windows/overlay/` | `thread` (host window, commands, refresh, hooks, menu), `taskbars`, `widget_window`, `render` (Direct2D/DirectWrite), `fullscreen`, `theme` |

`kuality-settings` modules (Phase 5, see `docs/specs/settings.md`):

| Module | Role |
|---|---|
| `lib.rs` | `run(args)`: single instance (or focus the open window), data dir, link to the core (`--core-pid`, running core, or start it), stderr logger in debug builds |
| `model.rs` | `Document`: load / save / poll of `config.json`, read-only on invalid file; field validation helpers (unit-tested, no UI) |
| `recorder.rs` | Portable key-recording state machine (trigger key or chord, swallowed keys) |
| `startup/` | Start at logon (`docs/specs/distribution.md`): portable plan / task XML / arguments (`mod.rs`, unit-tested) and the Win32 side (`windows.rs`: HKCU Run value, Task Scheduler COM, UAC helper). Also behind `kuality.exe --startup` / `--startup-task` |
| `platform.rs` | Windows: known folder, `Local\Kuality.Settings` mutex + focus existing window, core-running check, `CoreProcess` (open / find / start / watch the core), recorder hook thread |
| `ui/mod.rs` | `SettingsWindow` view: tabs, debounced save, disk poll, recording, banner and status bar |
| `ui/{general,widgets,hyperkey,shortcuts}.rs` | One tab each; widgets and shortcuts have list rows with an inline editor |
| `ui/controls.rs` | Layout helpers (sections, rows, fields, segmented buttons) |

Keyboard thread (`platform/windows/keyboard.rs`): owns a hidden top-level window (class `KualityKeyboard`) for session/power broadcasts and a `EVENT_SYSTEM_DESKTOPSWITCH` WinEvent hook. The `WH_KEYBOARD_LL` hook is installed only while needed and not paused (Hyperkey enabled, or a shortcut with a normal hotkey such as `ctrl+t`); its callback runs the engine and at most one tagged `SendInput` (no allocation, locks or logging). Commands arrive through `mpsc` + `PostMessageW(WM_APP_COMMAND)`; `Apply` carries the Hyperkey config and an `Arc<CompiledBindings>` compiled on the main thread. `crash.rs` installs a panic hook and an unhandled-exception filter that release injected modifiers. `hook_watchdog.rs` registers keyboard Raw Input while the hook is installed and reinstalls the hook when a physical CapsLock press arrives that the hook never saw (Windows removed it).

Shortcuts: when a bound key goes down while Hyper is held (Hyper hotkey), or with exactly the bound modifiers held (normal hotkey, modifier state tracked by the engine itself), the engine marks it consumed (or "passed" for `pass_through` bindings) (its repeats and up are swallowed) and the callback only posts `WM_APP_ACTION(index, generation)` to its own window. The window procedure drops the message if the table generation changed, injects `send_hotkey` chords itself (releasing and re-pressing the injected Hyper modifiers, or the modifiers the user still holds, in the same `SendInput`), and hands `launch` / `open_url` to the `ActionWorker` (`kuality-actions`, STA with a message loop, `ShellExecuteExW`). The worker never touches keyboard state. Because the shell may leave hidden windows in our process, the action thread hooks the threads that own them so a `WM_CLOSE` (e.g. from `taskkill`) still quits the app cleanly; Kuality's own window threads are named `kuality-*`. Quit stops the keyboard thread first, then the worker (1 s timeout each).

Overlay thread (`platform/windows/overlay/`, see `docs/specs/overlay.md`): `kuality-overlay` owns a hidden top-level window (class `KualityOverlayHost`) that receives `TaskbarCreated`, display, setting, power and session broadcasts and owns the refresh timers. For every (taskbar, item) it creates a `WS_EX_LAYERED` child window (class `KualityWidget`) of `Shell_TrayWnd` / `Shell_SecondaryTrayWnd`, rendered with one software `ID2D1DCRenderTarget` into a per-window DIB section and shown with `UpdateLayeredWindow`. A full refresh (re-enumerate taskbars, reconcile windows, lay out, render) runs on `Apply` and, debounced, on every change trigger; nothing runs while idle. WinEvent hooks limited to Explorer's pid keep widgets on top (`EVENT_OBJECT_REORDER`) and relayout on taskbar size changes; with `hide_on_fullscreen`, a foreground hook plus a location hook on the foreground process hide the widgets of a monitor covered by a fullscreen window. Because widget windows are children of Explorer's taskbar, the thread's input queue is attached to Explorer's: it never blocks (no `SendMessage` to Explorer, no waits, only the theme registry read). Widget right-click → Settings posts to the main window; Hide is session-only state of the thread.

Main loop: a hidden top-level window (class `Kuality.Core`) owns the tray icon. Its window procedure only queues events; the message loop delivers them to the app handler after `DispatchMessageW`, so the handler is never re-entered. Other threads send data through `mpsc` channels and wake the loop with `PostMessageW` (`Waker`).

## Config

- File: `config.json`.
- Location: `%APPDATA%\Kuality\config.json`, or next to the exe if a `portable` file exists there.
- Every feature section has `"enabled": false` by default.
- Missing fields fall back to defaults (forward-compatible); unknown fields are preserved where possible.

Draft shape (will be refined per phase):

```json
{
  "version": 1,
  "general": { "log_level": "info" },
  "hyperkey": { "enabled": false, "modifiers": "none" },
  "shortcuts": {
    "enabled": false,
    "bindings": [
      { "key": "hyper+1", "name": "Notepad", "enabled": true, "pass_through": false,
        "action": { "type": "launch", "path": "C:\\Windows\\notepad.exe", "args": [], "cwd": null } },
      { "key": "2", "action": { "type": "send_hotkey", "keys": "ctrl+shift+esc" } },
      { "key": "ctrl+alt+g", "action": { "type": "open_url", "url": "https://github.com" } }
    ]
  },
  "widgets": {
    "enabled": false,
    "hide_on_fullscreen": true,
    "items": [
      {
        "id": "media-1",
        "type": "media",
        "enabled": true,
        "anchor": "left",
        "x": "35%",
        "y": "50%",
        "origin": "center",
        "width": "320px"
      },
      { "id": "text-1", "type": "text", "enabled": true, "text": "Hello", "anchor": "right", "x": "-200px", "y": "50%", "origin": "center" }
    ]
  }
}
```

## Key Windows techniques

| Feature | Technique |
|---|---|
| Hyperkey | `SetWindowsHookExW(WH_KEYBOARD_LL)`, swallow CapsLock, `SendInput` modifiers, tag with `dwExtraInfo`, mask key before releasing Win/Alt |
| Office key | Registry override of `ms-officeapp` protocol handler (HKCU), restored on disable |
| Taskbar widgets | Layered child window of `Shell_TrayWnd` / `Shell_SecondaryTrayWnd`, re-attach on `TaskbarCreated` |
| Fullscreen detection | `SHQueryUserNotificationState` and/or foreground window covers monitor |
| Rendering | Direct2D + DirectWrite, per-monitor DPI aware v2 |
| Media | WinRT `GlobalSystemMediaTransportControlsSessionManager` |
| Theme | Widgets: taskbar pixels sampled from the screen (contrast rule), registry `SystemUsesLightTheme` as fallback; re-checked on `WM_SETTINGCHANGE` |
| Elevated autostart | Task Scheduler task with "run with highest privileges" at logon |
| Single instance | Named mutex |
| IPC | Named pipe `\\.\pipe\kuality` (later; Phase 5 uses the config file only) |
