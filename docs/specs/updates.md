# Updates

## Goal

Kuality tells the user when a new version is published on GitHub and installs it in one click,
without making the core heavier: no new runtime, no polling, nothing installed without asking.

## Behavior (user-visible)

- **Setting** — `updates.check` in `config.json` (Settings → General → Updates → "Check for updates
  automatically"). **On by default**: the one exception to "everything is opt-in" (user decision
  2026-11-09). Off: no automatic network access; manual checks still work.
- **When** — 60 s after the core starts, then every 6 h; 30 min after a failed check. The wall clock
  is re-read at least every 15 min, so a check that fell due while the PC slept runs soon after resume.
- **Which releases** — the newest GitHub release newer than the running version (SemVer precedence)
  with both `Kuality-<version>-setup.exe` and `SHA256SUMS.txt`. Drafts are ignored. Pre-releases are
  offered only to pre-release builds: a beta gets the next beta or the final release; a release build
  only gets releases.
- **Notification** — the tray icon's balloon (a toast on Windows 11): "Kuality X is available — You
  have Y. Click to install it." Shown once per version and run (again after a manual check). The tray
  menu shows **Install update X** while an update is known, **Check for updates** otherwise; a manual
  check also notifies "up to date" or the error.
- **Install** (installed copy) — download the installer and `SHA256SUMS.txt` from the release, check
  the SHA-256, write it to `%TEMP%\Kuality-update\`, start it with
  `/SILENT /SUPPRESSMSGBOXES /NORESTART /update=1`. The installer shows only its progress window,
  closes Kuality (and the settings window), installs, keeps the start-at-logon entry as it is and
  starts Kuality again. The download folder is deleted at the next start. A failure shows a
  notification and is logged.
- **Portable / development copies** (a `portable` file next to the exe, or an exe that is not the
  installer's `InstallLocation`) — "install" opens the release page instead.
- **Settings → General → Updates** — the switch, "Version X" with the result of a check run when the
  window opens, **Check now**, and when an update exists **What's new** (release page) and
  **Install X** (**Download X** for portable copies). Installing from the window works like the tray.

## Implementation

- Crate `kuality-update` (shared by core and settings): `Version` (SemVer parse/compare),
  `pick_update` (releases JSON → `Release`), `expected_hash` (`SHA256SUMS.txt`), `Checker` (keeps the
  `ETag`: repeated checks are conditional requests; a `304` does not count against GitHub's 60
  requests/hour/IP limit), `download_and_install`, `can_self_update`, `clean_downloads`.
- Windows side: WinHTTP (`WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`, redirects to the asset CDN followed,
  timeouts 15/15/30/60 s), CNG `BCryptHash` for SHA-256, `InstallLocation` from
  `HKCU\…\Uninstall\{6C1C4F0E-2B7A-4C55-9E7B-4B2D7C1A9F31}_is1`. `winhttp.dll` and `bcrypt.dll` are
  delay-loaded, so an idle core never maps them before the first check.
- API: `GET https://api.github.com/repos/VictorGuill/KUALITY/releases?per_page=20`, headers
  `User-Agent: Kuality/<version>`, `Accept: application/vnd.github+json`,
  `X-GitHub-Api-Version: 2022-11-28`. (`/releases/latest` skips pre-releases, so it is not used.)
- Core: thread `kuality-update` waits on a channel (`SetEnabled`, `Check`, `Install`, `Shutdown`)
  with a timeout up to the next due check; reports go to the main thread (`Updates::poll` on every
  wake), which updates the tray. The thread is never joined (a download may be running at quit).
- Installer (`installer/kuality.iss`): `IsUpdate()` = `/update=1`; the `--startup` entries are skipped
  in update mode and an extra `[Run]` entry starts `kuality.exe`. `CloseKuality` also waits up to 3 s
  for the settings window.
- Debug builds: `KUALITY_UPDATE_AS_VERSION=<version>` pretends another version
  (`cargo run -p kuality-update --example check -- 1.0.0-alpha --download` checks and downloads
  without installing).

## Limitations

- Installing from a settings window that is not elevated while the core runs elevated cannot close
  the core (UIPI); install from the tray instead.
- Not code-signed yet: the installer is verified against the release's `SHA256SUMS.txt` (same
  origin), not a signature.
