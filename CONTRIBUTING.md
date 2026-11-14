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

Every tag `v<version>` is a release, built and published by GitHub Actions
([.github/workflows/release.yml](.github/workflows/release.yml)):

1. Bump `version` in the root `Cargo.toml` (`[workspace.package]`) and run `cargo build` so
   `Cargo.lock` follows.
2. In [CHANGELOG.md](CHANGELOG.md), rename `## [Unreleased]` to `## [<version>] - <date>` (it becomes
   the release notes; the workflow fails without it) and end the section with the `### Upgrading`
   and `### Downloads` blocks of the release before, with the new version in the file names.
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
published as a **pre-release**; copies that are themselves pre-releases are offered it, copies of a
final version are not.
