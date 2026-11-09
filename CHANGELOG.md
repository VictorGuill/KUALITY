# Changelog

All notable changes to Kuality. Versions follow [Semantic Versioning](https://semver.org); each `## [x.y.z]` section becomes the notes of the GitHub Release tagged `vx.y.z` (keep one line per paragraph or bullet: release notes keep line breaks).

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
