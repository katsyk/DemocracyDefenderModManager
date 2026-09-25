---
title: Project structure
---

# Project structure

```text
Helldivers2ModManager/       (repository root — the folder name is historical)
├── src/                     SvelteKit frontend
│   ├── routes/               pages: Mods (+page.svelte), Settings, Create, Help
│   └── lib/
│       ├── components/       reusable Svelte components (popups, toggle switch, ...)
│       ├── models/            frontend-side TS models mirroring the Rust ones (Mod, manifest, profile, settings)
│       ├── state/             Svelte state/stores (localization, popup manager)
│       ├── services/          supporting services (e.g. localization loading)
│       ├── types/             shared TS types (popups, command results, UUID)
│       ├── utils/             helpers, including the typed Tauri command wrappers (commands.ts)
│       └── locales/           UI strings (en.json)
├── src-tauri/                Rust/Tauri backend
│   ├── src/
│   │   ├── commands/           #[tauri::command] entry points: mods.rs, profiles.rs, settings.rs, updates.rs, nexus.rs, plus deploy/purge in mod.rs
│   │   ├── models/              manifest.rs, profile.rs, settings.rs, and the shared Mod type
│   │   ├── archive/             zip/7z/rar handling and extraction hardening
│   │   ├── sources.rs            provider-neutral Source resolution and the .hd2mm-origin.json sidecar
│   │   ├── download.rs           direct-URL archive downloading
│   │   ├── providers/            per-site update checks (AyakaMods, GitHub, GameBanana, ModWorkshop, Nexus Mods)
│   │   ├── secrets.rs            the optional Nexus API key: OS keychain / Linux 0600-file fallback, redaction
│   │   ├── utils.rs              small filesystem helpers (recursive copy, case-insensitive path matching)
│   │   └── lib.rs                Tauri app setup: plugins, app state, command registration
│   └── Cargo.toml
├── docs/                     this documentation site (MkDocs + Material)
├── mkdocs.yml
└── .github/workflows/         build.yml (release builds), docs.yml (this site)
```

## How a command call flows

The frontend calls into Rust exclusively through `src/lib/utils/commands.ts`, which wraps Tauri's
`invoke()` around each `#[tauri::command]` in `src-tauri/src/commands/`. State on the Rust side (the loaded mod
list) is held in `AppState`, behind a `tokio::Mutex`, for the lifetime of the app — see `src-tauri/src/lib.rs`.

## Models mirror both sides

Data types that cross the Tauri boundary — the manifest formats, `Profile`/`Config`, `Settings` — are defined once
in Rust (`src-tauri/src/models/`) with `serde` `PascalCase` (de)serialization, and mirrored by hand as TypeScript
types under `src/lib/models/` and `src/lib/types/`. If you change a Rust model's shape, the matching TypeScript
type needs updating too.
