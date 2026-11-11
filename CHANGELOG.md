# Changelog

All notable changes to Kuality. Versions follow [Semantic Versioning](https://semver.org); each `## [x.y.z]` section becomes the notes of the GitHub Release tagged `vx.y.z` (keep one line per paragraph or bullet: release notes keep line breaks).

## [Unreleased]

### New

- **Clipboard history** — Settings → **Clipboard** → Enable keeps what you copy (text and images). Press **Hyper + V** (or record any hotkey, for example **Win + V** to replace the Windows clipboard history) to open it: type to search, ↑/↓ to choose, **Enter** pastes into the app you were in, **Shift+Enter** only copies. **Ctrl+P** pins an entry so it survives restarts, **Ctrl+Delete** removes it.
- **Private by design** — the history stays in memory only and encrypted; pinned entries are saved encrypted for your Windows account. Passwords copied from password managers that mark them private are never recorded.
- **Search in Settings** — the Settings window has a search field: type a word ("hotkey", "startup"…) to find a setting in any tab.

### Changed

- **New look** — Settings, the to-do list and the clipboard history now use the standard look of the UI toolkit (colors, corners and font), light or dark with Windows. This prepares choosing and customizing themes later.
- **Action picker** — when choosing what a shortcut does, "Open website…", "Send keys…", "Open file or folder…" and "Run program…" are listed first, then your apps; typing searches both.
- **Widget colors** — a color picker next to the color field of a widget.

### Fixed

- Editing settings no longer interrupts a held Hyper key: the keyboard hook is only restarted when Hyperkey, shortcuts or the clipboard hotkey change (before, on every saved change).
- Copying large images uses far less memory for a moment (no full uncompressed copies), and starting with pinned images is faster.
- A clipboard history window that stopped responding could block the history for the next one; requests now time out.

## [1.0.0-beta.4] - 2026-11-10

### New

- **To-do list** — Settings → **To do** → Enable puts a small `TO DO (3)` at the bottom-left of the taskbar. Rest the mouse on it to see your list (it closes again when you move away), or click it to open it: type and press Enter to add an item, check it off (it stays, struck through) or delete it with its trash button. Esc or a click elsewhere closes it. The list is stored on this PC only and survives restarts.
- **Taskbar bar** — the to-do lives in a new kind of taskbar widget, a bar that holds several items at one position (now playing will join it). Move it and change its font in Settings → Widgets.

### Upgrading

Kuality 1.0.0-beta.2 or later shows a notification: click it (or tray → **Install update**). From 1.0.0-beta.1, install this version once by hand. The to-do list is off until you turn it on in Settings → **To do**.

### Downloads

- `Kuality-1.0.0-beta.4-setup.exe` — installer (per user, no admin rights needed).
- `Kuality-1.0.0-beta.4-portable.zip` — unzip anywhere and run `kuality.exe`.

The files are not code-signed yet: Windows SmartScreen may show "Windows protected your PC". Click **More info → Run anyway**. Checksums are in `SHA256SUMS.txt`.

## [1.0.0-beta.3] - 2026-11-09

### Changed

- **New look for the Settings window** — rounder corners, more space between items, the Windows 11 font (Segoe UI Variable), the page shown as a rounded panel next to the sidebar, filled buttons and smoother option selectors. Follows the Windows light/dark mode as before.
- **The title bar matches the window** — it uses the window's background color, so the whole window looks like one piece (snap layouts and the window buttons work as before).
- **New text widgets start turned off** — adding one no longer shows a placeholder on the taskbar right away; turn it on from its switch when it is ready.

### Upgrading

Kuality 1.0.0-beta.2 shows a notification: click it (or tray → **Install update**). From 1.0.0-beta.1, install this version once by hand.

### Downloads

- `Kuality-1.0.0-beta.3-setup.exe` — installer (per user, no admin rights needed).
- `Kuality-1.0.0-beta.3-portable.zip` — unzip anywhere and run `kuality.exe`.

The files are not code-signed yet: Windows SmartScreen may show "Windows protected your PC". Click **More info → Run anyway**. Checksums are in `SHA256SUMS.txt`.

## [1.0.0-beta.2] - 2026-11-09

### New

- **Automatic updates** — Kuality checks GitHub for a new version when it starts and every 6 hours, and shows a notification when one is out. Click it (or tray → **Install update**) to download it, verify its checksum and install it; Kuality restarts by itself and your start-with-Windows choice is kept.
- **Tray → Check for updates**, and a new **Updates** section in Settings → General (turn automatic checks off, check now, see what's new, install).
- Portable copies are notified too; "install" opens the download page.

### Upgrading from 1.0.0-beta.1

Beta 1 has no updater: install this version once by hand (download `Kuality-1.0.0-beta.2-setup.exe` below and run it). From now on Kuality updates itself.

### Downloads

- `Kuality-1.0.0-beta.2-setup.exe` — installer (per user, no admin rights needed).
- `Kuality-1.0.0-beta.2-portable.zip` — unzip anywhere and run `kuality.exe`.

The files are not code-signed yet: Windows SmartScreen may show "Windows protected your PC". Click **More info → Run anyway**. Checksums are in `SHA256SUMS.txt`.

## [1.0.0-beta.1] - 2026-11-09

First public beta. Windows 11, x64.

### Highlights

- **Hyperkey** — hold CapsLock for a layer of shortcuts. By default apps never see it (Kuality-only); optionally apps get Ctrl+Shift+Alt or Ctrl+Shift+Alt+Win. Modifiers are never left stuck (quit, lock screen, sleep, crash), and a hook removed by Windows is reinstalled automatically.
- **Shortcuts** — Hyper+key or any hotkey (Ctrl+Alt+K, Win+E…) to launch an app or program, open a file, folder or website, or send another hotkey. Bound keys are consumed; "Also send to the app" is optional.
- **Taskbar widgets** — text widgets drawn on top of the taskbar of every monitor, placed in px or %, following the Windows light/dark theme, hidden over fullscreen apps, surviving Explorer restarts.
- **Settings window** — General, Widgets, Hyperkey and Shortcuts tabs; every change is saved and applied right away.
- **Start with Windows** — optionally as administrator, so Hyperkey and shortcuts also work over elevated windows.
- **Lightweight** — one `kuality.exe`; the always-running core uses a few MB of RAM and ~0% CPU when idle. Everything is disabled until you turn it on.

### Downloads

- `Kuality-1.0.0-beta.1-setup.exe` — installer (per user, no admin rights needed).
- `Kuality-1.0.0-beta.1-portable.zip` — unzip anywhere and run `kuality.exe`.

The files are not code-signed yet: Windows SmartScreen may show "Windows protected your PC". Click **More info → Run anyway**. Checksums are in `SHA256SUMS.txt`.

### Known limitations

- Keys pressed in windows running as administrator are not seen unless Kuality runs as administrator too (Settings → General → Startup → Run as administrator).
- No media widget yet (next phase), no automatic updates yet.
