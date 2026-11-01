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

- **Processes** — two processes of **one** executable *(2026-11-01, user decision; was two executables)*: `kuality.exe` is the core (always running) and `kuality.exe --settings` the settings window (only alive while it is open). The core never loads GPUI, so its footprint is unchanged and the window's memory is freed when it closes. Crate `kuality-settings` is a library the `kuality` binary calls.
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
- **Mapping** — while CapsLock is held, Ctrl+Shift+Alt+Win are held; on release they are released. Nothing else. *(2026-11-01: Win is optional, see "Include Windows key".)*
- **Real CapsLock** — not available while Hyperkey is enabled (double-tap to toggle real CapsLock may come later).
- **Tap actions** — none (no single/double tap behaviors for now).
- **Start menu / menu bar side effects** — send a neutral "mask" key before releasing Win/Alt (AutoHotkey technique) so Start menu / app menu bar don't open.
- **Office key** — when Hyperkey is enabled, disable Windows' Office key shortcut via registry; restore when disabled. *(2026-11-01: only while Hyper includes Win.)*

## Shortcuts (2026-10-31)

- **Definition** — Hyper+key → action, stored in config. Initial actions: launch program, send hotkey, open URL. *(2026-11-01: any hotkey, see "Any hotkey".)*

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
- **Main loop** — window procedure only queues events; the loop delivers them after dispatch (no re-entrancy). Worker threads use `mpsc` + `PostMessageW` to wake it. *(2026-10-31: an event queued while handling a *sent* message, e.g. the tray callback, is followed by a posted `WM_NULL` so the loop delivers it right away.)*
- **DPI manifest** — embedded with MSVC linker args (`/MANIFEST:EMBED /MANIFESTINPUT`) from `build.rs`, no resource crate; PerMonitorV2; effective awareness logged at startup. *(2026-11-01, single exe: GPUI's build script always embeds its own manifest as `RT_MANIFEST 1` (default feature `windows-manifest`, enabled by gpui-kit) and an exe holds only one, so ours is gone; GPUI's has the same `supportedOS`, PerMonitorV2 and common controls v6, plus `asInvoker` and the segment heap.)*
- **Settings placeholder** — `kuality-settings` has no dependencies until Phase 5.

## Hyperkey implementation (2026-10-31, Phase 1)

- **Key codes** — `protocol::Key(u16)` whose values equal Windows virtual-key codes; a future port maps native codes to them.
- **Keyboard thread** — `kuality-keyboard` lives for the whole process (hidden top-level window for broadcasts); the `WH_KEYBOARD_LL` hook is installed only while Hyperkey is active and reinstalled on every config apply, unlock and resume.
- **Pause** — tray item `Pause` (checkable, after Settings) uninstalls the hook; runtime only, keyboard features only (not widgets, not the Office key); tooltip `Kuality (paused)`.
- **Office key backup** — besides `PrevExisted`/`PrevCommand`, the backup records the outermost key Kuality created (`CreatedKey`) and restore deletes only that subtree; `HKCU\Software\Kuality` is removed if left empty. A previous command of type `REG_EXPAND_SZ` keeps its type; any other type is left alone (apply fails with a warning).
- **Office letters** *(2026-11-01)* — while Hyper is held, the hook drops W/X/P/O/T/D/N/L/Y unless bound to a shortcut (no setting). Explorer registers them as hotkeys and opens `https://go.microsoft.com/fwlink/…` web pages (Teams, Word, LinkedIn…) directly, which the `ms-officeapp` override cannot stop; apps never received these combos anyway. A physical Ctrl+Shift+Alt+Win+letter (without CapsLock) still reaches Explorer (limitation).
- **Leftover cleanup** — releases sent after unlock/resume are preceded by the mask key (a lone Win up would open Start); if a forced release failed (secure desktop), the next desktop switch also clears leftover modifiers.
- **Window close** — `taskkill` without `/F` may target the keyboard window: its `WM_CLOSE` is forwarded to the main window (same clean shutdown). Commands fall back to `PostThreadMessageW` if the window is gone.
- **Hard kill** — `taskkill /F` cannot release injected modifiers or restore the Office key; the next start restores the Office key if Hyperkey is disabled (documented limitation).
- **keyviewer** — dev-only example `cargo run -p kuality --example keyviewer` for manual tests.

## Shortcuts implementation (2026-10-31, Phase 2)

- **Requires Hyperkey** — shortcuts only fire while Hyperkey is active; tray Pause stops both. A warning is logged when `shortcuts.enabled` is on but `hyperkey.enabled` is off. *(2026-11-01, user request: applies to Hyper hotkeys only; normal hotkeys only need `shortcuts.enabled`. The warning is logged when an active binding is a Hyper hotkey and Hyperkey is off.)*
- **Bindings** — typed `BindingEntry` (valid binding, or raw JSON kept verbatim); compiled on the main thread on every load into an `Arc<CompiledBindings>` with a generation; actions triggered against an older generation are dropped. Linear lookup in the hook (no allocation).
- **Log identity** — compiled entries keep their index in `shortcuts.bindings`; logs say `shortcut #<index>` plus the action type or skip reason, never key names (key/chord parse errors never echo the input).
- **Consumed keys vs. hook reinstall** — every `Apply` still reinstalls the hook with a forced release (Phase 1), but the consumed-key set is restored afterwards, so the up of a bound key held across a config reload is still swallowed. `reset`, Pause/disable and lock/suspend clear it.
- **Threads** — `send_hotkey` runs on the keyboard thread (it owns the injected Hyper modifiers); `launch` / `open_url` run in order on `kuality-actions`, spawned on the first action, joined (1 s) after the keyboard thread at quit. The action thread is a COM STA with a message loop; jobs travel through a channel plus a `WM_APP` thread message.
- **Quit with shell windows in the process** — `ShellExecuteExW` can leave hidden top-level windows (e.g. a `WorkerW` on a shell helper thread that lives ~20–40 s) and `taskkill` without `/F` may deliver its `WM_CLOSE` there. The action thread watches window creation in our process (`EVENT_OBJECT_CREATE`, out of context) and hooks threads that are not Kuality's with thread-local `WH_CALLWNDPROC` + `WH_GETMESSAGE` (same process, no injection); a `WM_CLOSE` to those windows or to the action thread's own windows requests a clean app quit. Kuality threads that own windows must be named `kuality-*` (or be the main thread) so they are never hooked.
- **Shell call** — `ShellExecuteExW` with verb `open`, `SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS` (the handle only gives the PID for the focus watch, then is closed), `%VAR%` expanded at execution time, `cwd` defaults to the parent of an absolute path, else `%USERPROFILE%`; `AllowSetForegroundWindow(ASFW_ANY)` before each call.
- **Foreground for launched apps** — before each `ShellExecuteExW` the worker sends one empty, tagged mouse `SendInput` so Windows counts Kuality as the last input source; only then does `AllowSetForegroundWindow(ASFW_ANY)` take effect (the hook swallows the shortcut key, so otherwise the foreground lock keeps launched apps and `open_url` targets behind). Same technique as PowerToys' `BringToForeground`.
- **Foreground after the Hyper release** *(2026-11-01)* — the grant given before `ShellExecuteExW` ends at the next input, and releasing Hyper is input (the user's CapsLock up and our injected modifier ups), so an app whose first window appears after the release (Character Map: ~250–400 ms) opens behind; renewing the grant after the release did not help (tested). Instead the action thread watches for up to 3 s (50 ms thread timer, jobs never wait) for the first visible, unowned, non-tool top-level window of the started process (PID from `SEE_MASK_NOCLOSEPROCESS`), or of any process if it was not there before the launch (no process handle for packaged apps, `shell:AppsFolder`, URLs, launcher stubs), and brings it forward (empty tagged mouse input + `SetForegroundWindow`) only while the foreground is still the window that was in front when the shortcut fired. If the foreground changed, the app came forward itself or the user moved on, and nothing is done.
- **Argument quoting** — `launch.args` lists use `CommandLineToArgvW` rules; besides space, tab and `"`, newline and vertical tab also trigger quoting (as in Microsoft's reference implementation).

- **Any hotkey** *(2026-11-01, user request)* — a shortcut is Hyper+key or a normal combination (`ctrl+t`, `win+e`, `ctrl+alt+k`). `bindings[].key` is a chord of the existing key names plus a `hyper` modifier; a bare key (`"1"`) keeps meaning Hyper+1 so old files work. The settings app writes `hyper+<key>` for Hyper hotkeys. Modifier sides are ignored (`rctrl` = `ctrl`); matching needs exactly the bound set. `hyper` cannot be combined with other modifiers. A normal hotkey needs at least one of Ctrl/Shift/Alt/Win (a lone key would be swallowed everywhere, and a bare name already means Hyper).
- **Override by default** *(2026-11-01, user request)* — a bound hotkey is swallowed (down, repeats, up): the app in front never gets it, Win+E no longer opens Explorer. `bindings[].pass_through: true` ("Also send to the app", default `false`) runs the action and lets the key through.
- **Normal hotkeys: modifier state** — tracked by the engine from the hook's own events; on a key down with modifiers believed down, bits the system (`GetAsyncKeyState`) reports up are cleared (an up missed while the hook was off or an elevated window was in front). The system state only clears, never sets; on hook install it is copied once. A swallowed normal hotkey is followed by the mask key, so releasing Win/Alt/Ctrl+Shift opens no Start menu, menu bar or input-language switch.
- **`send_hotkey` from a normal hotkey** — the modifiers the user still holds are released (mask first) before the chord and pressed again after it, in the same `SendInput`, followed by the mask key when Win or Alt is among them.
- **Reserved hotkeys** — Ctrl+Alt+Del (any extra modifiers) and Win+L cannot be bound: the parser rejects them (binding skipped with a warning) and the recorder refuses them with a message. Elevated windows in front hide keys from the hook (documented limitation; "Run elevated" later).
- **Duplicates** — compared as parsed triggers (`"1"` = `"hyper+1"`, `alt+ctrl+k` = `ctrl+alt+k`): first enabled one wins, later ones logged and flagged in the UI.
- **Include Windows key** *(2026-11-01, user request)* — `hyperkey.include_win`, default **true** (Ctrl+Shift+Alt+Win as before); off → Hyper is Ctrl+Shift+Alt. The Office-key registry override and the Office-letter dropping exist only because of Ctrl+Shift+Alt+Win: both apply only while `include_win` is on (Explorer never sees the Office combination otherwise). The settings app warns that on some layouts (e.g. Spanish) Ctrl+Alt acts as AltGr.
- **Hook without Hyperkey** — the keyboard hook is installed when Hyperkey is enabled or any active binding is a normal hotkey (and not paused); with Hyperkey off, CapsLock and modifiers pass untouched.

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
- **Drafts** — "Add shortcut" opens one draft at a time; it is written once its key and required action field are valid, and dropped by Discard or on close. A new widget is written at once (text "Hello", id `text-<n>`). *(2026-11-01: the shortcut draft is a table row whose Hotkey button starts recording at once; the picker opens once a key is recorded; its delete button discards it.)*
- **Launch arguments** — shown as one command line (`join_args` for lists); an edited value is saved as a raw string (empty → `[]`), untouched lists stay lists.
- **Key recording** — a `WH_KEYBOARD_LL` hook on thread `kuality-settings-recorder`, alive only while recording (plus up to 1 s for pending key-ups); it ignores injected events, swallows keys only while the settings window is in front, never records CapsLock, and Esc alone cancels. Recording also ends on a click, focus loss, tab change or after 10 s. Pure state machine in `recorder.rs` (unit-tested).
- **Opening** — tray / widget "Settings" spawn `kuality-settings.exe --core-pid <pid>` from the core's folder (`AllowSetForegroundWindow` first); if it is missing or fails, the old behaviour (open `config.json`) is the fallback. *(2026-11-01: `kuality.exe --settings --core-pid <pid>`, the core's own exe.)*
- **Single instance** — named mutex `Local\Kuality.Settings`; a second instance finds the window titled `Kuality Settings` owned by `kuality-settings.exe`, restores and focuses it, and exits 0. *(2026-11-01: owned by a process of our own exe name.)*
- **Core detection** — the status bar says "Kuality is not running" when `Local\Kuality.Core` cannot be opened (checked every second).
- **One app** *(2026-10-31, follow-up)* — the settings window never outlives the core: it attaches to the core given by `--core-pid`, else to the running one (found by its `Kuality.Core` window), else starts the core (its own exe without arguments); a thread waits on the core process handle and, when the core exits for any reason (Quit, `taskkill`, `/F`, crash), the window flushes a pending save and quits. If the `--core-pid` process is already gone, the settings app exits without a window. Closing the window never stops the core.
- **No core available** *(2026-10-31)* — when `kuality.exe` is missing or fails to start, the window opens anyway with a "Kuality is not running" banner giving the reason; no second core is started when the core mutex exists but its window is not found.
- **Logging** — no log file; debug builds log to stderr (shared with a debug core's console), release builds are a GUI-subsystem exe with no console. *(2026-11-01: same for the single exe: debug is a console app in every mode, so `--settings` started alone opens a console in debug builds only.)*
- **Accessibility** — icon-only buttons, switches and checkboxes carry accessibility labels (they double as stable names for UI automation).
- **Shortcuts editor** *(2026-11-01)* — Raycast-style table, one row per binding: enabled switch, Hotkey recorder button (`Hyper + 1`), Action button opening a searchable picker, delete. No expandable cards, no key dropdown (a hotkey that cannot be recorded is edited in `config.json`); `name` is not shown and is kept in the file. Reason: the card editor was confusing; the user's model is "press the hotkey, then pick what it does". *(2026-11-01, follow-up: columns Enabled | Name | Hotkey | Action; see "Name column".)*
- **App picker source** *(2026-11-01)* — the shell's Applications folder (`FOLDERID_AppsFolder`, the Start menu's "All apps"): desktop and Store apps alike, saved as `launch` of `shell:AppsFolder\<parsing name>` (no config change; the core already launches these through `ShellExecuteExW`). Listed once per window on thread `kuality-settings-apps` (names first, then 32 px icons through `IShellItemImageFactory`), kept in memory while the window is open.
- **Action kinds** *(2026-11-01)* — the picker's entries are derived from the existing three actions: app (`shell:AppsFolder\…` without arguments), Open website (`open_url`; a bare `github.com` gets `https://`), Send keys (`send_hotkey`), Open file or folder (`launch` of a non-program without arguments), Run program (`.exe`/`.com`/`.bat`/`.cmd`, or anything with arguments or a working folder; arguments under "Advanced"). Unknown fields inside `action` survive when the `type` stays the same.
- **Extra icons** *(2026-11-01)* — Lucide icons beyond gpui-component's default set (AppWindow, Keyboard) are embedded one by one with `icon_assets!` in front of the default asset source, not the whole catalog. `image` (already in GPUI's tree) is a direct dependency for the icon pixel buffers.
- **Name column** *(2026-11-01, user request)* — `name` is back as an optional inline text field; when empty it shows the action in grey (`Open Helium`, `Send Win + 1`). The Action cell reads `Open: [icon] Helium`.
- **Hyper icon** *(2026-11-01, user request)* — Hyper is a white four-pointed star ✦ on a dark keycap (same in light and dark mode), drawn from an SVG path (`Icon::data`), shown as `[✦] + [1]` in the Hotkey column and on the Hyperkey tab.
- **Hotkey recorder** *(2026-11-01)* — records a full chord: CapsLock held → `hyper+<key>`, else the held modifiers + key (`ctrl+alt+k`); a key alone or a reserved combination is refused with a message and recording restarts. Esc alone cancels (Ctrl+Esc is a hotkey).
- **Pass-through option** *(2026-11-01)* — "Also send to the app" lives in a per-row "⋯" popover (checkbox + one-line explanation) so the row stays clean; a row with it on shows "The app in front also gets this hotkey."

## Out of scope for now

- Notifications widget (needs package identity / MSIX).
- Auto-updates, translations, first-run wizard, per-monitor layouts, macOS/Linux.
