# Architecture

> Living document. Update it when the real code diverges from this.

## Processes

```
kuality.exe  (core, always running, few MB)
├── Keyboard hook thread   → Hyperkey + shortcut detection (hook must return instantly);
│                            also injects `send_hotkey` chords (it owns the Hyper modifiers)
├── Action worker thread   → runs `launch` / `open_url` (ShellExecuteExW), spawned on first use
├── Overlay thread         → widget windows attached to each taskbar, Direct2D rendering
├── Data providers         → media session, (later) extensions' data
├── Tray icon              → Settings / Pause / Quit
├── Config watcher         → hot-reloads config.json
└── IPC server             → named pipe for kuality-settings.exe

kuality-settings.exe  (GPUI, only alive while open)
└── Reads/writes config.json, talks to the core over the named pipe

extensions/*  (later, separate processes in any language)
└── JSON over stdin/stdout with the core
```

## Cargo workspace

```
crates/
├── core/       → package/binary `kuality`
├── settings/   → package/binary `kuality-settings`
└── protocol/   → package `kuality-protocol`, library: config types, IPC messages, extension protocol types (serde)
```

Inside `core`, OS-specific code lives in a `platform` module (e.g. `platform::windows::keyboard`, `platform::windows::taskbar`) behind traits, so logic stays portable.

Current `core` modules (Phases 0–3):

| Module | Role |
|---|---|
| `main.rs` | Startup sequence: data dir → single instance → logging → config → tray → watcher → main loop |
| `app.rs` | Main-thread state; handles `Event::{Wake, OpenSettings, TogglePause, Quit}`, applies reloaded configs (Hyperkey + compiled shortcuts, widgets when their section changed), reconciles the Office key |
| `hyperkey/engine.rs` | Portable Hyperkey state machine + shortcut lookup, consumed keys and the `send_hotkey` sequence (no OS types, no allocation), unit-tested |
| `hyperkey/mod.rs` | `KeyboardController` (start / apply / set_paused / shutdown) and `KeyboardCommand` |
| `shortcuts/mod.rs` | `compile(&Shortcuts) -> CompiledBindings`: validation, duplicates, warnings, generation |
| `actions/mod.rs` | `ActionWorker`: lazy `kuality-actions` thread running `launch` / `open_url` jobs in order |
| `actions/quote.rs` | Windows command-line quoting for `launch.args` lists |
| `overlay/mod.rs` | `resolve(&Widgets) -> OverlayConfig` (runtime ids, duplicates, lenient fields, warnings) and `OverlayController` (start / apply / shutdown) |
| `overlay/layout.rs` | Pure layout math: anchor, x/y, origin, width/height (`px` = DIPs, `%`), unit-tested |
| `overlay/widgets/` | `Renderer` trait, `Theme`, `Content`; `text.rs` resolves and draws the text widget |
| `paths.rs` | Data dir resolution (portable marker vs `%APPDATA%\Kuality`) |
| `config_store.rs` | Load/create/atomic save of `config.json`, `Reloader` (change detection), section diff |
| `logging.rs` | `log` backend: `logs\kuality.log`, rotation, panic hook |
| `platform/mod.rs` | Traits `SingleInstance`, `DirWatcher`, `TrayIcon`, `Waker`, `KeyboardBackend`, `OverlayBackend` + free functions |
| `platform/windows/` | Win32 implementations (`instance`, `tray`, `watcher`, `shell` (incl. `launch` / `open_url`), `folders`, `time`, `dpi`, `keyboard`, `input`, `office_key`, `crash`, `overlay/`) |
| `platform/windows/overlay/` | `thread` (host window, commands, refresh, hooks, menu), `taskbars`, `widget_window`, `render` (Direct2D/DirectWrite), `fullscreen`, `theme` |

Keyboard thread (`platform/windows/keyboard.rs`): owns a hidden top-level window (class `KualityKeyboard`) for session/power broadcasts and a `EVENT_SYSTEM_DESKTOPSWITCH` WinEvent hook. The `WH_KEYBOARD_LL` hook is installed only while Hyperkey is active; its callback runs the engine and at most one tagged `SendInput` (no allocation, locks or logging). Commands arrive through `mpsc` + `PostMessageW(WM_APP_COMMAND)`; `Apply` carries the Hyperkey config and an `Arc<CompiledBindings>` compiled on the main thread. `crash.rs` installs a panic hook and an unhandled-exception filter that release injected modifiers.

Shortcuts: when a bound key goes down while Hyper is held, the engine marks it consumed (its repeats and up are swallowed) and the callback only posts `WM_APP_ACTION(index, generation)` to its own window. The window procedure drops the message if the table generation changed, injects `send_hotkey` chords itself (releasing and re-pressing the injected Hyper modifiers in the same `SendInput`), and hands `launch` / `open_url` to the `ActionWorker` (`kuality-actions`, STA, `ShellExecuteExW`). The worker never touches keyboard state. Quit stops the keyboard thread first, then the worker (1 s timeout each).

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
  "general": { "autostart": false, "run_elevated": false },
  "hyperkey": { "enabled": false },
  "shortcuts": {
    "enabled": false,
    "bindings": [
      { "key": "1", "name": "Notepad", "enabled": true,
        "action": { "type": "launch", "path": "C:\\Windows\\notepad.exe", "args": [], "cwd": null } },
      { "key": "2", "action": { "type": "send_hotkey", "keys": "ctrl+shift+esc" } },
      { "key": "G", "action": { "type": "open_url", "url": "https://github.com" } }
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
| Theme | Registry `AppsUseLightTheme` / `SystemUsesLightTheme` + `WM_SETTINGCHANGE` |
| Elevated autostart | Task Scheduler task with "run with highest privileges" at logon |
| Single instance | Named mutex |
| IPC | Named pipe `\\.\pipe\kuality` |
