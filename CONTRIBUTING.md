# Contributing to Kuality

Thanks for your interest! Bug reports, ideas and pull requests are welcome.

## Before you start

- Read [docs/VISION.md](docs/VISION.md) (what Kuality is and is not) and skim
  [docs/DECISIONS.md](docs/DECISIONS.md): changes that contradict a decision should be discussed in an
  issue first.
- Work happens phase by phase, see [docs/ROADMAP.md](docs/ROADMAP.md).
- AI coding agents: start with [AGENTS.md](AGENTS.md).

## Rules of the project

- **English** everywhere: code, comments, UI text, docs and commit messages.
- **Lightweight first**: the core must stay a few MB of RAM and ~0% CPU when idle. No webviews or
  heavy runtimes in the core; new dependencies need a good reason.
- **Opt-in**: every feature can be turned off and is disabled by default.
- **Windows 11 only** for now, but OS-specific code stays behind traits/modules.
- **No code injection** into `explorer.exe` or any other process.
- Low-level keyboard hook callbacks return immediately; real work goes to another thread.

## Development

Requirements: Windows 11, [Rust](https://rustup.rs) (stable, MSVC toolchain), Visual Studio Build
Tools with the C++ workload.

```powershell
cargo run -p kuality                        # core with a console showing the log
cargo run -p kuality -- --settings          # settings window
cargo fmt --all                             # format
cargo clippy --all-targets -- -D warnings   # lint, must be clean
cargo test                                  # unit tests
```

> [!IMPORTANT]
> **Quit an installed Kuality before running a dev build** (tray → Quit, or `taskkill /IM kuality.exe`
> without `/F`). Only one core runs per session, so a dev build would exit at once, and two keyboard
> hooks would fight over CapsLock anyway.

For a throwaway setup, create an empty file `target\debug\portable`: config and logs then live in
`target\debug\` instead of `%APPDATA%\Kuality\`.

Before opening a pull request, `cargo fmt`, `cargo clippy` and `cargo test` must pass (CI checks them
on every push and pull request).

## Releases (maintainers)

There is one long-lived branch, `main`. Features and fixes are made on short branches
(`feat/<name>`, `fix/<name>`) and merged into `main` once CI is green. Releases are tags on `main`:

- **Beta** — `v1.1.0-beta.1`, `v1.1.0-beta.2`… published as GitHub **pre-releases**. Only copies
  set to the **beta** channel (Settings → General → Updates → Channel) are offered them.
- **Stable** — `v1.1.0`, published as the latest release and offered to everyone. A stable release
  is normally a beta that was tested and approved: the same commit, with the final version number
  and the changelog of all its betas in one section. A **patch** (fixes and small improvements, no
  new feature) may skip the beta and be tagged `v1.1.1` on `main` directly.

If `main` already has work that has not been tested in a beta, make the stable release on a short
branch from the approved beta tag (`git switch -c release/1.1 v1.1.0-beta.3`), tag it there, and
merge that branch back into `main`. Fixes for a stable release are made the same way
(`1.1.1` on `release/1.1`).

Version numbers follow [SemVer](https://semver.org): a new feature raises the minor (`1.1.0`), a fix
alone the patch (`1.0.1`). Betas count up from `.1` for each target version (`1.1.0-beta.1`, then
`1.1.0-beta.2` after fixes).

Every tag `v<version>` is a release, built and published by GitHub Actions
([.github/workflows/release.yml](.github/workflows/release.yml)):

1. Bump `version` in the root `Cargo.toml` (`[workspace.package]`) and run `cargo build` so
   `Cargo.lock` follows.
2. In [CHANGELOG.md](CHANGELOG.md), rename `## [Unreleased]` to `## [<version>] - <date>` (it becomes
   the release notes; the workflow fails without it) and end the section with the `### Upgrading`
   and `### Downloads` blocks of the release before, with the new version in the file names. For a
   stable release made from betas, merge the sections of its betas into one (what changed since the
   last stable release) and keep the beta sections below it.
3. If the release ships a phase, add "Released in `v<version>`" to it in
   [docs/ROADMAP.md](docs/ROADMAP.md).
4. Commit (`Release <version>`) and push `main`. Wait for its CI run to finish green:

   ```powershell
   git push origin main
   gh run watch (gh run list --branch main --limit 1 --json databaseId --jq '.[0].databaseId') --exit-status
   ```

5. Tag that commit and push the tag: this starts the release workflow.

   ```powershell
   git tag v<version>
   git push origin v<version>
   ```

6. The GitHub Release appears only when the workflow ends (about 5–10 minutes: its last step
   publishes it). Check it has the three files:

   ```powershell
   gh release view v<version>
   ```

Tag only a commit of `main` whose CI run is green (so do not push the tag together with `main`): the
release workflow does not run clippy or the tests again. It checks that the tag matches `Cargo.toml`,
builds the installer and the portable zip, and publishes them with `SHA256SUMS.txt`. These file names
are what the updater of installed copies looks for. A version with a suffix (`1.1.0-beta.2`) is
published as a **pre-release** and offered only on the beta channel; a final version is offered on
both channels.
