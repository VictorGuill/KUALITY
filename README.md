# Kuality

Lightweight Windows 11 utility: taskbar widgets, CapsLock Hyperkey and keyboard shortcuts. Written in Rust, open source (MIT).

> 🚧 Early development. See [docs/ROADMAP.md](docs/ROADMAP.md).

## Features (planned)

- **Taskbar widgets** — media player, custom text and more, placed anywhere on top of the taskbar.
- **Hyperkey** — hold CapsLock to get Ctrl+Shift+Alt+Win.
- **Shortcuts** — Hyper+key to launch apps, send hotkeys, open URLs.
- **Settings app** — clean window to configure everything.
- **Extensions** — add widgets and actions written in any language.

Everything is disabled by default; enable only what you need.

## Building

Requirements: Windows 11, [Rust](https://rustup.rs) (MSVC toolchain), Visual Studio Build Tools with the C++ workload.

```powershell
cargo build --release
```

## Running (dev mode)

```powershell
cargo build                     # builds the core and the settings app
cargo run -p kuality
```

Starts the core with a console showing the log; a tray icon appears. Press Ctrl+C in the console or use the tray menu → Quit to stop it.

Tray menu → **Settings** (or right-click a taskbar widget → Settings) opens the settings window. Changes are saved automatically and applied by the running core right away. You can also start the window on its own with `cargo run -p kuality-settings`: it starts Kuality first if it is not running. Kuality and its settings window behave as one app: quitting Kuality (tray → Quit) also closes the window, after saving any pending change.

Config lives in `%APPDATA%\Kuality\config.json` (created with everything disabled on first run) and is reloaded live when it changes (from the settings window or a text editor). For a throwaway setup, create an empty file `target\debug\portable`: config and logs then live in `target\debug\`.

## Docs

- [Vision](docs/VISION.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Decisions](docs/DECISIONS.md)
- [Roadmap](docs/ROADMAP.md)

## License

[MIT](LICENSE)
