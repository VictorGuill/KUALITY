# Skeleton (Phase 0)

## Goal

A runnable `kuality.exe` that does nothing visible except a tray icon, but has the plumbing every later phase needs: workspace, config types, config load/save/hot-reload, portable mode, logging, single instance, tray menu and per-monitor DPI awareness v2.

## Behavior (user-visible)

- Starting `kuality.exe` adds a tray icon (tooltip "Kuality"). No window, no console in release builds.
- Right-click (or left-click) on the tray icon opens a menu: **Settings**, separator, **Quit**.
  - **Settings** opens `config.json` in the default editor for `.json` files (placeholder until the settings app exists in Phase 5). (No double-click action: the first click already opens the menu.)
  - **Quit** removes the icon and exits cleanly.
- On first start, `config.json` is created with defaults (everything disabled).
- Editing and saving `config.json` is picked up within ~0.5 s; the log shows "config reloaded" and which sections changed. Invalid JSON is logged and ignored (previous config stays active, file is not touched).
- Starting a second instance exits silently (logged).
- If Explorer restarts, the tray icon comes back.

## Workspace layout

```
Cargo.toml                 workspace (resolver 3, edition 2024), shared [workspace.dependencies] and lints
crates/
├── protocol/              package `kuality-protocol` (lib): config types, defaults, (de)serialization
│   └── src/{lib.rs, config.rs}
├── core/                  package `kuality`, binary `kuality`
│   ├── build.rs           embeds kuality.manifest
│   ├── kuality.manifest
│   └── src/
│       ├── main.rs        startup sequence, wires everything
│       ├── app.rs         main-thread state, handles events (config reloaded, tray commands, quit)
│       ├── paths.rs       data-dir resolution (portable vs %APPDATA%), portable logic only
│       ├── config_store.rs load / create / atomic write / reload-diff
│       ├── logging.rs     `log` backend: file + stderr in debug, rotation
│       └── platform/
│           ├── mod.rs     traits + `cfg(windows)` re-exports
│           └── windows/{mod.rs, instance.rs, tray.rs, watcher.rs, shell.rs, time.rs, folders.rs}
└── settings/              package `kuality-settings`, binary `kuality-settings`: placeholder `main` that
                           prints "not implemented yet" (no GPUI until Phase 5)
```

`platform/mod.rs` exposes small traits only where there is a real seam: `SingleInstance`, `Tray` (add/remove/re-add icon, show menu), `DirWatcher` (start/stop), plus free functions `roaming_app_data_dir()`, `local_time_now()`, `open_with_default_app(path)`. Everything else (config, paths decision, log formatting, diffing) is OS-independent and unit-tested.

## Config types (`kuality-protocol`)

All structs `#[serde(default)]`, so missing fields fall back to defaults and `{}` is a valid config. Each struct has `#[serde(flatten)] extra: serde_json::Map<String, Value>` so unknown fields survive a load → save round trip (needed once the settings app writes the file).

```rust
Config        { version: u32 = 1, general, hyperkey, shortcuts, widgets, extra }
General       { autostart: bool = false, run_elevated: bool = false, log_level: LogLevel = Info, extra }
Hyperkey      { enabled: bool = false, extra }
Shortcuts     { enabled: bool = false, bindings: Vec<serde_json::Value> = [], extra }
Widgets       { enabled: bool = false, hide_on_fullscreen: bool = true, items: Vec<serde_json::Value> = [], extra }
LogLevel      error | warn | info | debug | trace   (lowercase in JSON)
```

`bindings` and `items` stay raw JSON values in Phase 0; Phases 2 and 3 replace them with typed structs per their specs. Default file written on first start:

```json
{
  "version": 1,
  "general": { "autostart": false, "run_elevated": false, "log_level": "info" },
  "hyperkey": { "enabled": false },
  "shortcuts": { "enabled": false, "bindings": [] },
  "widgets": { "enabled": false, "hide_on_fullscreen": true, "items": [] }
}
```

`version` greater than the supported one: log a warning, load best-effort, never write the file.

## Config location + portable mode

- Data dir = exe directory if a file (or directory) named `portable` exists next to `kuality.exe`; otherwise `%APPDATA%\Kuality\` (resolved via `SHGetKnownFolderPath(FOLDERID_RoamingAppData)`), created if missing.
- `config.json` and `logs\` live in the data dir.
- Resolved once at startup and logged ("data dir: …, portable: yes/no").

## Load / save

- Startup: if `config.json` is missing → write defaults (atomic: write `config.json.tmp`, then `MoveFileExW(REPLACE_EXISTING)` via `std::fs::rename`). If present → parse. If parse fails → log the error with line/column, run with defaults **in memory**, never overwrite the user's file.
- The core never rewrites an existing config in Phase 0 (no "fill in missing fields" write-back). The save function exists (used for creation and later by the settings app via `protocol`), with tests.

## Hot-reload

- Dedicated watcher thread: `FindFirstChangeNotificationW(data_dir, FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME)` + `WaitForMultipleObjects` on the change handle and a stop event (clean shutdown, no polling, 0% CPU idle).
- Watching the directory (not the file) handles editors that save by replace/rename.
- On a change: debounce 250 ms, read `config.json`; skip if bytes are identical to the last read. Parse; on success send the new `Config` to the main thread (channel + `PostMessageW(WM_APP_CONFIG)` to the hidden window). On failure log a warning and keep the old config. File deleted → log, keep current config (do not recreate while running).
- Main thread compares old/new per section and logs which sections changed; applies `log_level` immediately. Feature sections are consumed by later phases.

## Logging

- `log` facade + a small built-in backend (no logging framework): `<data dir>\logs\kuality.log`, line format `2026-10-31 14:05:12.345 INFO  [module] message` (local time via `GetLocalTime`).
- On startup an existing `kuality.log` is renamed to `kuality.old.log` (one previous run kept). While running, if the file passes 5 MB it is rotated the same way.
- Debug builds also write to stderr (console visible); release builds use `windows_subsystem = "windows"` (no console).
- Panic hook logs the panic message and location before the default hook runs.
- Level from `general.log_level` (default `info`), changeable via hot-reload. Before config is loaded, level is `info`.

## Single instance

- Named mutex `Local\Kuality.Core` (per user session, shared by installed and portable copies).
- `ERROR_ALREADY_EXISTS` → log "another instance is running", exit code 0, no UI. Mutex held for the process lifetime.

## Tray

- Hidden **top-level** window (class `Kuality.Core`, never shown) owns the tray icon. Not a message-only window, because those do not receive the broadcast `TaskbarCreated` message used to re-add the icon after an Explorer restart.
- `Shell_NotifyIconW` with `NOTIFYICON_VERSION_4`, identified by `uID` (no GUID, so moving the exe does not break it). Icon: stock `IDI_APPLICATION` until the real app icon in Phase 6.
- Menu via `CreatePopupMenu` + `TrackPopupMenu` (with `SetForegroundWindow` before and a `WM_NULL` post after, so it closes when clicking elsewhere).
- `TaskbarCreated` → re-add icon. `WM_CLOSE`, `WM_ENDSESSION` and (debug) Ctrl+C on the console all go through the same clean shutdown.

## Clean shutdown

Quit → remove tray icon → signal watcher stop event and join the thread → log "bye" and flush → release mutex → exit 0.

## DPI manifest

- `crates/core/kuality.manifest`: `dpiAwareness` = `PerMonitorV2`, `dpiAware` = `true/pm` (fallback), Windows 10/11 `supportedOS`, Common Controls v6.
- Embedded with linker args from `build.rs` (`/MANIFEST:EMBED`, `/MANIFESTINPUT:…`) for the `kuality` binary only: no extra crate. Phase 6 (icon, version info) may switch to a resource compiler crate.
- Startup logs the effective awareness (`GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext())`) so the manifest is verifiable.

## Crates

| Crate | Where | Reason |
|---|---|---|
| `windows` 0.62 | core | Win32 APIs (mutex, tray, window, shell, known folders, change notifications, time, DPI); already the chosen binding |
| `serde` (derive) | protocol, core | Config (de)serialization |
| `serde_json` | protocol, core | Config is JSON |
| `log` | core | Logging facade, zero dependencies; later phases and crates log through it |

No other runtime or dev dependencies (tests use `std::env::temp_dir()` with unique subfolders instead of `tempfile`).

## Edge cases

- Data dir not writable / config can't be created → log (if possible) and continue with in-memory defaults; tray still works.
- `%APPDATA%` lookup fails → fall back to exe dir and log it.
- Config edited while invalid, then fixed → the fixed version is loaded.
- Many rapid saves → one reload thanks to debounce + identical-bytes check.
- Explorer restart → tray icon re-added.

## Out of scope

Pause menu item (Phase 1), real settings app (Phase 5), autostart/elevation (Phase 6), app icon (Phase 6), IPC named pipe (Phase 5), any feature behavior.

## Test plan

Automated (`cargo test`):
- `protocol`: defaults are all disabled; `{}` parses to defaults; partial config keeps given values; unknown fields at top level and in sections survive round trip; serialized defaults match the documented JSON; invalid `log_level` is an error.
- `core`: portable detection with/without `portable` file (temp dirs); load creates file when missing; invalid JSON is not overwritten and yields defaults; section diff reports the right sections; log rotation renames the old file.

Manual:
- `cargo run -p kuality`: tray icon appears; `config.json` and `logs\kuality.log` created in `%APPDATA%\Kuality`; log shows data dir and DPI awareness `PerMonitorV2`.
- Edit `config.json` (change `log_level`, `hyperkey.enabled`) → log shows reload and changed sections; break the JSON → warning, file untouched.
- Start a second instance → exits immediately.
- Create `target\debug\portable` → config/logs created next to the exe.
- Tray → Settings opens `config.json`; tray → Quit exits with code 0, icon disappears. Restart Explorer → icon returns.

## Assumptions

Decisions taken without asking, chosen as the most consistent with the docs; each will be added to `DECISIONS.md`:

1. **Tray "Settings" placeholder** opens `config.json` in the default editor (useful now, shows hot-reload; Phase 5 replaces it with launching `kuality-settings.exe`).
2. **File watching** uses `FindFirstChangeNotificationW` through the `windows` crate instead of the `notify` crate (AGENTS.md: prefer the `windows` crate, avoid casual dependencies).
3. **Logging** uses the `log` facade with a small built-in file backend instead of `tracing`/`simplelog` (lightweight first).
4. **Log level** is configurable as `general.log_level` (default `info`), hot-reloadable.
5. **Tray icon image** is the stock Windows application icon until Phase 6 adds the app icon.
6. **Manifest** is embedded via MSVC linker args, no build-dependency crate.
7. **Unknown config fields** are preserved via flattened `extra` maps from the start.
8. **Core never rewrites an existing config**; invalid config → in-memory defaults, file untouched.
9. **Second instance** exits silently with code 0 (later phases may make it open the settings app).
10. **`shortcuts.bindings` / `widgets.items`** stay untyped JSON in Phase 0; typed in Phases 2/3.
11. **`kuality-settings`** is a placeholder binary with no dependencies until Phase 5.
12. **Package names**: `kuality` (core, matches `cargo run -p kuality`), `kuality-settings`, `kuality-protocol`; edition 2024; toolchain = current stable (no pinned MSRV).
