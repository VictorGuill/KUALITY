---
name: verify-ui
description: Check Kuality's GPUI windows (settings window, to-do popup, clipboard popup) with screenshots, clicks and scrolls from a script, without stealing the user's focus. Use after changing UI code, to confirm the change in the real window before reporting it done.
---

# Verify UI

`capture.ps1` (next to this file) finds a window by title, shows it without activating it, can post a
click or wheel notches, then captures it with `PrintWindow(…, PW_RENDERFULLCONTENT)`, which works while
the window is behind others. Read the PNG with the Read tool.

```powershell
$cap = ".claude\skills\verify-ui\capture.ps1"
Copy-Item target\debug\config.json <scratchpad>\config.backup.json      # back up the dev config
cargo build -p kuality
$p = Start-Process target\debug\kuality.exe -ArgumentList '--settings' -WindowStyle Minimized -PassThru
pwsh -NoProfile -File $cap -Out <scratchpad>\s1.png                       # waits for the window
pwsh -NoProfile -File $cap -Out <scratchpad>\s2.png -Scroll 5             # scroll the page down
pwsh -NoProfile -File $cap -Out <scratchpad>\s3.png -ClickX 1518 -ClickY 437   # click (capture pixels)
taskkill /PID $p.Id                                                       # close it (no /F)
Copy-Item <scratchpad>\config.backup.json target\debug\config.json -Force
```

Window titles: `Kuality Settings`, `Kuality To-do`, `Kuality Clipboard`.

## Setup and safety

- Dev copies use `target\debug\portable` + `target\debug\config.json` (the user's own test config):
  back it up before a test that changes settings and restore it after.
- The installed Kuality usually runs; a dev `--settings` window works next to it (it edits the dev
  config only). A dev **core** cannot start while the installed one runs: ask before closing it.
- Popups close when they lose focus. The clipboard popup has a debug-only sample mode
  (`KUALITY_CLIPBOARD_SAMPLE=1`: made-up entries, stays open). Never put test data on the user's real
  clipboard.
- Update UI: `$env:KUALITY_UPDATE_AS_VERSION='1.0.0-alpha'` (debug builds) makes published releases
  look newer.

## Pitfalls

- PowerShell passes `$null` to a `string` P/Invoke parameter as `""`: use `[NullString]::Value`
  (`FindWindow(class = null, title)` otherwise finds nothing).
- The script calls `SetProcessDPIAware`; without it window sizes are scaled and captures are cropped.
- Give the window a few seconds after a rebuild (the script polls for the title for 30 s).
- Typing: post `WM_CHAR` per character; keys: `WM_KEYDOWN` / `WM_KEYUP`. GPUI handles posted input.
- Git Bash heredocs on this machine mangle `\\` and odd apostrophes (Rust lifetimes): write scripts
  that contain Rust code to a file with the Write tool instead of a heredoc.
