# AGENTS.md

Entry point for any AI agent (Claude Code, Codex, Cursor, …) working on **Kuality**.
Read this file fully before doing anything.

## What is Kuality

A lightweight, open-source (MIT) Windows 11 utility written in Rust:

- **Taskbar widgets** drawn on top of the Windows taskbar (media player, text, later sensors…).
- **Hyperkey**: CapsLock acts as Ctrl+Shift+Alt+Win while held.
- **Shortcuts**: Hyper+key runs actions (launch program, send hotkey, open URL…).
- **Settings app**: Raycast-style window to configure everything.
- **Extensions** (later): external processes that provide widgets data and actions.

Details: [docs/VISION.md](docs/VISION.md).

## Where things are

| File | Purpose |
|---|---|
| [docs/VISION.md](docs/VISION.md) | Goals and non-goals |
| [docs/DECISIONS.md](docs/DECISIONS.md) | Every decision taken, one line each. **Do not contradict it without asking the user.** |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Processes, crates, threads, IPC, config format |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Phases, tasks (checkboxes) and definition of done |
| `docs/specs/` | Detailed spec per feature, written when its phase starts |

## How to work

1. Open [docs/ROADMAP.md](docs/ROADMAP.md) and find the **current phase** (first one not completed). Work only on that phase unless the user says otherwise.
2. If the phase has no spec in `docs/specs/` yet, write it first (short, concrete). If the user already approved a plan for the phase (e.g. in plan mode), that counts as confirmation: write the spec and continue straight to the implementation. Otherwise confirm the spec with the user before coding.
3. One branch per phase (`phase-N-short-name`), small focused commits.
4. When you finish a task, tick its checkbox in `ROADMAP.md` in the same commit.
5. If you make a new decision (library choice, behavior, format…), add a line to `DECISIONS.md`.
6. Never leave the build broken: `cargo build`, `cargo clippy` and `cargo test` must pass before committing.

## Hard rules

- **Language**: code, comments, UI text, docs and commit messages in **English**. (The user may talk to you in Spanish.)
- **Lightweight first**: the core process must stay a few MB of RAM and ~0% CPU when idle. No webviews, no Electron, no heavy runtimes in the core.
- **Everything is opt-in**: every feature can be enabled/disabled and is **disabled by default**.
- **Windows 11 only**, but keep OS-specific code behind traits/modules so a future port only rewrites those parts.
- **No code injection** into `explorer.exe` or other processes.
- The core must survive Explorer restarts, sleep/resume and monitor changes without crashing.
- Low-level keyboard hook callbacks must return immediately; any real work goes to another thread.
- Do not add dependencies casually: prefer the `windows` crate and small, well-maintained crates.

## Commands

```powershell
cargo build                                 # debug build (core keeps a console with log output)
cargo build --release                       # release build (no console)
cargo fmt --all                             # format
cargo clippy --all-targets -- -D warnings   # lint, must be clean
cargo test                                  # unit tests (all crates)
cargo run -p kuality                        # run the core (tray icon; Ctrl+C in the console quits cleanly)
cargo run -p kuality -- --settings          # settings window (second process of the same exe; starts the core if needed)
cargo run -p kuality -- --todo              # to-do popup (bottom-left of the primary screen without --at <x>,<y>)
cargo run -p kuality -- --startup off       # start-at-logon entry: off | on | normal | elevated (UAC)
.\scripts\package.ps1 [-Installer]          # release build + dist\Kuality-<version>-portable.zip (+ Inno Setup installer)
cargo run -p kuality --example keyviewer    # dev tool: prints key messages + modifier state (Hyperkey tests)
cargo run -p kuality-update --example check -- 1.0.0-alpha --download   # dev tool: update check + verified download (no install)
```

Runtime files: `%APPDATA%\Kuality\config.json`, `todos.json` (to-do items) and `logs\kuality.log`.
For a throwaway setup, create an empty file `target\debug\portable`: config and logs then live in `target\debug\`.
To quit a running core from a script, close it gracefully (`taskkill /IM kuality.exe`, without `/F`).
Startup tests: in debug builds set `KUALITY_STARTUP_NAME=Kuality-test` so the Run value / scheduled task use a test name instead of the real `Kuality` entry.
Update tests: in debug builds set `KUALITY_UPDATE_AS_VERSION=1.0.0-alpha` so the core and settings window see the published releases as newer (a dev copy only opens the release page, never installs).
There is one executable: `kuality.exe` is the core, `kuality.exe --settings` the settings window (tray / widget "Settings" start it as a second process), `kuality.exe --todo` the to-do popup (a click on the taskbar's `TO DO`); only one settings window and one popup run at a time.
