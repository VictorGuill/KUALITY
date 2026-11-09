# Vision

## What Kuality is

A small, fast, always-running Windows 11 utility that lets the user:

1. **Show anything on the taskbar**: widgets placed exactly where the user wants (absolute px or %), always visible on top of the taskbar. Examples: currently playing media (with cover and controls), custom text, fan speeds, notifications.
2. **Use CapsLock as a Hyperkey**: holding CapsLock gives a whole new layer of shortcuts that never collide with other apps.
3. **Bind actions to shortcuts**: Hyper+key launches a program, sends another hotkey, opens a URL… (more complex actions later).
4. **Configure everything in a clean settings window** (Raycast-like: centered window, tabs on the left).
5. **Extend it**: installable extensions (any language) that add widgets and actions, e.g. "read fan speeds", "control program X".

## Principles

- **Lightweight**: a few MB of RAM idle, ~0% CPU. This is the main reason the project is in Rust and not Electron.
- **Opt-in**: everything is disabled by default; the user enables what they want. A first-run wizard will come later.
- **Native look**: follows Windows light/dark theme, Fluent-like visuals.
- **Open source**: MIT license, contributions and third-party extensions welcome.

## Non-goals (for now)

- macOS / Linux support (code is kept portable-friendly, but no ports planned).
- Windows 10 support.
- Translations (UI is English only).
- Auto-updates. *(2026-11-09: planned, update check enabled by default; see DECISIONS "Releases & CI".)*
- Multi-monitor specific layouts (all monitors show the same widgets).
- Injecting code into Explorer (unlike Windhawk mods).
