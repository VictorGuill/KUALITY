# Architecture

> Living document. Update it when the real code diverges from this.

## Processes

```
kuality.exe  (core, always running, few MB)   — one exe, two processes (docs/specs/distribution.md)
├── Keyboard hook thread   → Hyperkey + shortcut detection (hook must return instantly);
│                            also injects `send_hotkey` chords (it owns the Hyper modifiers)
├── Action worker thread   → runs `launch` / `open_url` (ShellExecuteExW), spawned on first use
├── Overlay thread         → widget windows attached to each taskbar, Direct2D rendering
├── Clipboard thread       → (clipboard history on) records clipboard changes into the in-memory,
│                            encrypted history; writes entries back for paste / copy
├── IPC thread             → (clipboard history on) named pipe `\\.\pipe\Kuality.Core.<pid>`
│                            for the clipboard popup and the settings window
├── Update thread          → checks GitHub releases (60 s after start, every 6 h), downloads and
│                            starts the installer on request (docs/specs/updates.md)
├── Tray icon              → Settings / Pause / Check for updates or Install update / Quit,
│                            notifications (balloon / toast)
├── Config watcher         → hot-reloads config.json, re-reads todos.json for the to-do count
└── Log thread             → writes the log lines the other threads queue (no thread waits for
                             the disk; the keyboard thread least of all)
    (later: data providers such as the media session, and extensions' data)

kuality.exe --todo  (GPUI, the to-do popup, only alive while open; started by a click on the bar's
                     to-do module; the only writer of todos.json — docs/specs/todo.md)

kuality.exe --clipboard  (GPUI, the clipboard history popup, only alive while open; started by the
                          clipboard hotkey (`--paste`) or Settings; reads the history over the pipe,
                          never writes to disk — docs/specs/clipboard.md)

kuality.exe --settings  (GPUI via gpui-kit, only alive while open; started by tray/widget "Settings")
├── UI thread              → window, auto-save (300 ms), 1 s check for outside edits of config.json
├── Core watch thread      → waits on the core process; when it exits the window saves and quits
└── Recorder thread        → WH_KEYBOARD_LL hook, only while a Record button is active
    Started with --core-pid by the core, or on its own (then it attaches to / starts the core)
    Settings reach the core through config.json, which the core hot-reloads; the Clipboard tab's
    buttons (open, clear, remove pins) use the core's pipe

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
                  atomic config write (`store`), Windows argument quoting (`quote`), `todos.json` types
                  (`todo`), pipe framing (`ipc`), clipboard requests and pins file (`clipboard`), the
                  command line between the processes (`cli`); later extension types
```

Inside `core`, OS-specific code lives in a `platform` module (e.g. `platform::windows::keyboard`, `platform::windows::taskbar`) behind traits, so logic stays portable.

Current `core` modules:

| Module | Role |
|---|---|
| `main.rs` | Mode by first argument (`protocol::cli`: `--settings` → `kuality_settings::run`, `--todo`, `--clipboard`, `--startup`…), else the core's startup sequence: data dir → single instance → logging → config → tray → watcher → main loop |
| `app.rs` | Main-thread state; handles `Event::{Wake, OpenSettings, Todo, Clipboard, TogglePause, CheckUpdates, InstallUpdate, Quit}`, applies reloaded configs section by section (the keyboard thread only when Hyperkey, shortcuts or the clipboard hotkey changed; widgets when their section changed), starts the popups, reconciles the Office key |
| `hyperkey/engine.rs` | Portable Hyperkey state machine + shortcut lookup, consumed keys and the `send_hotkey` sequence (no OS types, no allocation), unit-tested |
| `hyperkey/mod.rs` | `KeyboardController` (start / apply / set_paused / shutdown) and `KeyboardCommand` |
| `shortcuts/mod.rs` | `compile(&Shortcuts) -> CompiledBindings`: validation, duplicates, warnings, generation |
| `actions/mod.rs` | `ActionWorker`: lazy `kuality-actions` thread running `launch` / `open_url` jobs in order |
| `overlay/mod.rs` | `resolve(&Widgets) -> OverlayConfig` (runtime ids, duplicates, lenient fields, warnings) and `OverlayController` (start / apply / shutdown) |
| `overlay/layout.rs` | Pure layout math: anchor, x/y, origin, width/height (`px` = DIPs, `%`), unit-tested |
| `overlay/widgets/` | `Renderer` trait, `Theme`, `Content`, `LiveData`; `text.rs` (text widget), `bar.rs` (bar widget: modules, spans for clicks, hover) |
| `todos.rs` | `TodoWatch`: re-reads `todos.json` on data-dir changes and reports the pending count |
| `clipboard/` | Clipboard history (`docs/specs/clipboard.md`): `ClipboardService` (start / apply / shutdown, pipe request handler, pins file), `history.rs` (ordering, duplicates, limits, pins; unit-tested), `sealed.rs` (`Sealed` / `Plain` encrypted and wiped buffers), `image.rs` (DIB / PNG / thumbnails, row by row: never a full RGBA copy) |
| `updates.rs` | `Updates` (main-thread side: tray item, notifications, install / release page) and the `kuality-update` thread (`docs/specs/updates.md`) |
| `config_store.rs` | Load/create/save of `config.json` (atomic write from `protocol::store`), `Reloader` (change detection), section diff |
| `logging.rs` | `log` backend: lines queued to the `kuality-log` thread, which writes `logs\kuality.log` (rotation); flushed at quit, on panic and on a crash |
| `platform/mod.rs` | Traits `SingleInstance`, `DirWatcher`, `TrayIcon`, `Waker`, `KeyboardBackend`, `OverlayBackend` + free functions |
| `platform/windows/` | Win32 implementations (`instance`, `tray`, `watcher`, `shell` (incl. `launch` / `open_url`), `actions` (action thread message loop, WM_CLOSE watch), `folders`, `time`, `dpi`, `keyboard`, `input`, `hook_watchdog`, `office_key`, `crash`, `overlay/`, `clipboard` (listener thread, clipboard writes, `CryptProtectMemory`, DPAPI, paste target), `ipc` (pipe server: overlapped I/O with deadlines, stop event)) |
| `platform/windows/overlay/` | `thread` (host window, commands, refresh, hooks, menu), `taskbars`, `widget_window`, `render` (Direct2D/DirectWrite), `fullscreen`, `theme` |

`kuality-settings` modules (see `docs/specs/settings.md`; everything drawn is a gpui-component element, DECISIONS "Kit first"):

| Module | Role |
|---|---|
| `lib.rs` | `run(args)`: single instance (or focus the open window), data dir, link to the core (`--core-pid`, running core, or start it), stderr logger in debug builds |
| `model.rs` | `Document`: load / save / poll of `config.json`, read-only on invalid file; field validation helpers (unit-tested, no UI) |
| `recorder.rs` | Portable key-recording state machine (trigger key or chord, swallowed keys) |
| `startup/` | Start at logon (`docs/specs/distribution.md`): portable plan / task XML / arguments (`mod.rs`, unit-tested) and the Win32 side (`windows.rs`: HKCU Run value, Task Scheduler COM, UAC helper). Also behind `kuality.exe --startup` / `--startup-task` |
| `platform.rs` | Windows: known folder, `Local\Kuality.Settings` mutex + focus existing window, core-running check, `CoreProcess` (open / find / start / watch the core), recorder hook thread |
| `ui/mod.rs` | `SettingsWindow` view: the kit `Settings` (sidebar with search, one page per tab), debounced save, disk poll, recording, banner and status bar |
| `ui/{general,widgets,hyperkey,shortcuts,todo,clipboard}.rs` | One `SettingPage` each; Widgets has cards with a collapsible form, Shortcuts a table |
| `ui/picker.rs` | The action picker of a shortcut: kit `List` of actions and installed apps, and the forms of the actions that need a value |
| `ui/window.rs` | Shared by the three GPUI processes: assets, opening the one window, popup window options, following the core process |
| `ui/style.rs` | Applies the kit theme (light / dark with Windows) and paints the native title bar to match |
| `todo.rs` | The to-do popup (`kuality.exe --todo`): placement above the clicked module, list editing, writes `todos.json` |
| `clipboard.rs` | The clipboard history popup (`kuality.exe --clipboard`): kit `List` with search and sections, preview fetched off the UI thread, pin / delete, paste / copy through the core's pipe |
| `ui/controls.rs` | Setting items bound to the window (switch, dropdown, own control, full-width block, notes) and small builders (segmented choice, form field, hotkey recorder button, keycaps) |

Keyboard thread (`platform/windows/keyboard.rs`): owns a hidden top-level window (class `KualityKeyboard`) for session/power broadcasts and a `EVENT_SYSTEM_DESKTOPSWITCH` WinEvent hook. The `WH_KEYBOARD_LL` hook is installed only while needed and not paused (Hyperkey enabled, or a shortcut with a normal hotkey such as `ctrl+t`); its callback runs the engine and at most one tagged `SendInput` (no allocation, locks or logging). Commands arrive through `mpsc` + `PostMessageW(WM_APP_COMMAND)`; `Apply` carries the Hyperkey config and an `Arc<CompiledBindings>` compiled on the main thread. `crash.rs` installs a panic hook and an unhandled-exception filter that release injected modifiers. `hook_watchdog.rs` registers keyboard Raw Input while the hook is installed and reinstalls the hook when a physical CapsLock press arrives that the hook never saw (Windows removed it).

Shortcuts: when a bound key goes down while Hyper is held (Hyper hotkey), or with exactly the bound modifiers held (normal hotkey, modifier state tracked by the engine itself), the engine marks it consumed (or "passed" for `pass_through` bindings) (its repeats and up are swallowed) and the callback only posts `WM_APP_ACTION(index, generation)` to its own window. The window procedure drops the message if the table generation changed, injects `send_hotkey` chords itself (releasing and re-pressing the injected Hyper modifiers, or the modifiers the user still holds, in the same `SendInput`), and hands `launch` / `open_url` to the `ActionWorker` (`kuality-actions`, STA with a message loop, `ShellExecuteExW`). The worker never touches keyboard state. Because the shell may leave hidden windows in our process, the action thread hooks the threads that own them so a `WM_CLOSE` (e.g. from `taskkill`) still quits the app cleanly; Kuality's own window threads are named `kuality-*`. Quit stops the keyboard thread first, then the worker (1 s timeout each).

Overlay thread (`platform/windows/overlay/`, see `docs/specs/overlay.md`): `kuality-overlay` owns a hidden top-level window (class `KualityOverlayHost`) that receives `TaskbarCreated`, display, setting, power and session broadcasts and owns the refresh timers. For every (taskbar, item) it creates a `WS_EX_LAYERED` child window (class `KualityWidget`) of `Shell_TrayWnd` / `Shell_SecondaryTrayWnd`, rendered with one software `ID2D1DCRenderTarget` into a per-window DIB section and shown with `UpdateLayeredWindow`. A full refresh (re-enumerate taskbars, reconcile windows, lay out, render) runs on `Apply` and, debounced, on every change trigger; nothing runs while idle. WinEvent hooks limited to Explorer's pid keep widgets on top (`EVENT_OBJECT_REORDER`) and relayout on taskbar size changes; with `hide_on_fullscreen`, a foreground hook plus a location hook on the foreground process hide the widgets of a monitor covered by a fullscreen window. Because widget windows are children of Explorer's taskbar, the thread's input queue is attached to Explorer's: it never blocks (no `SendMessage` to Explorer, no waits, only the theme registry read). Widget right-click → Settings posts to the main window; Hide is session-only state of the thread. Left clicks and mouse moves are posted to the host too: bar modules are hit-tested with the spans of the last render, hover re-renders with a highlight, and a click on (or the mouse resting on) the to-do module hands a `TodoRequest` (module rectangle in screen px, trigger) to the main window (`WM_APP_TODO`, `Event::Todo`). `OverlayCommand::Data(LiveData)` (e.g. the pending to-do count) re-renders without re-enumerating taskbars.

Main loop: a hidden top-level window (class `Kuality.Core`) owns the tray icon. Its window procedure only queues events; the message loop delivers them to the app handler after `DispatchMessageW`, so the handler is never re-entered. Other threads send data through `mpsc` channels and wake the loop with `PostMessageW` (`Waker`).

## Config

- File: `config.json`.
- Location: `%APPDATA%\Kuality\config.json`, or next to the exe if a `portable` file exists there.
- Every feature section has `"enabled": false` by default.
- To-do items are data, not settings: `todos.json` next to `config.json` (`protocol::todo`), written only by the to-do popup.
- Pinned clipboard entries: `clipboard-pins.bin` next to `config.json`, DPAPI-encrypted, written only by the core (`protocol::clipboard` has the decrypted layout). The rest of the clipboard history is never written.
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
  "todo": { "enabled": false, "new_items": "top" },
  "clipboard": { "enabled": false, "hotkey": "hyper+v", "max_entries": 100, "images": true },
  "widgets": {
    "enabled": false,
    "hide_on_fullscreen": true,
    "items": [
      { "id": "bar-1", "type": "bar", "enabled": true, "anchor": "left", "x": "12px",
        "modules": [{ "type": "todo", "label": "TO DO", "show_count": true }] },
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
| Fullscreen detection | Foreground window covers its monitor (`EVENT_SYSTEM_FOREGROUND` + location changes of that process, out-of-context WinEvent hooks) |
| Rendering | Direct2D + DirectWrite, per-monitor DPI aware v2 |
| Media (Phase 4, not built yet) | WinRT `GlobalSystemMediaTransportControlsSessionManager` |
| Theme | Widgets: taskbar pixels sampled from the screen (contrast rule), registry `SystemUsesLightTheme` as fallback; re-checked on `WM_SETTINGCHANGE` |
| Elevated autostart | Task Scheduler task with "run with highest privileges" at logon |
| Single instance | Named mutex |
| Clipboard history | `AddClipboardFormatListener` on a message-only window; `CryptProtectMemory` in memory, DPAPI (`CryptProtectData`) for pins |
| IPC | Named pipe `\\.\pipe\Kuality.Core.<pid>` (clipboard history; settings changes still use the config file); overlapped I/O, every read and write with a deadline |
