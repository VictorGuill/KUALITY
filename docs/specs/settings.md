# Settings app (Phase 5, scoped down)

## Goal

A window to adjust the **existing** settings (Phases 0–3) and play with them, instead of editing `config.json` by hand. No new features. Live apply works by writing `config.json`: the core already hot-reloads it (250 ms debounce). Done **before** Phase 4 (media).

Deferred (stay in the roadmap, not in this phase): named-pipe IPC, drag-to-place mode, Appearance and Extensions tabs, `autostart` / `run_elevated` UI.

## Behavior (user-visible)

- Tray **Settings** and widget right-click **Settings** open the window. A second launch brings the open window to the front.
- Centered window, title **Kuality Settings**, ~900×620 (min 720×480), resizable, follows Windows light/dark. Sidebar on the left with tabs **General**, **Widgets**, **Hyperkey**, **Shortcuts** (DECISIONS order, deferred tabs left out); the content area scrolls.
- **Auto-save, no Save button**: every valid change is written to `config.json` 300 ms after the last edit; the core reloads it. Opening the window never rewrites the file.
- Invalid input shows a red message under the field and is **not** saved; the file keeps the last valid value of that field. Other fields still save.
- Status line at the bottom of the window: `Saved` / `Saving…` / `1 field not saved` / `Could not write config.json: <error>`; plus `Kuality is not running — changes apply when it starts` when the core mutex does not exist.

### General

| Field | Control | Validation / notes |
|---|---|---|
| `general.log_level` | Select: Error, Warn, Info, Debug, Trace | always valid |
| Config file | read-only path + buttons **Open config file** (default editor) and **Open data folder** | — |

`autostart` and `run_elevated` are not shown (Phase 6); their values are preserved.

### Widgets

| Field | Control | Validation / notes |
|---|---|---|
| `widgets.enabled` | Switch "Show taskbar widgets" | — |
| `widgets.hide_on_fullscreen` | Switch "Hide when a fullscreen app is in front" | — |
| `widgets.items` | List; button **Add text widget** | — |

List row: enabled switch, text preview (first line, ellipsized) or `(empty)`, id, chevron to expand, delete button (confirm dialog). One row expanded at a time; the expanded row shows the editor inline. Entries that are not a known widget (`WidgetEntry::Unknown`, e.g. a `media` item) show as `Unsupported widget "<type>" (kept as is)` with only Delete.

Text widget editor (defaults from `TextWidget::default()`; new items get id `text-<n>`, smallest unused n, `enabled: true`):

| Field | Control | Validation |
|---|---|---|
| `text` | Multi-line text area | any; empty is allowed (core hides the widget) — hint shown |
| `id` | Text input | any; duplicate of another item's id → warning "Only the first item with this id is shown" (still saved, core rule) |
| `enabled` | Switch | — |
| `anchor` | Select: Left, Center, Right | — |
| `x`, `y` | Number input with ± steppers (step 1) + unit Select `px` / `%` | finite number |
| `origin` | Select of the 9 origins (Top left … Bottom right) | — |
| `width`, `height` | Checkbox **Auto**; when off, number input + unit Select | finite number ≥ 0 |
| `font_family` | Text input | non-empty (core falls back to Segoe UI if not installed) |
| `font_size` | Number input with steppers (step 1) | 4…200 (core clamps to the same range) |
| `font_weight` | Select: Normal, Semibold, Bold | — |
| `color` | Checkbox **Theme color**; when off, hex input + swatch | `#RRGGBB` or `#AARRGGBB` (`Color::parse`) |
| `align` | Select: Left, Center, Right | — |

A `Lenient::Invalid` value read from the file is shown as its raw JSON text with the message "Invalid in config.json, Kuality uses the default"; it is written back verbatim until the user changes that field.

### Hyperkey

| Field | Control | Notes |
|---|---|---|
| `hyperkey.enabled` | Switch "Enable Hyperkey" | Text: "Hold CapsLock to press Ctrl+Shift+Alt+Win. While enabled, CapsLock does not toggle caps and the Office key shortcut is disabled. Tray → Pause turns it off temporarily." |

### Shortcuts

| Field | Control | Notes |
|---|---|---|
| `shortcuts.enabled` | Switch "Enable shortcuts" | If Hyperkey is off: callout "Shortcuts only work while Hyperkey is enabled" + button **Enable Hyperkey** |
| `shortcuts.bindings` | List; button **Add shortcut** | — |

List row: enabled switch, `Hyper + <KEY>` badge (or `No key`), name (or action summary), action summary (`Launch notepad.exe`, `Send ctrl+shift+esc`, `Open https://…`), chevron, delete (confirm dialog). `BindingEntry::Invalid` entries show as `Unrecognized shortcut (kept as is)` with only Delete.

**Add shortcut** creates a *draft* row (expanded). A draft is written to the file only once it is valid (key + required action field); until then it lives in the window only and is lost on close (hint shown on the row).

Binding editor:

| Field | Control | Validation |
|---|---|---|
| `name` | Text input | any |
| `key` | **Record** button + searchable Select of key names (canonical names from `keys.rs`) | `parse_key` ok, not a modifier, not `capslock`. Another enabled binding with the same key → warning "Only the first enabled shortcut with this key works" (saved, core rule) |
| `enabled` | Switch | — |
| `action.type` | Segmented control: Launch / Send hotkey / Open URL | Switching type starts the new action with empty fields (old action fields are dropped once the new one is valid) |
| launch `path` | Text input + **Browse…** (native file dialog) | non-empty |
| launch `args` | Text input "Arguments" (one command line) | any. An existing list is shown joined with Windows quoting (`join_args`) and stays a list until edited; an edit is saved as a raw string (same command line for the core) |
| launch `cwd` | Text input + **Browse…** (folder dialog) | empty → `null` |
| send_hotkey `keys` | Text input + **Record** button | `parse_chord` ok |
| open_url `url` | Text input | scheme + ≤ 2048 chars (same rule as the core) |

### Key capture (Record buttons)

- Clicking **Record** shows `Press a key… (Esc to cancel)` on the button. Recording stops on: a recorded result, Esc (no modifier held), a click anywhere, the window losing focus, or 10 s.
- Capture uses a `WH_KEYBOARD_LL` hook in the settings process, installed only while recording, on a dedicated thread `kuality-settings-recorder` with its own message loop. The callback only reads the event, pushes it into a channel and returns. It ignores injected events (`LLKHF_INJECTED`). While recording, and only while the settings window is the foreground window, it **swallows** the keys (so Win+D, Ctrl+Shift+Esc… do not act); every key whose down was swallowed also has its up swallowed, even after recording ends (the hook stays until those keys are up).
- Why a hook: GPUI key events lose the virtual-key code (layout-dependent names) and never see system combos. And the hook works with **Hyperkey active**: our hook is installed after the core's, so it runs first and sees the raw CapsLock before the core turns it into Hyper modifiers; swallowing means the core does not inject modifiers or fire a bound shortcut. If the core's hook happens to run first (it reinstalls on resume/unlock), injected Hyper modifiers are ignored and a key already bound may be consumed by the core; the dropdown and text fields remain as fallback.
- **Trigger key** recorder: the first key down that is not a modifier and not CapsLock → `Key(vk).name()` (unnamed codes become `vk:0xNN`). Modifiers and CapsLock are ignored, so pressing either `G` or `CapsLock+G` records `g`.
- **Chord** recorder: modifiers pressed (in order, max 4) then a non-modifier key down → chord. Left modifiers are written as `ctrl` / `shift` / `alt` / `win`, right ones as `rctrl` / `rshift` / `ralt` / `rwin`. If all modifiers are released without another key, the chord is the modifiers alone (e.g. `win`). CapsLock is ignored (type `capslock` by hand if wanted).

## Config file handling

- **Location**: same rule as the core. The pure part of `crates/core/src/paths.rs` (`DataDir::resolve`, file names) moves to `kuality-protocol` (`protocol::paths`); both exes call it with their own exe dir (they ship side by side) and the roaming app data folder (`SHGetKnownFolderPath(FOLDERID_RoamingAppData)`). The only argument from the core is `--core-pid` (see "Lifecycle"); it does not affect the data dir.
- **Read**: at startup and on every external change. Missing file → defaults in the window; the first saved change creates it (and its folder).
- **Write**: atomic temp file + rename, as today. `config_store::save`'s write moves to `protocol::store::write_atomic(path, bytes)`, used by both. Bytes are skipped if identical to what is on disk. Pending changes are flushed when the window closes.
- **Preserve unknown fields**: the window edits the parsed `Config` in place and serializes it, so every `extra` map, every raw `Invalid` / `Unknown` entry and every `Lenient::Invalid` value survives. Gap closed in this phase: `Action` variants become newtype variants over structs with their own `extra` map (same pattern as `WidgetItem`), so unknown fields inside `action` survive too. Formatting is normalized (2-space pretty JSON, known fields in struct order, missing known fields written with defaults, unknown keys sorted — no `preserve_order`).
- **Invalid file** (not JSON or not a config): banner "config.json has an error: <serde message>. Kuality keeps its last valid settings. Fix the file — this window updates when it is valid." + **Open config file**. All controls are read-only and nothing is written, so a half-done hand edit is never clobbered.
- **`version` newer than supported**: warning banner, editing allowed (unknown fields are preserved, `version` is not changed).
- **Changes on disk while open**: a 1 s GPUI timer (also run on window activation) checks size + modified time, then compares bytes with the last read/written ones. Different and valid → the form reloads from the file (selected tab and expanded row kept when they still exist, drafts kept) and a pending debounced save is dropped (external edit wins); status `Reloaded: config.json changed on disk`. Different and invalid → invalid-file banner as above. Our own writes are recognized by their bytes.

## Launching and single instance

- `Event::OpenSettings` in the core: `kuality-settings.exe` next to `current_exe()`. If it exists → `std::process::Command::new(path).arg("--core-pid").arg(<core pid>).current_dir(exe_dir).spawn()` (not awaited, handle dropped); if it is missing or the spawn fails → log and fall back to the old behaviour (open `config.json` in the default editor).
- The settings exe uses `windows_subsystem = "windows"` in release (like the core); in debug it is a console app, so when started from a debug core its `log` output lands in the core's console. No log file.
- Single instance: named mutex `Local\Kuality.Settings`. A second instance finds the first one's top-level window (title `Kuality Settings`, owning process image `kuality-settings.exe`), restores it if minimized, calls `SetForegroundWindow` (it can: it was just launched by the foreground core or by the user) and exits with code 0.

## Lifecycle (follow-up: Kuality behaves as one app)

The settings window belongs to the core: it never outlives it, and opening it starts the core when needed.

- **Attach**, in order: the process given by `--core-pid` (only if it is a running `kuality.exe`; if it is already gone, the settings app exits without a window); else the running core of the session, found by its `Kuality.Core` window (waits up to 3 s while its mutex exists but the window does not yet); else **start** `kuality.exe` from the settings exe's own folder (same folder, so the same portable/data-dir rule) and attach to it.
- **Watch**: a background thread `kuality-settings-core-watch` waits on the core process handle (`SYNCHRONIZE`). When the core exits for any reason (tray Quit, `taskkill`, `taskkill /F`, crash) the window cancels a recording, writes a pending auto-save at once and quits.
- **No core**: if `kuality.exe` is missing or cannot be started, the window still opens, with a warning banner "Kuality is not running" and the reason; edits are saved to `config.json` and apply when Kuality starts. (Also when the core's mutex exists but its window cannot be found: then no second core is started.)
- Closing the settings window never stops the core.

## Implementation notes

```
crates/protocol/src/paths.rs      DataDir (moved from core), file-name consts
crates/protocol/src/store.rs      write_atomic(path, bytes)
crates/protocol/src/quote.rs      join_args (moved from core/actions/quote.rs)
crates/protocol/src/config.rs     Action newtype variants + extra; valid_url(); Key::is_trigger() in keys.rs
crates/settings/src/main.rs       single instance, data dir, GPUI app + window
crates/settings/src/model.rs      document state: Config, disk bytes, validation, debounced save, reload (unit-tested, no GPUI)
crates/settings/src/ui/           sidebar + one view per tab, list/editor components
crates/settings/src/platform/     Windows: known folder, mutex + activate, core-running check, key recorder hook
crates/core/src/app.rs            OpenSettings launches the exe, fallback to the editor
```

- UI state per field is a GPUI `InputState` entity; on every change the value is validated, and if valid applied to the model's `Config`, then a 300 ms save timer is (re)started.
- The core-running check is `OpenMutexW(SYNCHRONIZE, "Local\Kuality.Core")`, polled with the 1 s timer.
- Theme: gpui-component theme synced to the system appearance and re-synced when it changes.

### Dependencies

- `gpui-kit = "=0.8.1"` (crates.io, default features `component` + `assets`): the umbrella crate of gpui-component; it pins `gpui-pre = 0.3.10` (published GPUI snapshot), `gpui-component 0.8.1`, `gpui-base`, and the Lucide icon assets. Exact pin because GPUI APIs change between releases. Settings crate only; the core gains no dependency.
- `windows` (workspace 0.62) in the settings crate: mutex, window lookup, LL hook, known folder.
- `log` (workspace) with a tiny stderr logger in debug builds.

Build check (2026-10-31, throwaway crate outside the repo, rustc 1.99.1 stable-x86_64-pc-windows-msvc): `gpui-kit =0.8.1` hello window with Switch, Input and Button built in release with no patches or extra tools (2 min 41 s cold), exe 24 MB; it opened its window, ~100 MB working set while open. This weight is acceptable only because the settings process lives just while its window is open; the core is untouched.

## Edge cases

| Case | Handling |
|---|---|
| Core not running | Settings starts `kuality.exe` from its own folder and attaches to it. If that is impossible: banner + status hint; changes apply when it starts. |
| Settings started directly (`cargo run -p kuality-settings`) | Same data-dir rule, same file; starts the core if none is running. |
| File missing | Defaults shown; first change creates it. |
| File invalid / newer version | See "Config file handling". |
| External edit while a field has an unsaved invalid value | Reload wins; the invalid text is replaced by the file value. |
| Write fails (locked, read-only, disk full) | Status error; retried on the next change and on close. |
| Hand-made key alias (`"escape"`, `"G"`) | Shown as its canonical name in the Select but written back verbatim unless changed. |
| Recording while another app has focus | Never: recording stops on deactivation and keys are only swallowed while our window is in front. |
| Settings window closed while a key is held during recording | Hook thread is joined after the remaining ups are swallowed or 1 s, whichever comes first. |
| Core quits or is killed while the window is open | The window saves a pending change and closes (see "Lifecycle"). |

## Out of scope

Named pipe IPC, drag-to-place, Appearance and Extensions tabs, autostart / run elevated toggles, reordering list items (order is kept; edit `config.json` to reorder), "test this shortcut" button, font picker, undo.

## Test plan

Automated (`cargo test`): `model.rs` — load/save round trip keeps unknown fields at every level (incl. inside `action`), `Lenient::Invalid` kept, no write when unchanged, invalid file blocks writes, external change detection by bytes, draft bindings not written until valid, new widget id allocation; `protocol` — moved paths/quote tests, `Action` extra round trip, `valid_url`, `Key::is_trigger`; `core` — existing tests still pass.

Manual (portable setup in `target\debug`): open Settings from the tray and from a widget; second launch focuses the window; each tab: change values and see `config reloaded, changed: …` in the core log and the effect (log level, Hyperkey on/off, a new shortcut works, a text widget appears/moves/restyles); invalid input not saved; hand-added unknown fields survive; invalid file → read-only banner; external edit reloads the form; record a key and a chord with Hyperkey on and off; core idle RAM/CPU unchanged.

## Assumptions

1. Phase 5 is done before Phase 4 and limited to Phases 0–3 settings (per the approved plan).
2. Live apply = writing `config.json`; no named pipe yet (DECISIONS line to be added; the pipe line stays as the later target).
3. `gpui-kit` (the umbrella crate of gpui-component) is used rather than `gpui-component` + `gpui` separately, pinned exactly; egui stays the fallback.
4. Tab order follows DECISIONS (General, Widgets, Hyperkey, Shortcuts); Appearance/Extensions are hidden, not shown as disabled.
5. Auto-save debounce 300 ms; external-change poll 1 s (the settings process is short-lived, a timer is simpler than a directory watcher).
6. Invalid field values are never written; a `Lenient::Invalid` value already in the file is kept verbatim until changed.
7. An invalid `config.json` makes the window read-only (no "reset to defaults" button).
8. New shortcuts are drafts until valid; drafts are not persisted across window closes.
9. Editing launch arguments stores them as one raw string; untouched lists stay lists.
10. `Action` gets `extra` maps so unknown fields inside actions survive (small protocol change, core code updated accordingly).
11. Key recording uses a global LL hook only while recording and only swallowing while our window is in front; CapsLock is never recorded; Esc alone cancels.
12. Delete asks for confirmation (dialog); there is no undo and no reordering.
13. The settings app has no log file; stderr only in debug builds.
14. Single instance via `Local\Kuality.Settings` mutex + window lookup by title and process name (no IPC).
15. General tab gets "Open config file" / "Open data folder" buttons as escape hatches (no new feature behind them).
