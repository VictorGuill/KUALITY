---
name: feature
description: Take a Kuality feature or fix from the user's idea to merged code ready for a beta - spec, branch, implementation, checks in the real app, PR, merge. Use when the user describes something new to build or change in Kuality ("quiero que…", "add…", "make the widget…").
---

# Feature

The workflow is defined in `AGENTS.md` → "Feature workflow"; read `AGENTS.md` fully first (hard
rules: lightweight, opt-in, no injection, hook callbacks return at once). This is the checklist.

## 1. Define

- Read the related spec in `docs/specs/`, `docs/DECISIONS.md` and the code it touches.
- Write the behavior as a spec (new file or a new section of the existing spec): what the user sees,
  config keys and defaults (disabled by default), edge cases (Explorer restart, sleep, monitors,
  elevated apps), what is out of scope.
- Ask only what changes the design; propose a default for the rest. Get the user's OK on the spec
  unless they already approved the plan.
- Add the task to `docs/ROADMAP.md` (a phase, or a dated follow-up under the phase it extends).

## 2. Build

- Branch from an up-to-date `main`: `feat/<short-name>` (`fix/<short-name>` for bugs).
- Small focused commits in English. OS-specific code stays under a `platform` module behind a trait.
- New decisions → one line each in `docs/DECISIONS.md`. User-visible changes → `CHANGELOG.md` under
  `## [Unreleased]` (create the heading if missing), written for users.
- `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test` before each commit.

## 3. Check it for real

- Unit tests for the logic; then run it (`run` skill / commands in `AGENTS.md`). For the settings
  window and popups, use the `verify-ui` skill (screenshots without stealing the user's focus).
- A dev core cannot run while the installed Kuality runs (one core per session). Ask before closing
  the user's installed copy; prefer checks that do not need the core.
- Measure the idle core (RAM, CPU) when the change touches the core.

## 4. Merge

- Push the branch, `gh pr create` (summary + how it was tested), wait for green CI
  (`gh pr checks --watch`), merge (`gh pr merge --rebase --delete-branch`: keeps the focused
  commits), update local `main`.
- Report to the user: what changed, what to test in the beta, anything not verified.
- **Do not publish.** A beta or stable release needs the user's explicit order → `release` skill.
