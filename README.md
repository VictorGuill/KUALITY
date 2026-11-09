<div align="center">

<img src="assets/icon/png/kuality-128.png" width="96" height="96" alt="Kuality logo">

# Kuality

**A tiny Windows 11 utility for power users: CapsLock Hyperkey, global shortcuts and taskbar widgets.**

Written in Rust. A few MB of RAM, ~0% CPU when idle. Everything is off until you turn it on.

[![CI](https://github.com/VictorGuill/KUALITY/actions/workflows/ci.yml/badge.svg)](https://github.com/VictorGuill/KUALITY/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/VictorGuill/KUALITY?include_prereleases&label=release)](https://github.com/VictorGuill/KUALITY/releases)
[![Downloads](https://img.shields.io/github/downloads/VictorGuill/KUALITY/total)](https://github.com/VictorGuill/KUALITY/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Windows 11](https://img.shields.io/badge/Windows-11-0078D4?logo=windows11&logoColor=white)

[**Download**](https://github.com/VictorGuill/KUALITY/releases) · [Features](#features) · [Getting started](#getting-started) · [Building](#building-from-source) · [Roadmap](docs/ROADMAP.md)

</div>

---

> [!NOTE]
> Kuality is in **beta**: expect rough edges — [bug reports](https://github.com/VictorGuill/KUALITY/issues/new/choose) are very welcome.

## Features

| | Feature | What it does |
|---|---|---|
| ✅ | **Hyperkey** | Hold **CapsLock** for a whole new layer of shortcuts that never collide with other apps. By default apps never see it; optionally they get Ctrl+Shift+Alt(+Win). Tapping CapsLock alone does nothing. |
| ✅ | **Shortcuts** | **Hyper+key** or any hotkey (`Ctrl+Alt+K`, `Win+E`…) to launch an app, open a file, folder or website, or send another hotkey. |
| ✅ | **Taskbar widgets** | Text drawn right on top of the taskbar, on every monitor, placed exactly where you want (px or %). Follows light/dark theme, hides over fullscreen apps. |
| ✅ | **Settings window** | A clean, Raycast-style window. Every change is saved and applied instantly. |
| ✅ | **Start with Windows** | Optionally as administrator, so Hyperkey and shortcuts also work over elevated apps. |
| 🚧 | **Media widget** | Cover art, title and controls for whatever is playing. *Next up.* |
| 🚧 | **Automatic updates** | Get notified when a new version is out and install it in one click. |
| 🗓️ | **Extensions** | Widgets and actions written in any language (fan speeds, app control…). |

### Why Kuality?

- **Lightweight first** — native Win32 + Direct2D, no Electron, no webview. The always-running core stays at a few MB.
- **Opt-in** — nothing happens until you enable it. No telemetry.
- **Safe** — no code injection into Explorer or any other process; modifiers are never left stuck (lock screen, sleep, crash).
- **One file** — a single `kuality.exe`, installable or portable.

## Getting started

1. Download the latest release from the [**Releases**](https://github.com/VictorGuill/KUALITY/releases) page:
   - `Kuality-<version>-setup.exe` — installer, per user, **no admin rights** needed.
   - `Kuality-<version>-portable.zip` — unzip anywhere and run `kuality.exe`; config and logs stay in that folder.
2. Run it. A **tray icon** appears; click it to open the settings window.
3. Turn on what you want: **Hyperkey** first, then add a few **Shortcuts** (for example `Hyper+T` → Windows Terminal).

> [!TIP]
> The files are not code-signed yet, so Windows SmartScreen may say *"Windows protected your PC"*. Click **More info → Run anyway**. You can check the files against `SHA256SUMS.txt` in each release.

Config lives in `%APPDATA%\Kuality\config.json` (plain JSON, reloaded live if you edit it by hand) and logs in `%APPDATA%\Kuality\logs\`. Start with Windows is set in **Settings → General → Startup** or with `kuality.exe --startup <off|on|normal|elevated>`.

## Building from source

Requirements: Windows 11, [Rust](https://rustup.rs) (stable, MSVC toolchain) and Visual Studio Build Tools with the C++ workload.

```powershell
cargo run -p kuality                # run the core with a console showing the log
cargo run -p kuality -- --settings  # open the settings window
cargo build --release               # target\release\kuality.exe
.\scripts\package.ps1 -Installer    # dist\: portable zip (+ installer if Inno Setup 6 is installed)
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the dev workflow and how releases are made.

## Documentation

- [Vision](docs/VISION.md) — goals and non-goals
- [Architecture](docs/ARCHITECTURE.md) — processes, crates, threads, config format
- [Decisions](docs/DECISIONS.md) — every design decision, one line each
- [Roadmap](docs/ROADMAP.md) — phases and what's next
- [Changelog](CHANGELOG.md)

## License

[MIT](LICENSE)
