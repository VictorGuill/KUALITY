# Changelog

All notable changes to Kuality. Versions follow [Semantic Versioning](https://semver.org); each `## [x.y.z]` section becomes the notes of the GitHub Release tagged `vx.y.z` (keep one line per paragraph or bullet: release notes keep line breaks).

## [Unreleased]

### Changed

- **Faster popups** — the to-do list now opens on hover almost at once: Kuality starts it in the background as soon as the mouse reaches `TO DO`, and it appears about 0.1 s later (was 250 ms plus the time to start, about 0.4 s). A click on `TO DO` while the mouse rests there opens it faster too, and the "loading" cursor no longer shows next to the pointer. The clipboard history (Hyper+V) also opens a little faster.

## [1.1.0] - 2026-11-16

### New

- **App lock** — Settings → **App lock**: pick programs (from the apps running now, or Browse…) and set a Kuality password. When a locked program starts, its windows are hidden and Kuality asks for the password; once you type it, the program stays unlocked until you close it. After 5 wrong passwords Kuality makes you wait. While App lock is on, its settings tab and tray Quit / Pause ask for the password too. It keeps out someone using your unlocked PC; it is not meant as real security.

### Changed

- **Pause** — tray → Pause now also pauses App lock: hidden programs come back, and programs started while paused stay unlocked until you close them.

### Upgrading

Kuality 1.0.0 shows a notification: click it (or tray → **Check for updates**, then **Install update**). Your settings are kept; App lock stays off until you turn it on.

### Downloads

- `Kuality-1.1.0-setup.exe` — installer (per user, no admin rights needed).
- `Kuality-1.1.0-portable.zip` — unzip anywhere and run `kuality.exe`.

The files are not code-signed yet: Windows SmartScreen may show "Windows protected your PC". Click **More info → Run anyway**. Checksums are in `SHA256SUMS.txt`.

## [1.0.0] - 2026-11-14

The first stable version of Kuality. Windows 11, x64. Everything below is off until you turn it on in Settings, except the update check.

### New since 1.0.0-beta.5

- **Stable or beta updates** — Settings → General → Updates → **Channel**. **Stable** (the default) only offers final versions like this one. **Beta** also offers test versions of the next release, before everyone gets them; they may have rough edges, and bug reports are very welcome. Switching back to Stable keeps the version you have until a newer final version is out.

### What Kuality does

- **Hyperkey** — hold CapsLock for a layer of shortcuts. By default apps never see it; optionally they get Ctrl+Shift+Alt or Ctrl+Shift+Alt+Win. Modifiers are never left stuck (quit, lock screen, sleep, crash).
- **Shortcuts** — Hyper+key or any hotkey (Ctrl+Alt+K, Win+E…) to launch an app, open a file, folder or website, or send another hotkey.
- **Taskbar widgets** — text and bars drawn on top of the taskbar of every monitor, placed in px or %, following the Windows light/dark theme, hidden over fullscreen apps.
- **To-do list** — `TO DO (n)` on the taskbar; click it, or rest the mouse on it, to add, check off and delete items.
- **Clipboard history** — Hyper+V (or any hotkey, even Win+V) opens what you copied, text and images: search, Enter pastes into the app you were in, pin entries to keep them. Kept in memory and encrypted; pinned entries are saved encrypted for your Windows account; passwords that password managers mark private are never recorded.
- **Settings window** — every change is saved and applied right away; search finds any setting.
- **Start with Windows** — optionally as administrator, so Hyperkey and shortcuts also work over elevated apps.
- **Automatic updates** — a notification when a new version is out, installed in one click after its checksum is verified. Can be turned off.
- **Lightweight** — one `kuality.exe`; the always-running core uses a few MB of RAM and ~0% CPU when idle.

### Upgrading

Kuality 1.0.0-beta.2 or later shows a notification: click it (or tray → **Install update**). From 1.0.0-beta.1, install this version once by hand. After updating from a beta you are on the **Stable** channel; choose **Beta** in Settings → General → Updates to keep getting test versions.

### Downloads

- `Kuality-1.0.0-setup.exe` — installer (per user, no admin rights needed).
- `Kuality-1.0.0-portable.zip` — unzip anywhere and run `kuality.exe`.

The files are not code-signed yet: Windows SmartScreen may show "Windows protected your PC". Click **More info → Run anyway**. Checksums are in `SHA256SUMS.txt`.

## [1.0.0-beta.5] - 2026-11-14

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

### Upgrading

Kuality 1.0.0-beta.2 or later shows a notification: click it (or tray → **Install update**). From 1.0.0-beta.1, install this version once by hand. The clipboard history is off until you turn it on in Settings → **Clipboard**.

### Downloads

- `Kuality-1.0.0-beta.5-setup.exe` — installer (per user, no admin rights needed).
- `Kuality-1.0.0-beta.5-portable.zip` — unzip anywhere and run `kuality.exe`.

The files are not code-signed yet: Windows SmartScreen may show "Windows protected your PC". Click **More info → Run anyway**. Checksums are in `SHA256SUMS.txt`.

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
