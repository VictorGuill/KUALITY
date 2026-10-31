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
- **Core ↔ settings IPC** — config file + Windows named pipe for live changes/commands. *(2026-10-31: Phase 5 ships with the file only; the pipe is deferred, see "Settings app implementation".)*
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
- **Layout** — centered window, sidebar tabs on the left (General, Widgets, Hyperkey, Shortcuts, Appearance, Extensions). *(2026-10-31: Appearance and Extensions are hidden until they have content.)*

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
- **Tray** — hidden top-level window (not message-only, so it gets `TaskbarCreated`); stock `IDI_APPLICATION` icon until Phase 6; right click opens the menu (no double-click action) *(2026-10-31: left click and Enter/Space on the focused icon open Settings directly, same as the menu item; repeats within 500 ms are ignored; until then left click also opened the menu)*; "Settings" opens `config.json` in the default editor (Notepad fallback) until Phase 5. *(2026-10-31: since Phase 5 it starts `kuality-settings.exe`; opening the file is the fallback.)*
- **Shutdown** — tray Quit, `WM_CLOSE` (e.g. `taskkill` without `/F`), `WM_ENDSESSION` and Ctrl+C on the debug console all run the same clean shutdown.
- **Main loop** — window procedure only queues events; the loop delivers them after dispatch (no re-entrancy). Worker threads use `mpsc` + `PostMessageW` to wake it.
- **DPI manifest** — embedded with MSVC linker args (`/MANIFEST:EMBED /MANIFESTINPUT`) from `build.rs`, no resource crate; PerMonitorV2; effective awareness logged at startup.
- **Settings placeholder** — `kuality-settings` has no dependencies until Phase 5.

## Hyperkey implementation (2026-10-31, Phase 1)

- **Key codes** — `protocol::Key(u16)` whose values equal Windows virtual-key codes; a future port maps native codes to them.
- **Keyboard thread** — `kuality-keyboard` lives for the whole process (hidden top-level window for broadcasts); the `WH_KEYBOARD_LL` hook is installed only while Hyperkey is active and reinstalled on every config apply, unlock and resume.
- **Pause** — tray item `Pause` (checkable, after Settings) uninstalls the hook; runtime only, keyboard features only (not widgets, not the Office key); tooltip `Kuality (paused)`.
- **Office key backup** — besides `PrevExisted`/`PrevCommand`, the backup records the outermost key Kuality created (`CreatedKey`) and restore deletes only that subtree; `HKCU\Software\Kuality` is removed if left empty. A previous command of type `REG_EXPAND_SZ` keeps its type; any other type is left alone (apply fails with a warning).
- **Leftover cleanup** — releases sent after unlock/resume are preceded by the mask key (a lone Win up would open Start); if a forced release failed (secure desktop), the next desktop switch also clears leftover modifiers.
- **Window close** — `taskkill` without `/F` may target the keyboard window: its `WM_CLOSE` is forwarded to the main window (same clean shutdown). Commands fall back to `PostThreadMessageW` if the window is gone.
- **Hard kill** — `taskkill /F` cannot release injected modifiers or restore the Office key; the next start restores the Office key if Hyperkey is disabled (documented limitation).
- **keyviewer** — dev-only example `cargo run -p kuality --example keyviewer` for manual tests.

## Shortcuts implementation (2026-10-31, Phase 2)

- **Requires Hyperkey** — shortcuts only fire while Hyperkey is active; tray Pause stops both. A warning is logged when `shortcuts.enabled` is on but `hyperkey.enabled` is off.
- **Bindings** — typed `BindingEntry` (valid binding, or raw JSON kept verbatim); compiled on the main thread on every load into an `Arc<CompiledBindings>` with a generation; actions triggered against an older generation are dropped. Linear lookup in the hook (no allocation).
- **Log identity** — compiled entries keep their index in `shortcuts.bindings`; logs say `shortcut #<index>` plus the action type or skip reason, never key names (key/chord parse errors never echo the input).
- **Consumed keys vs. hook reinstall** — every `Apply` still reinstalls the hook with a forced release (Phase 1), but the consumed-key set is restored afterwards, so the up of a bound key held across a config reload is still swallowed. `reset`, Pause/disable and lock/suspend clear it.
- **Threads** — `send_hotkey` runs on the keyboard thread (it owns the injected Hyper modifiers); `launch` / `open_url` run in order on `kuality-actions`, spawned on the first action, joined (1 s) after the keyboard thread at quit. The action thread is a COM STA with a message loop; jobs travel through a channel plus a `WM_APP` thread message.
- **Quit with shell windows in the process** — `ShellExecuteExW` can leave hidden top-level windows (e.g. a `WorkerW` on a shell helper thread that lives ~20–40 s) and `taskkill` without `/F` may deliver its `WM_CLOSE` there. The action thread watches window creation in our process (`EVENT_OBJECT_CREATE`, out of context) and hooks threads that are not Kuality's with thread-local `WH_CALLWNDPROC` + `WH_GETMESSAGE` (same process, no injection); a `WM_CLOSE` to those windows or to the action thread's own windows requests a clean app quit. Kuality threads that own windows must be named `kuality-*` (or be the main thread) so they are never hooked.
- **Shell call** — `ShellExecuteExW` with verb `open`, `SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC`, `%VAR%` expanded at execution time, `cwd` defaults to the parent of an absolute path, else `%USERPROFILE%`; `AllowSetForegroundWindow(ASFW_ANY)` before each call.
- **Foreground for launched apps** — before each `ShellExecuteExW` the worker sends one empty, tagged mouse `SendInput` so Windows counts Kuality as the last input source; only then does `AllowSetForegroundWindow(ASFW_ANY)` take effect (the hook swallows the shortcut key, so otherwise the foreground lock keeps launched apps and `open_url` targets behind). Same technique as PowerToys' `BringToForeground`.
- **Argument quoting** — `launch.args` lists use `CommandLineToArgvW` rules; besides space, tab and `"`, newline and vertical tab also trigger quoting (as in Microsoft's reference implementation).

## Taskbar overlay implementation (2026-10-31, Phase 3)

- **Feasibility** — verified on Windows 11 build 26300: a `WS_EX_LAYERED` child of `Shell_TrayWnd` placed at `HWND_TOP` draws above the XAML taskbar content (`DesktopWindowContentBridge`); no fallback needed. Layered child windows need the exe manifest's Windows 10/11 `supportedOS` (creation fails without it).
- **Item types** — `WidgetItem` is an internally tagged enum (`type`) whose variants embed a flattened `CommonItem` plus their own `extra` map (flattening a tagged enum next to `extra` would copy its fields into `extra`).
- **Invalid values** — `anchor`, `x`, `y`, `origin`, `width`, `height`, `font_size`, `font_weight`, `color` and `align` are `Lenient<T>`: an invalid value is kept verbatim on save and replaced by the default (+ `warn`) at runtime. A wrong JSON type for `type`, `id`, `enabled` or `text` makes the whole item raw JSON (kept, ignored with a `warn`).
- **Duplicate ids** — checked across all known items, enabled or not; the first one wins.
- **Resolution** — the `widgets` section is resolved on the main thread (`overlay::resolve`, problems logged once per load) and sent as `Arc<OverlayConfig>`; `Apply` is only sent when the section changed, so a session "Hide" survives unrelated reloads. The overlay thread logs rare events only.
- **Refresh** — `Apply` refreshes immediately; all other triggers use the 300 ms debounce. `TaskbarCreated` destroys our windows explicitly (harmless if already dead), so a repeated or synthetic message never leaves duplicates.
- **Z-order** — on `EVENT_OBJECT_REORDER` our windows are re-raised only when they are not already the topmost children in item order (no feedback loop with our own `SetWindowPos`).
- **Text drawing** — `ID2D1RenderTarget::DrawText` into the box rect with `D2D1_DRAW_TEXT_OPTIONS_CLIP` (avoids a direct `windows-numerics` dependency that `DrawTextLayout` would need); measured with `IDWriteTextLayout`. Font fallback cached per family, logged once.
- **Hit testing** — every render clears to transparent and fills the box with black at alpha 1/255 through Direct2D, then draws the content.
- **Fullscreen hooks** — installed only while widgets are enabled and `hide_on_fullscreen` is true; the foreground hook skips our own process (our menu taking focus keeps the previous state).
- **Create retry** — a failed widget window creation (seen: `CreateWindowExW` "Access is denied" for minutes after a Hide, cleared by opening Start) is retried by a one-shot timer with backoff 1, 2, 4, 8, 16, then 30 s, at most 12 times; while one is pending, taskbar reorder/size and foreground changes also refresh. Logged once per failure streak (+ one line on recovery). Debug builds: `KUALITY_TEST_FAIL_CREATES=<n>` fails the first n creations to test this path.
- **Widget menu position** — bottom-aligned at the taskbar's top edge with the taskbar rect excluded (`TPM_VERTICAL` + `TPMPARAMS`); at the cursor it could open partly behind the taskbar band.
- **Default text color** — chosen from what each taskbar actually shows, not from the registry: on every refresh the overlay reads two pixel rows of the taskbar from the screen (12 % and 88 % of its height, every ≥ 4 px, our widgets excluded), takes the median WCAG relative luminance and uses `#1B1B1B` or `#FFFFFF`, whichever has the higher contrast (light text below luminance ≈ 0.2). `SystemUsesLightTheme` is only the fallback when no sample was ever possible (taskbar auto-hidden off its monitor, covered by a fullscreen window, capture failed); a failed sample keeps the previous one. Reason: the registry value can disagree with the real taskbar (seen: value 1 with a dark taskbar); this also covers accent-colored and restyled taskbars. A color-mode change refreshes at +0.3, +2 and +5 s because Explorer repaints asynchronously.
- **Widget menu → Settings** — the overlay thread posts `WM_APP_OPEN_SETTINGS` to the `Kuality.Core` window, which queues `Event::OpenSettings` (no channel: there is no data).
- **Debug console close** — `taskkill` without `/F` also closes the debug console (it reports our process as owner): `CTRL_CLOSE_EVENT`, logoff and shutdown now request the normal quit and block until the process exits.

## Settings app implementation (2026-10-31, Phase 5)

- **Order and scope** — Phase 5 is done before Phase 4 and only covers settings that exist (Phases 0–3); named pipe, drag-to-place, Appearance/Extensions tabs and autostart/run-elevated UI are deferred (ROADMAP "Later").
- **Live apply** — the settings app only writes `config.json`; the core's existing hot-reload applies it. No IPC yet.
- **Framework** — `gpui-kit =0.8.1` (gpui-component's umbrella crate; pins `gpui-pre 0.3.10`), exact pin because GPUI APIs move. Settings crate only; about 100 MB RAM while the window is open, nothing when closed. `futures-channel` (already in GPUI's tree) for the recorder's oneshot.
- **Shared code** — `DataDir`, the atomic write (`store::write_atomic`) and `quote::join_args` moved from the core to `kuality-protocol`; `valid_url` and `Key::is_trigger` live there too. `test_util::TempDir` is behind the protocol feature `test-util` (dev-dependency only).
- **Action extras** — `Action` variants are newtype variants over `LaunchAction` / `SendHotkeyAction` / `OpenUrlAction`, each with its own `extra` map, so unknown fields inside `action` survive a save.
- **Saving** — edits change the parsed `Config` in place; valid changes are written 300 ms after the last edit (atomic, skipped when bytes are unchanged) and flushed on close. Opening the window never writes. Output is normalized pretty JSON (missing known fields written with defaults, unknown keys sorted).
- **Invalid values** — an invalid field shows a message and is not written (the file keeps the last valid value); `Lenient::Invalid` values from the file are kept verbatim until changed. Number fields reject non-numeric typing themselves.
- **Outside changes** — a 1 s timer (and window activation) checks size + modified time, then bytes. A valid change reloads the form and drops a pending save (the outside edit wins); an invalid file closes the editors and makes the window read-only until fixed.
- **Drafts** — "Add shortcut" opens one draft at a time; it is written once its key and required action field are valid, and dropped by Discard or on close. A new widget is written at once (text "Hello", id `text-<n>`).
- **Launch arguments** — shown as one command line (`join_args` for lists); an edited value is saved as a raw string (empty → `[]`), untouched lists stay lists.
- **Key recording** — a `WH_KEYBOARD_LL` hook on thread `kuality-settings-recorder`, alive only while recording (plus up to 1 s for pending key-ups); it ignores injected events, swallows keys only while the settings window is in front, never records CapsLock, and Esc alone cancels. Recording also ends on a click, focus loss, tab change or after 10 s. Pure state machine in `recorder.rs` (unit-tested).
- **Opening** — tray / widget "Settings" spawn `kuality-settings.exe --core-pid <pid>` from the core's folder (`AllowSetForegroundWindow` first); if it is missing or fails, the old behaviour (open `config.json`) is the fallback.
- **Single instance** — named mutex `Local\Kuality.Settings`; a second instance finds the window titled `Kuality Settings` owned by `kuality-settings.exe`, restores and focuses it, and exits 0.
- **Core detection** — the status bar says "Kuality is not running" when `Local\Kuality.Core` cannot be opened (checked every second).
- **One app** *(2026-10-31, follow-up)* — the settings window never outlives the core: it attaches to the core given by `--core-pid`, else to the running one (found by its `Kuality.Core` window), else starts `kuality.exe` from its own folder; a thread waits on the core process handle and, when the core exits for any reason (Quit, `taskkill`, `/F`, crash), the window flushes a pending save and quits. If the `--core-pid` process is already gone, the settings app exits without a window. Closing the window never stops the core.
- **No core available** *(2026-10-31)* — when `kuality.exe` is missing or fails to start, the window opens anyway with a "Kuality is not running" banner giving the reason; no second core is started when the core mutex exists but its window is not found.
- **Logging** — no log file; debug builds log to stderr (shared with a debug core's console), release builds are a GUI-subsystem exe with no console.
- **Accessibility** — icon-only buttons, switches and checkboxes carry accessibility labels (they double as stable names for UI automation).

## Out of scope for now

- Notifications widget (needs package identity / MSIX).
- Auto-updates, translations, first-run wizard, per-monitor layouts, macOS/Linux.
