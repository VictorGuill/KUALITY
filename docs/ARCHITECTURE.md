# Architecture

> Living document. Update it when the real code diverges from this.

## Processes

```
kuality.exe  (core, always running, few MB)
├── Keyboard hook thread   → Hyperkey + shortcut detection (must return instantly)
├── Action worker thread   → executes actions (launch, send hotkey, open URL…)
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

Current `core` modules (Phase 0):

| Module | Role |
|---|---|
| `main.rs` | Startup sequence: data dir → single instance → logging → config → tray → watcher → main loop |
| `app.rs` | Main-thread state; handles `Event::{Wake, OpenSettings, Quit}` and applies reloaded configs |
| `paths.rs` | Data dir resolution (portable marker vs `%APPDATA%\Kuality`) |
| `config_store.rs` | Load/create/atomic save of `config.json`, `Reloader` (change detection), section diff |
| `logging.rs` | `log` backend: `logs\kuality.log`, rotation, panic hook |
| `platform/mod.rs` | Traits `SingleInstance`, `DirWatcher`, `TrayIcon`, `Waker` + free functions |
| `platform/windows/` | Win32 implementations (`instance`, `tray`, `watcher`, `shell`, `folders`, `time`, `dpi`) |

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
      { "key": "1", "action": { "type": "launch", "path": "C:\\Windows\\notepad.exe", "args": [] } },
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
