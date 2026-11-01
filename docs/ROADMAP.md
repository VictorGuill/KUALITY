# Roadmap

Work phase by phase. The **current phase** is the first one not marked ✅.
Each phase gets a spec in `docs/specs/` before coding and its own branch (`phase-N-name`).

**MVP = Phases 0–3.** Phase 5 was done before Phase 4 (user request: a settings window first, no new features).

---

## Phase 0 — Skeleton ✅

- [x] Cargo workspace with `crates/core`, `crates/settings`, `crates/protocol`
- [x] Config types in `protocol` (serde, defaults = everything disabled)
- [x] Load/save `config.json`, create with defaults if missing
- [x] Portable mode (`portable` file next to exe)
- [x] Config hot-reload (file watcher)
- [x] Logging to file (`%APPDATA%\Kuality\logs` or portable dir)
- [x] Single instance (named mutex)
- [x] Tray icon with menu: Settings (placeholder) / Quit
- [x] Per-monitor DPI awareness v2 manifest
- [x] `cargo build`, `clippy`, `test` pass; commands documented in `AGENTS.md`

**Done when:** `kuality.exe` starts, shows a tray icon, creates/reads `config.json`, reacts to edits, and quits cleanly.

## Phase 1 — Hyperkey ✅

- [x] Spec `docs/specs/hyperkey.md`
- [x] Low-level keyboard hook on a dedicated thread
- [x] CapsLock held → Ctrl+Shift+Alt+Win down; released → up; CapsLock itself swallowed
- [x] Injected events tagged, no feedback loops, no key-repeat spam
- [x] Mask key before releasing so Start menu / menu bar don't open
- [x] Office key disabled while enabled, restored when disabled
- [x] Toggle via `hyperkey.enabled` (hot-reload) and tray "Pause"
- [x] Safe recovery: modifiers never left stuck (on quit, crash, lock screen)

**Done when:** holding CapsLock + T in any app is received as Ctrl+Shift+Alt+Win+T, tapping it alone does nothing visible.

## Phase 2 — Shortcuts ✅

- [x] Spec `docs/specs/shortcuts.md`
- [x] Bindings Hyper+key from config
- [x] Actions: `launch`, `send_hotkey`, `open_url`
- [x] Bound shortcuts are consumed (not forwarded); unbound ones pass through as the Hyper combo
- [x] *(2026-11-01, follow-up)* Any hotkey (`ctrl+t`, `win+e`…) besides Hyper+key, "Also send to the app" option, Hyperkey "Include Windows key" option; settings table with a Name column and the ✦ Hyper keycap

**Done when:** Hyper+1 opens the configured program and Hyper+2 sends the configured hotkey.

## Phase 3 — Taskbar overlay ✅

- [x] Spec `docs/specs/overlay.md`
- [x] Widget host window as child of every taskbar (primary + secondary)
- [x] Direct2D/DirectWrite rendering, DPI aware
- [x] Positioning: `x`/`y` in px or %, `anchor`, `origin`, `width`
- [x] Re-attach on Explorer restart (`TaskbarCreated`), handle monitor/DPI changes
- [x] Hide on fullscreen apps
- [x] Follow Windows light/dark theme
- [x] First widget: static `text`
- [x] Right-click on widget → small menu (Settings / Hide)

**Done when:** a text widget appears exactly where config says, stays above the taskbar while clicking it, and survives an Explorer restart.

---

## Phase 4 — Media widget

- [ ] Spec `docs/specs/media.md`
- [ ] Current session: cover art, title, artist
- [ ] Controls: previous / play-pause / next
- [ ] Updates on session/track change; hides when nothing is playing (configurable)

## Phase 5 — Settings app ✅

Done before Phase 4 and scoped to the settings that already exist (Phases 0–3); see `docs/specs/settings.md`.

- [x] Spec `docs/specs/settings.md`
- [x] GPUI window (gpui-kit), centered, sidebar tabs: General, Widgets, Hyperkey, Shortcuts
- [x] Live apply by writing `config.json` (auto-save, the core hot-reloads it); opened from tray and widget menu
- [x] Shortcut editor (record key / chord, choose action)
- [x] Widget editor (text widgets: position, size, style)

**Done when:** tray → Settings opens the window, every change is applied by the running core within a second, and invalid input never reaches the file.

### Later (moved out of Phase 5)

- [ ] Named pipe IPC with the core (live commands, state the file cannot carry)
- [ ] Drag-to-place mode for widgets
- [ ] Appearance and Extensions tabs (Extensions with Phase 7)
- [x] Autostart / Run elevated toggles in the General tab (with Phase 6)

## Phase 6 — Startup & distribution

- [x] Spec `docs/specs/distribution.md`
- [x] One executable: the settings window runs as `kuality.exe --settings` *(2026-11-01, user decision)*
- [x] Autostart toggle (`HKCU\...\Run`)
- [x] "Run elevated" toggle (Task Scheduler, one UAC prompt)
- [ ] Inno Setup installer (with elevated-autostart checkbox)
- [ ] Portable zip
- [ ] App icon, version info, release build profile (size-optimized)

## Phase 7 — Extensions

- [ ] Spec `docs/specs/extensions.md`
- [ ] Manifest `kuality-extension.json` + JSON-over-stdio protocol in `protocol`
- [ ] Extension host in core (spawn, lifecycle, permissions)
- [ ] Extensions contribute widgets data, actions, settings schema
- [ ] Settings app: Extensions tab (install/enable/configure)
- [ ] Catalog repo (`catalog.json` + sha256 zips)
- [ ] First extension: fan speeds (C# + LibreHardwareMonitor)

## Future (unordered)

- First-run wizard presenting each feature
- Real CapsLock via double-tap
- Notifications widget
- Auto-updates
- Per-monitor layouts
- Translations
- macOS / Linux ports
