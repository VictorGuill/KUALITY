# To-do + taskbar bar (Phase 8, done before Phase 4)

## Goal

A local to-do list. You add items, check them off (they stay, struck through) and delete any item for good. The list opens from a small `TO DO (n)` on the taskbar. It is stored on this PC only (no sync) and survives restarts.

The taskbar part is the first module of a new **bar** widget: one widget with one position that holds several modules side by side. To-do is the first module; "now playing" (Phase 4) joins it later.

## Behavior (user-visible)

- **Settings → To do** (new tab):
  - **Enable to-do** (off by default). Turning it on also makes sure an enabled bar shows the to-do module: it enables the first bar that has one, or adds `bar-<n>` at the bottom-left of the taskbar (anchor left, x 12 px, centered vertically).
  - **New items go to**: Top or Bottom.
  - **To-do list**: an "Open to-do list" button (opens the popup at the bottom-left of the primary screen).
  - **Taskbar label** (default `TO DO`) and **Show the number of pending items**. Both edit the first to-do module.
  - A warning with **Turn on** when taskbar widgets are off, and **Add to the taskbar** when no enabled bar shows the module.
- **Taskbar**:
  - The module shows `TO DO (3)`: the label plus the number of items not done yet. The count is left out at 0, when it is turned off, and when `todos.json` is invalid.
  - Hovering shows a rounded highlight (like taskbar buttons).
  - A click opens the popup above it; clicking it again while the popup is open closes it.
- **Popup** (`kuality.exe --todo`, a separate process like Settings):
  - About 340 × 440 DIPs, borderless, rounded, topmost, no taskbar button.
  - Its left edge sits at the module's left edge and its bottom 8 DIPs above the taskbar, kept inside the work area.
  - Layout: header "To do" with "n left" / "All done", then an input ("Add a task…", focused; Enter adds), then the list. Pending items come first and done ones after, struck through and dimmed.
  - A checkbox (or a click on the text) toggles done. A trash button deletes the item at once, with no undo (user decision).
  - Esc or clicking anywhere else closes it. Every change is saved at once.
- **Settings → Widgets**: bars appear as "Bar: To do" cards. The editor is the text widget's (id, position, size, font, size, weight, color) without Text/Align, plus **Modules** (a "To do" checkbox; unknown modules listed as kept) and **Space between modules**.

## Config

`config.json`:

```json
"todo": { "enabled": false, "new_items": "top" },
"widgets": { "items": [
  { "id": "bar-1", "type": "bar", "enabled": true, "anchor": "left", "x": "12px", "y": "50%",
    "origin": "left", "width": "auto", "height": "auto",
    "font_family": "Segoe UI Variable Text", "font_size": 12.0, "font_weight": "semibold",
    "color": null, "gap": 16.0,
    "modules": [ { "type": "todo", "label": "TO DO", "show_count": true } ] } ] }
```

- `todo.new_items` is `Lenient` (`top` | `bottom`).
- Bar fields follow the text widget rules (`Lenient` values, `extra` kept).
- `gap` is clamped to 0–200 DIPs.
- `modules` entries are `Known(BarModule)` or raw JSON (`Unknown`, kept on save, logged at Info and skipped).
- The to-do module only shows while `todo.enabled` is on. A bar with no visible module is not drawn.

`todos.json` (data dir, next to `config.json`):

```json
{ "version": 1, "items": [
  { "id": 3, "text": "Buy milk", "done": false, "created": 1794260750 },
  { "id": 2, "text": "Call bank", "done": true, "created": 1794200000, "done_at": 1794250000 } ] }
```

- Types are in `protocol::todo`: `#[serde(default)]` and `extra` everywhere.
- `id` is max + 1. `created` / `done_at` are Unix seconds.
- Text is one line, trimmed, at most 500 characters.
- Missing file = empty list.

## Implementation notes

- **Only the popup writes `todos.json`** (atomic `store::write_atomic`, on every change). The core and Settings only read it.
- **Core**:
  - The config watcher already wakes on any write in the data dir. Its callback also runs `todos::TodoWatch` (byte compare), which sends the pending count on a channel.
  - `App` forwards it as `OverlayCommand::Data(LiveData)`. The overlay thread re-renders without re-enumerating taskbars and keeps session "Hide".
  - `overlay::resolve` takes `todo.enabled`. An Apply is also sent when only `todo.enabled` changed.
- **Bar rendering** (`overlay/widgets/bar.rs`):
  - Each module is a "pill": text plus 8 DIP padding on each side, 6 DIP above and below.
  - Modules are laid out left to right `gap` apart. `spans()` gives the x-ranges for hit testing, stored per widget window.
  - Hover fills the hovered pill with `Theme::hover_color` (`Renderer::fill_rounded_rect`).
- **Clicks**:
  - Widget windows now handle `WM_LBUTTONDOWN/UP`, `WM_MOUSEMOVE` and `WM_MOUSELEAVE` (`TrackMouseEvent`). They only post to the host (`WM_APP_CLICK`, `WM_APP_HOVER`).
  - The host maps the click to a module. For to-do it posts `WM_APP_TOGGLE_TODO` (anchor: module left, taskbar top, physical px) to the main window, which becomes `Event::ToggleTodo`.
- **Toggle**:
  - Pressing on the taskbar takes the focus from the popup, which closes itself before the click arrives. So the widget window records at `WM_LBUTTONDOWN` whether the popup's mutex (`Local\Kuality.Todo`) exists, a lookup that never blocks.
  - If it existed, the click only closes the popup (posts `WM_CLOSE` to its window) and never opens another one.
- **Popup** (`settings/src/todo.rs`, GPUI):
  - `WindowKind::PopUp` (`WS_EX_TOOLWINDOW | WS_EX_TOPMOST`).
  - Bounds come from `--at` with `MonitorFromPoint` + work area + `GetDpiForMonitor`, converted to GPUI logical units (physical / scale) with `display_id` = the `HMONITOR`.
  - `DWMWCP_ROUND` corners, `SetForegroundWindow` (the core called `AllowSetForegroundWindow` before spawning).
  - It closes on deactivation (ignored for the first 400 ms) and on Esc.
  - It attaches to the core (`--core-pid`, else `find_core`) and quits when the core exits. It never starts the core.
  - Single instance: mutex `Local\Kuality.Todo`, and a second launch exits.

## Edge cases

- **Invalid `todos.json`**: the core logs a warning and shows the label without a count. The popup shows the error, disables editing and never writes.
- **To-do disabled**: the module and its bar disappear, and `todos.json` is kept.
- **Widgets off**: nothing is drawn; the To do tab warns and offers **Turn on**.
- **Explorer restart / monitor or DPI change**: the bar follows the existing overlay rules (re-attach, relayout).
- **Popup opened by hand** (`kuality.exe --todo` or Settings): bottom-left of the primary work area.
- **Several taskbars**: the popup opens on the monitor that was clicked.

## Out of scope

- Editing an item's text, reordering, due dates, reminders.
- A hotkey or shortcut action to open the list (easy follow-up).
- Sync; the media module (Phase 4).

## Test plan

- **Automated**:
  - protocol: todo list add/toggle/remove/order/text cleaning/round trip; bar and module parsing, unknown modules kept, `todo` section defaults.
  - core: bar resolve (switch, unknown modules, invalid fields), spans, hover fill, label formatting; `TodoWatch` change detection and counts; `overlay::resolve` with bars.
  - settings: popup placement (anchor, clamping, scale, no anchor), `--at` parsing, `todos.json` load, `new_items` reading, `ensure_todo_bar`, gap rule.
- **Manual** (portable `target\debug`, 3840 × 2160 at 150 %, tested 2026-11-09):
  - [x] `TO DO (2)` appears at the bottom-left; the click opens the popup above it.
  - [x] Add (Enter): the item is saved and the taskbar shows `TO DO (3)` within a second.
  - [x] Check one: struck through and moved down, count 2. Delete one: removed from the file.
  - [x] Widget click while open closes it (3/3). A click elsewhere closes it. Esc closes it.
  - [x] Settings → To do: off hides the bar, on shows it again; the Widgets tab shows "Bar: To do" with its editor.
  - [x] Invalid `todos.json`: `TO DO` without a count, the popup shows the error, the file is untouched.
  - [ ] Explorer restart, second monitor, light taskbar (overlay rules unchanged; not re-tested).

## Assumptions

- One to-do list. The first to-do module of the first bar is the one the To do tab edits.

## Dependencies

- None new. Settings uses the existing `windows` crate with the added `Win32_UI_HiDpi` feature.
