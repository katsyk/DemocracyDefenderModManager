---
title: Building from source
---

# Building from source

## Prerequisites

- [pnpm](https://pnpm.io/)
- A stable [Rust toolchain](https://rustup.rs/)
- Node.js 24 (matches the version the project's own CI uses)
- On Linux, the Tauri system dependencies:

  ```sh
  sudo apt install -y libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
  ```

  (Debian/Ubuntu package names; adjust for your distribution.)

## Build

```sh
pnpm install
pnpm tauri build
```

This runs `pnpm build:checked` first (an i18n key check, then the SvelteKit static build via
`@sveltejs/adapter-static`), then compiles the Tauri/Rust side and links the frontend in. The resulting binary is
`src-tauri/target/release/ddmm` (`ddmm.exe` on Windows) — DDMM ships as a bare executable
(`bundle.active` is `false` in `tauri.conf.json`), not an installer or platform bundle.

## Develop

```sh
pnpm tauri dev
```

Runs the app with hot reload; the frontend dev server listens on `http://localhost:1420` (see `vite.config.js` /
`tauri.conf.json`).

## Other useful scripts

See `package.json` for the full list; the most relevant ones:

| Script | What it does |
| --- | --- |
| `pnpm check` | Type-checks the Svelte/TypeScript frontend |
| `pnpm run scan:i18n` | Checks that every UI string used in the frontend has a matching key in `src/lib/locales/en.json` |
| `cargo test` (from `src-tauri/`) | Runs the Rust unit tests (manifest parsing, source resolution, archive hardening, download handling, and more) |

## CI

The project's own release workflow (`.github/workflows/build.yml`) builds on `windows-latest` and `ubuntu-latest`
using the same `pnpm install` / `pnpm tauri build` steps described above, triggered by pushing a `v*` or
`v*_preview*` tag, and attaches the resulting `ddmm.exe` / `ddmm` to a draft pre-release.
