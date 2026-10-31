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

## Docs

- [Vision](docs/VISION.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Decisions](docs/DECISIONS.md)
- [Roadmap](docs/ROADMAP.md)

## License

[MIT](LICENSE)
