# Decisions

One line per decision. Newest at the bottom of each section. Format: **Topic** — decision (short reason).
Do not contradict these without asking the user. To change one, edit the line and note the date.

## General (2026-10-31)

- **Platform** — Windows 11 only (Win10 taskbar differs; not worth the effort).
- **Language** — Rust (minimal RAM/CPU, direct Win32 access via the `windows` crate).
- **License** — MIT.
- **Project language** — code, docs, UI and commits in English; no translations for now.
- **Defaults** — every feature can be toggled and is **off by default**; a first-run wizard comes later.
- **Theme** — follow Windows light/dark mode.
- **Portability** — OS-specific code behind traits/modules so a future macOS/Linux port is possible (not planned).

## Architecture (2026-10-31)

- **Processes** — two executables: `kuality.exe` (core, always running) and `kuality-settings.exe` (only alive while the window is open).
- **Workspace** — Cargo workspace with crates `core`, `settings`, `protocol` (shared types).
- **Config** — single JSON file `config.json`, hot-reloaded by the core when it changes (written by the settings app, hand-editable).
- **Config location** — `%APPDATA%\Kuality\`; if a file named `portable` exists next to the exe, config lives next to the exe instead.
- **Core ↔ settings IPC** — config file + Windows named pipe for live changes/commands.
- **Instances** — single instance of the core.

## Taskbar widgets (2026-10-31)

- **Embedding** — widget window is a child of the taskbar (`SetParent` to `Shell_TrayWnd`), no DLL injection into Explorer.
- **Rendering** — plain Win32 + Direct2D/DirectWrite, no UI framework (keeps the core tiny).
- **Positioning** — `x`/`y` in `px` or `%`, plus `anchor` (left|center|right of the taskbar) and `origin` (which point of the widget is placed at x/y). Later: drag-to-place mode.
- **Fullscreen** — widgets hide when a fullscreen app is in the foreground.
- **Monitors** — all taskbars show the same widgets (no per-monitor layout yet).
- **Explorer restart** — re-attach on `TaskbarCreated` message.
- **Media widget** — Windows `GlobalSystemMediaTransportControlsSessionManager` API; visual inspiration: Windhawk mod "Taskbar Fluent Media Player" (check its license before reusing any code).
- **Access to settings** — tray icon menu (Settings / Pause / Quit) + right-click menu on widgets.

## Hyperkey (2026-10-31)

- **Mechanism** — low-level keyboard hook (`WH_KEYBOARD_LL`) + `SendInput`; injected events tagged via `dwExtraInfo` to avoid loops.
- **Mapping** — while CapsLock is held, Ctrl+Shift+Alt+Win are held; on release they are released. Nothing else.
- **Real CapsLock** — not available while Hyperkey is enabled (double-tap to toggle real CapsLock may come later).
- **Tap actions** — none (no single/double tap behaviors for now).
- **Start menu / menu bar side effects** — send a neutral "mask" key before releasing Win/Alt (AutoHotkey technique) so Start menu / app menu bar don't open.
- **Office key** — when Hyperkey is enabled, disable Windows' Office key shortcut via registry; restore when disabled.

## Shortcuts (2026-10-31)

- **Definition** — Hyper+key → action, stored in config. Initial actions: launch program, send hotkey, open URL.

## Settings app (2026-10-31)

- **UI framework** — GPUI (gpui-component); egui is the fallback if GPUI causes problems.
- **Layout** — centered window, sidebar tabs on the left (General, Widgets, Hyperkey, Shortcuts, Appearance, Extensions).

## Startup & distribution (2026-10-31)

- **Autostart** — normal autostart via `HKCU\...\Run`.
- **Elevated autostart** — optional toggle ("Run elevated"): creates a Task Scheduler task with highest privileges at logon (needed for the hook to work over admin windows). One UAC prompt when enabling. Also offered as an installer checkbox.
- **Distribution** — Inno Setup installer + portable zip.
- **Updates** — no auto-update for now.

## Extensions (2026-10-31, implementation in a later phase)

- **Model** — inspired by Ghostex: each extension is a folder with a JSON manifest (`kuality-extension.json`) + its executable.
- **Runtime** — extensions are separate processes in any language, talking JSON over stdin/stdout with the core.
- **Responsibility** — extensions provide data and actions; **the core does all rendering**.
- **Settings** — extensions declare a settings schema; the settings app renders the form automatically.
- **Permissions** — declared in the manifest, granted by the user.
- **Catalog** — GitHub repo with `catalog.json` + zips verified by sha256.
- **Built-in features** — media, hyperkey… use the same internal interfaces as extensions.
- **First external extension** — fan speeds (likely C# + LibreHardwareMonitor).

## Skeleton (2026-10-31, Phase 0)

- **Packages** — `kuality` (core), `kuality-settings`, `kuality-protocol`; edition 2024, current stable toolchain, no pinned MSRV.
- **Config types** — every struct `#[serde(default)]`; unknown fields kept in flattened `extra` maps; `shortcuts.bindings` / `widgets.items` stay raw JSON until Phases 2/3.
- **Config writes** — the core only writes `config.json` to create it (atomic: `.tmp` + rename); it never rewrites an existing file. Invalid file → in-memory defaults, file untouched. `version` newer than supported → warning, best-effort load.
- **Hot-reload** — `FindFirstChangeNotificationW` on the data dir (not recursive) + stop event, 250 ms debounce, skip if bytes unchanged; no `notify` crate.
- **Logging** — `log` facade + small built-in file backend (no logging framework): `logs\kuality.log`, previous run kept as `kuality.old.log`, rotated at 5 MB, local time, stderr too in debug builds, panics logged.
- **Log level** — `general.log_level` (`error|warn|info|debug|trace`, default `info`), hot-reloadable.
- **Single instance** — named mutex `Local\Kuality.Core` (per session, shared by installed and portable copies); acquired before logging so a second instance never rotates the log; second instance exits with code 0.
- **Tray** — hidden top-level window (not message-only, so it gets `TaskbarCreated`); stock `IDI_APPLICATION` icon until Phase 6; left or right click opens the menu (no double-click action); "Settings" opens `config.json` in the default editor (Notepad fallback) until Phase 5.
- **Shutdown** — tray Quit, `WM_CLOSE` (e.g. `taskkill` without `/F`), `WM_ENDSESSION` and Ctrl+C on the debug console all run the same clean shutdown.
- **Main loop** — window procedure only queues events; the loop delivers them after dispatch (no re-entrancy). Worker threads use `mpsc` + `PostMessageW` to wake it.
- **DPI manifest** — embedded with MSVC linker args (`/MANIFEST:EMBED /MANIFESTINPUT`) from `build.rs`, no resource crate; PerMonitorV2; effective awareness logged at startup.
- **Settings placeholder** — `kuality-settings` has no dependencies until Phase 5.

## Out of scope for now

- Notifications widget (needs package identity / MSIX).
- Auto-updates, translations, first-run wizard, per-monitor layouts, macOS/Linux.
