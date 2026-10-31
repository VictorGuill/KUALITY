# Taskbar overlay + text widget (Phase 3)

## Goal

Draw widgets on top of every Windows 11 taskbar (primary and secondary), positioned exactly as configured, without injecting into Explorer. First widget type: static `text`. Survives Explorer restarts, monitor/DPI changes, sleep/resume; hides over fullscreen apps; follows light/dark theme; right-click gives a small menu.

## Behavior (user-visible)

- `widgets.enabled = false` (default): no windows, no event hooks, nothing drawn.
- `widgets.enabled = true`: each item with `enabled: true` and a known `type` is drawn on **every** taskbar at the configured position. Items of unknown types (e.g. `media` before Phase 4) are ignored (one `info` log line per load).
- The widget stays visible above the taskbar content while the user clicks the taskbar, opens Start, hovers icons, etc.
- Text color follows the Windows mode (dark taskbar → white text, light taskbar → near-black), unless `color` is set.
- When the foreground window is fullscreen on a monitor, the widgets on that monitor's taskbar hide; they reappear when it stops being fullscreen or loses the foreground (`widgets.hide_on_fullscreen`, default `true`).
- Right-click a widget → menu: **Settings**, **Hide**. Settings does what the tray "Settings" item does. Hide hides that item on all taskbars for the current session: it comes back when the core restarts or when the `widgets` section of `config.json` changes. The file is not modified (the core never rewrites an existing config, see skeleton.md).
- Config edits (text, position, enable/disable) apply live via hot-reload.
- Pause (tray) does not affect widgets.

## Config

```json
"widgets": {
  "enabled": false,
  "hide_on_fullscreen": true,
  "items": [
    { "id": "text-1", "type": "text", "enabled": true,
      "anchor": "right", "x": "-200px", "y": "50%", "origin": "center",
      "width": "auto", "height": "auto",
      "text": "Hello", "font_family": "Segoe UI Variable Text", "font_size": 12,
      "font_weight": "normal", "color": null, "align": "left" }
  ]
}
```

Section:

| Key | Type | Default |
|---|---|---|
| `widgets.enabled` | bool | `false` |
| `widgets.hide_on_fullscreen` | bool | `true` |
| `widgets.items` | array | `[]` |

Common item fields (all widget types):

| Key | Type | Default | Meaning |
|---|---|---|---|
| `id` | string | `""` | Stable identifier. Empty/missing → runtime id `#<index>`. Duplicate ids: later item ignored with `warn`. |
| `type` | string | required | `"text"` in this phase. |
| `enabled` | bool | `true` | Per-item switch. |
| `anchor` | `"left"` \| `"center"` \| `"right"` | `"left"` | Horizontal reference point of the taskbar. |
| `x` | length | `"0px"` | Horizontal offset from the anchor; positive = right. `%` of taskbar width. |
| `y` | length | `"50%"` | Offset from the taskbar's top edge; positive = down. `%` of taskbar height. |
| `origin` | origin | `"left"` | Which point of the widget is placed at (x, y). |
| `width` | length \| `"auto"` | `"auto"` | `auto` = content width. `%` of taskbar width. |
| `height` | length \| `"auto"` | `"auto"` | `auto` = content height. `%` of taskbar height. |

`text` item fields:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `text` | string | `""` | Text to show. `\n` makes extra lines; no word wrap. Truncated to 1000 chars. Empty → widget not shown. |
| `font_family` | string | `"Segoe UI Variable Text"` | Falls back to `"Segoe UI"` if not installed. |
| `font_size` | number | `12` | In DIPs (px at 100 % scale). Clamped to 4…200. |
| `font_weight` | `"normal"` \| `"semibold"` \| `"bold"` | `"normal"` | |
| `color` | null \| `"#RRGGBB"` \| `"#AARRGGBB"` | `null` | `null` = theme color. Invalid → theme color + `warn`. |
| `align` | `"left"` \| `"center"` \| `"right"` | `"left"` | Horizontal text alignment inside the widget box (matters when `width` is fixed). Text is always vertically centered in the box. |

### Value types (`protocol/src/config.rs`)

- **length**: JSON string `"<number>px"`, `"<number>%"`, or a JSON number (= px). Number may be negative and fractional. `px` are **DIPs** (multiplied by the monitor scale). Serialized back as `"<n>px"` / `"<n>%"`. Invalid → field default + `warn`.
- **origin**: `"top_left"`, `"top"`, `"top_right"`, `"left"`, `"center"`, `"right"`, `"bottom_left"`, `"bottom"`, `"bottom_right"`. Fractions (ox, oy): left/center/right → 0 / 0.5 / 1 horizontally; top/center/bottom → 0 / 0.5 / 1 vertically; a bare side (`left`, `right`) is vertically centered, a bare `top`/`bottom` is horizontally centered.
- Phase 0's `Widgets` struct keeps its fields; only `items` changes from `Vec<serde_json::Value>` to `Vec<WidgetEntry>`. Types derive `PartialEq`.
- Items: `#[serde(untagged)] enum WidgetEntry { Known(WidgetItem), Unknown(serde_json::Value) }`; `WidgetItem` = common fields (flattened) + `#[serde(tag = "type", rename_all = "snake_case")] enum WidgetKind { Text(TextWidget) }`. Unknown types are kept as raw JSON on save. Like the Phase 0 structs, `WidgetItem` has one `#[serde(flatten)] extra: serde_json::Map` declared **last** (after the flattened common fields and kind), so unknown fields survive a round trip.

### Layout (portable, `core/src/overlay/layout.rs`)

```rust
pub fn layout(item: &CommonItem, taskbar_px: SizePx, scale: f32, content_dip: SizeF) -> Option<RectPx>
```

```
W, H     = taskbar client size in physical px
res(v,R) = v.px * scale   |   v.pct / 100 * R
w        = width  == auto ? ceil(content.w * scale) : res(width,  W)
h        = height == auto ? ceil(content.h * scale) : res(height, H)
ax       = 0 | W/2 | W            (anchor left | center | right)
px, py   = ax + res(x, W),  res(y, H)
left     = round(px - ox * w),  top = round(py - oy * h)
```

Returns `None` if `w < 1` or `h < 1`. Coordinates are relative to the taskbar's client area. No clamping: parts outside the taskbar are clipped by the parent.

## Implementation

### Code layout

```
crates/protocol/src/config.rs                  Widgets (typed items), WidgetEntry, WidgetItem, TextWidget, Length, Anchor, Origin
crates/core/src/overlay/mod.rs                 OverlayController (API for main), reconcile logic
crates/core/src/overlay/layout.rs              pure layout math (unit-tested)
crates/core/src/overlay/widgets/text.rs        text widget: measure + draw via the Renderer trait
crates/core/src/platform/windows/overlay/
    thread.rs        overlay thread, hidden window, message routing, timers
    taskbars.rs      enumerate taskbars, explorer pid, WinEvent hooks for taskbar changes
    widget_window.rs widget window class, layered update, input, context menu
    render.rs        Direct2D/DirectWrite (implements Renderer)
    fullscreen.rs    foreground fullscreen detection
    theme.rs         SystemUsesLightTheme
```

Portable code sees a `Renderer` trait (`measure_text`, `begin(size, scale)`, `draw_text`, `end -> bitmap`) and a `Surface` id; Win32 types stay in `platform`.

### Thread

- Thread `kuality-overlay`, spawned at startup, alive until quit; same "channel + `WM_APP_COMMAND` wake" pattern as the keyboard thread.
- `OverlayCommand::Apply(Arc<Widgets>)`, `OverlayCommand::Shutdown`.
- Owns a hidden **top-level** window `KualityOverlayHost` (`WS_POPUP`, never shown, `WS_EX_TOOLWINDOW`) that receives broadcasts: `TaskbarCreated`, `WM_DISPLAYCHANGE`, `WM_SETTINGCHANGE`, `WM_POWERBROADCAST`, and WTS session notifications (`WTSRegisterSessionNotification`). Call `ChangeWindowMessageFilterEx(hwnd, TaskbarCreated, MSGFLT_ALLOW)` so it still arrives when the core runs elevated (Phase 6).
- **Hard rule:** widget windows are children of Explorer's taskbar, so this thread's input queue is attached to Explorer's. The overlay thread must never block: no file/registry writes (except the one theme registry read), no `SendMessage` to Explorer windows, no waiting on other threads. Only `GetWindowRect`/`GetClientRect`/`FindWindowExW`/`GetWindowThreadProcessId`/`MonitorFromWindow`/`GetDpiForMonitor` on Explorer windows.
- When `widgets.enabled` becomes false: destroy all widget windows, unhook all WinEvent hooks, stop timers. D2D/DWrite factories are created lazily on first enable and kept.

### Taskbars (`taskbars.rs`)

- Enumerate: `FindWindowW("Shell_TrayWnd")` + loop `FindWindowExW(None, prev, "Shell_SecondaryTrayWnd", None)`. Each taskbar → `TaskbarHost { hwnd, monitor, dpi, client_size, hidden_by_fullscreen, windows: Vec<(item_key, WidgetWindow)> }`.
- Scale = `GetDpiForMonitor(MonitorFromWindow(taskbar, MONITOR_DEFAULTTONEAREST), MDT_EFFECTIVE_DPI) / 96`.
- If `Shell_TrayWnd` is not found (core started before the shell): retry every 1 s for 30 s, then rely on `TaskbarCreated`.
- Explorer pid from `GetWindowThreadProcessId(Shell_TrayWnd)`. Two out-of-context WinEvent hooks limited to that pid (`WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS`):
  - `EVENT_OBJECT_REORDER` → if `hwnd` is one of our taskbars, re-assert our windows at `HWND_TOP` (in item order).
  - `EVENT_OBJECT_LOCATIONCHANGE` → if `idObject == OBJID_WINDOW` and `hwnd` is one of our taskbars and its **client size** changed → schedule relayout. Pure moves (auto-hide slide) need nothing: children move with the parent.
  - Re-registered after every `TaskbarCreated` (new pid).
- **Full refresh** = re-enumerate taskbars, reuse hosts whose `hwnd` is still valid, create hosts for new ones, drop hosts whose `hwnd` is gone, then relayout + render everything. Debounced through a one-shot timer (`SetTimer`, 300 ms; re-arming resets it). Triggers:
  - `TaskbarCreated` (Explorer restart) — also immediately forget all hosts (their windows were destroyed with the old taskbar);
  - `WM_DISPLAYCHANGE`, any `WM_SETTINGCHANGE` (covers scale changes and "show taskbar on all displays");
  - `PBT_APMRESUMEAUTOMATIC`, `WTS_SESSION_UNLOCK`;
  - a widget window receiving `WM_DESTROY` that we did not initiate (its taskbar died);
  - `WM_DPICHANGED_AFTERPARENT` on a widget window;
  - `Apply`.
  After `WM_DISPLAYCHANGE` and `TaskbarCreated` also schedule two extra refreshes at 2 s and 5 s (Explorer creates secondary taskbars asynchronously).

### Widget windows (`widget_window.rs`)

- One window **per (taskbar, item)**, sized to the item's rect (not one big window: cross-process children cannot pass clicks through transparent areas).
- Class `KualityWidget`, cursor `IDC_ARROW`, no background brush.
- `CreateWindowExW(WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_NOPARENTNOTIFY, "KualityWidget", "", WS_CHILD | WS_CLIPSIBLINGS, x, y, w, h, parent = taskbar hwnd, ...)`, then `SetWindowPos(HWND_TOP, ..., SWP_NOACTIVATE | SWP_SHOWWINDOW)`.
- Layered child windows need the exe manifest to declare Windows 8+ compatibility; the Phase 0 manifest already lists the Windows 10/11 `supportedOS` (verify it is present).
- Position changes: `SetWindowPos` (no `UpdateLayeredWindow` position). Content changes: `UpdateLayeredWindow(hwnd, null, null, &size, memdc, &(0,0), 0, &BLENDFUNCTION{AC_SRC_OVER, 0, 255, AC_SRC_ALPHA}, ULW_ALPHA)`.
- Reconcile on Apply/refresh: key = (taskbar hwnd, item id). Keep existing windows, create missing, destroy removed; then relayout and re-render all (cheap; nothing renders otherwise).
- Z-order among our widgets: config order, later items on top.
- Input: `WM_CONTEXTMENU` (or `WM_RBUTTONUP`) → menu. Other mouse input is ignored in this phase.

### Rendering (`render.rs`)

- Factories: `D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED)`, `DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)`.
- Per window: a 32-bpp top-down DIB section (`CreateDIBSection`, BGRA) selected into a memory DC, recreated only when the size changes.
- One `ID2D1DCRenderTarget` per thread: `D2D1_RENDER_TARGET_TYPE_SOFTWARE` (no D3D device: less RAM, no device-lost handling), `DXGI_FORMAT_B8G8R8A8_UNORM`, `D2D1_ALPHA_MODE_PREMULTIPLIED`, DPI = monitor DPI (so drawing is in DIPs). `BindDC` to the window's memory DC before each draw.
- Draw: `Clear(transparent)`; fill the whole box with premultiplied black at alpha 1/255 (invisible, but makes the whole rect hit-testable — fully transparent pixels of layered windows are click-through); draw content; `EndDraw`; `UpdateLayeredWindow`.
- Text: `IDWriteTextFormat` (family, weight, normal style, size in DIPs, locale `""`), `DWRITE_WORD_WRAPPING_NO_WRAP`, `DWRITE_PARAGRAPH_ALIGNMENT_CENTER`, text alignment from `align`, trimming with ellipsis (`CreateEllipsisTrimmingSign`) when wider than a fixed width. Measure with `IDWriteTextLayout` (max width = 10000 DIPs for `auto`), use `widthIncludingTrailingWhitespace` and `height`. `SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE)` (ClearType does not work on transparent surfaces).
- Font fallback: if `GetSystemFontCollection().FindFamilyName(font_family)` fails, use `"Segoe UI"`.
- Rendering happens only on Apply, refresh and theme change. Fullscreen hide/un-hide is just `ShowWindow` (no re-render). No periodic timers, no animation → ~0 % CPU idle.

### Theme (`theme.rs`)

- Read `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\SystemUsesLightTheme` (DWORD; missing → dark). The taskbar follows the *system* mode, not the apps mode.
- Default text color: dark → `#FFFFFF`, light → `#1B1B1B`.
- On `WM_SETTINGCHANGE` with `lParam` string `"ImmersiveColorSet"` → re-read, re-render all.

### Fullscreen (`fullscreen.rs`)

Only when `hide_on_fullscreen` is true:
- WinEvent hook `EVENT_SYSTEM_FOREGROUND` (global, out-of-context, skip own process).
- WinEvent hook `EVENT_OBJECT_LOCATIONCHANGE` limited to the **foreground window's process id**, re-registered whenever the foreground changes (catches F11 / going fullscreen without a focus change). In the callback act only when `idObject == OBJID_WINDOW && idChild == CHILDID_SELF && hwnd == current foreground`.
- Check (`is_fullscreen(fg)`): false if `fg` is null, the shell/desktop (`GetShellWindow()`, classes `Progman`, `WorkerW`, `Shell_TrayWnd`, `Shell_SecondaryTrayWnd`) or one of our windows. Otherwise `m = MonitorFromWindow(fg, MONITOR_DEFAULTTONULL)`; fullscreen iff `GetWindowRect(fg)` contains `MONITORINFO.rcMonitor` **and not** (`IsZoomed(fg)` and style has `WS_CAPTION`) — excludes maximized normal windows when the taskbar auto-hides.
- Result: the taskbar host on monitor `m` gets `hidden_by_fullscreen = true` (its widgets `ShowWindow(SW_HIDE)`); all other hosts `false` (`SW_SHOWNA`). Only act when the state flips.

### Context menu

- `CreatePopupMenu` with `Settings` (id 1), `Hide` (id 2). `SetForegroundWindow(hidden host window)`, `TrackPopupMenuEx(TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, cursor pos, owner = hidden host window)`, then `PostMessageW(host, WM_NULL)`. Plain Win32 menu, same as the Phase 0 tray menu (no extra dark-mode work in this phase).
- Settings → send `AppEvent::OpenSettings` to the main thread (`app.rs`), which runs the same handler as the tray Settings item (Phase 0: open `config.json` in the default editor; Phase 5: launch the settings app).
- Hide → add the item's runtime id (`id`, or `#<index>`) to the overlay thread's `session_hidden` set and destroy its windows on every taskbar. Reconcile skips items in `session_hidden`. On `Apply`, if the new `Widgets` value differs from the previous one (`PartialEq`), clear `session_hidden`. No message to the main thread, no file write.

### Integration with main

- `OverlayController::start()` at startup; `apply(Arc<Widgets>)` on every config load; `shutdown()` on quit (destroy windows, unhook, `PostQuitMessage`, join with 1 s timeout).
- The main thread's event enum in `app.rs` (Phase 0: config reloaded, tray commands, quit) gets an `OpenSettings` variant usable from other threads (channel + `PostMessageW` to the `Kuality.Core` window, same as the watcher).

## Edge cases

| Case | Handling |
|---|---|
| Explorer restart / crash | Children die with the taskbar; `TaskbarCreated` → full refresh + delayed refreshes; WinEvent hooks re-registered for the new pid. |
| Core starts before Explorer | 1 s retry for 30 s, then `TaskbarCreated`. |
| Monitor added/removed, "show taskbar on all displays" toggled | `WM_DISPLAYCHANGE` / `WM_SETTINGCHANGE` → refresh (+2 s, +5 s). |
| Scale change, taskbar moves to another DPI | Client-size change / `WM_SETTINGCHANGE` / `WM_DPICHANGED_AFTERPARENT` → relayout with the monitor's DPI, re-render. |
| Taskbar auto-hide | Child windows slide with the taskbar; nothing to do. |
| Taskbar size change (small icons, touch taskbar) | `EVENT_OBJECT_LOCATIONCHANGE` with new client size → relayout. |
| Explorer reorders its children (our widget pushed behind) | `EVENT_OBJECT_REORDER` → `HWND_TOP` again. |
| Sleep/resume, lock/unlock | Refresh on resume and unlock. |
| Fullscreen app on one monitor | Only that monitor's widgets hide. |
| Maximized window with auto-hide taskbar | Not fullscreen (zoomed + caption). |
| Theme switch | Re-render with new default color. |
| Empty text / zero size | Window hidden (not created). |
| Widget outside the taskbar | Clipped by the parent; no clamping. |
| Vertical/top taskbar (unsupported layouts via tweaks) | Same math on the client rect; not tested or supported. |
| Core hangs | Would freeze taskbar input (attached queues) — prevented by the no-blocking rule; all slow work is on other threads. |
| Core exits/crashes | Windows destroys our windows with the thread; the taskbar is left clean. |

## Out of scope

- Media widget (Phase 4), any widget other than `text`, widget click actions, tooltips, animations.
- Drag-to-place, per-monitor layouts, vertical taskbars, high-contrast themes, Windows 10.
- Background/border/padding styling of widgets.
- `Appearance` theme override (later, settings app).

## Test plan

Automated:
- `Length` parsing: `"10px"`, `"-200px"`, `"35%"`, `12`, `"1.5px"`, `"auto"` where allowed; invalid strings → default.
- `Origin`/`Anchor` parsing and fractions.
- `layout`: anchor left/center/right; negative x with right anchor (`-200px` at 150 % → 300 px left of the right edge); `%` x/y/width/height; origin `center`, `top_left`, `bottom_right`; auto size with scale 1.0/1.25/1.5 (ceil); `None` for zero size.
- Config: the ARCHITECTURE example (with a `media` item) parses; `media` becomes `Unknown` and round-trips unchanged; defaults applied for a minimal `{ "type": "text", "text": "x" }`; duplicate ids handled by reconcile helper.
- Color parsing: `#RRGGBB`, `#AARRGGBB`, invalid.

Manual (Windows 11, `widgets.enabled = true`, one text widget):
1. Widget appears exactly where configured; check `left`/`center`/`right` anchors, `%` and `px`, every origin, at 100 % and 150 % scale (measure with a screenshot).
2. Click the taskbar, open Start, hover icons, open the tray overflow → widget stays visible on top.
3. `taskkill /f /im explorer.exe` then `start explorer` → widget back within ~2 s; repeat 5 times.
4. Two monitors with "show taskbar on all displays": widget on both; unplug/replug the second monitor; change its scale; toggle the setting.
5. Browser F11, a borderless fullscreen game, a fullscreen video → widgets on that monitor hide, others stay; exiting fullscreen restores. Maximized Notepad with auto-hide taskbar → widget stays.
6. Switch Windows mode light ↔ dark → text color follows.
7. Sleep/resume and lock/unlock → widget still there and correct.
8. Edit text/position/enabled in `config.json` → updates live. Add a `media` item → ignored, no error.
9. Right-click → Settings does what the tray item does; Hide → widget disappears on all taskbars, `config.json` unchanged; editing any widget field brings it back; restart brings it back.
10. Turn on auto-hide taskbar → widget slides with it.
11. Idle 60 s with 5 text widgets → 0 % CPU in Task Manager; private memory increase vs widgets disabled ≤ 5 MB.
12. `widgets.enabled = false` → all widgets gone, no WinEvent hooks left (log at debug).

## Assumptions

1. One layered child window per (taskbar, item) instead of a single host window per taskbar (click-through of transparent areas does not work across processes).
2. `px` values are DIPs, scaled by the taskbar monitor's DPI.
3. `y` is always measured from the taskbar's top edge; `anchor` is horizontal only (as in DECISIONS).
4. Defaults: `anchor "left"`, `x "0px"`, `y "50%"`, `origin "left"`, `width/height "auto"`; origin values use snake_case (`top_left` …).
5. Text widget fields `text`, `font_family` (`Segoe UI Variable Text` → fallback `Segoe UI`), `font_size` 12 DIPs, `font_weight`, `color` (null = theme), `align`; multi-line via `\n`, no wrap.
6. Rendering uses a software `ID2D1DCRenderTarget` + `UpdateLayeredWindow` (no D3D/DirectComposition) to keep memory low.
7. The whole widget box is filled with alpha 1/255 so it is fully clickable.
8. Theme follows `SystemUsesLightTheme` (taskbar mode), default colors `#FFFFFF` / `#1B1B1B`.
9. Fullscreen = foreground window covering its monitor and not a maximized captioned window; detected with WinEvent hooks, hidden per monitor (the "foreground covers monitor" option of ARCHITECTURE.md, no `SHQueryUserNotificationState`).
10. "Hide" in the context menu is session-only (until restart or the next change of the `widgets` section), because skeleton.md establishes that the core never rewrites an existing config; Phase 5 can make it persistent through the settings app.
11. Unknown widget types are preserved in the file and ignored at runtime.
12. Pause does not hide widgets.
13. Integration follows skeleton.md: `app.rs` forwards each reloaded `Config` to the overlay; its event enum gains `OpenSettings` (posted from the overlay thread); the Phase 0 manifest already declares PerMonitorV2 and the Windows 10/11 `supportedOS` needed for layered child windows.
14. The widget context menu is a plain Win32 menu like the Phase 0 tray menu (no dark-mode menu hack in this phase).
15. If on some Windows 11 build a child window of `Shell_TrayWnd` cannot be drawn above the XAML taskbar content, the implementer stops and reports it; there is no automatic fallback (DECISIONS fixes `SetParent` embedding).

## Dependencies

No new crates. Additional `windows` features in `crates/core`: `Win32_Graphics_Direct2D`, `Win32_Graphics_Direct2D_Common`, `Win32_Graphics_DirectWrite`, `Win32_Graphics_Dxgi_Common` (pixel format), `Win32_Graphics_Gdi` (DIB section, memory DC, `UpdateLayeredWindow` types, monitors), `Win32_UI_HiDpi` (`GetDpiForMonitor`), `Win32_UI_Accessibility` (WinEvent hooks), `Win32_System_RemoteDesktop` (session notifications), `Win32_System_Registry` (theme), `Win32_UI_Shell` (shared with Phase 2).
