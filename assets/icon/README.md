# App icon

| File | Used for |
|---|---|
| `kuality.ico` | Embedded in `kuality.exe` as icon resource 1 (`crates/core/build.rs`): Explorer, taskbar, Start menu, settings window, tray icon, installer. Must contain 16, 20, 24, 32, 40, 48, 64 and 256 px images. |
| `kuality-256.png`, `kuality-512.png` | Docs, README, store pages (not used by the build). |

The current files are a **placeholder** (white "K" on a blue rounded square). To change the logo, replace the
files (same names) and rebuild; nothing else needs to change.
