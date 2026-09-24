---
title: Contributing
---

# Contributing

DDMM doesn't have a separate `CONTRIBUTING.md` at the time of writing — this page covers the basics until it does.

## Reporting issues and proposing changes

- Bugs and feature ideas go through [GitHub Issues](https://github.com/katsyk/DemocracyDefenderModManager/issues).
- Code changes go through a pull request against the repository's default branch.

## Before opening a pull request

- Run `pnpm check` (Svelte/TypeScript type checking) and `pnpm run scan:i18n` (checks that every UI string used in
  the frontend has a matching key in `src/lib/locales/en.json`) — see
  [Building from source](building.md#other-useful-scripts).
- Run the Rust test suite from `src-tauri/`: `cargo test`. The codebase has unit tests around manifest parsing,
  source resolution, archive extraction hardening, and URL downloading — mirror that coverage for new logic in
  those areas, especially anything touching archive extraction or file paths.
- Keep new user-facing strings in `src/lib/locales/en.json` rather than hard-coded in components, consistent with
  the rest of the frontend.

## Scope

This documentation site (`docs/`, `mkdocs.yml`, the `docs.yml` workflow, and `README.md`) is maintained
separately from the application source — see the repository for who currently owns which. If you spot something
inaccurate on this site relative to the actual code, please open an issue or use the "Edit this page" link at the
top of any page.
