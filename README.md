# Democracy Defender Mod Manager

*Your mods. Your democracy. Defended.*

**Works with:** AyakaMods · Nexus Mods · ModWorkshop · GameBanana · GitHub · any direct link

**Docs: https://katsyk.github.io/DemocracyDefenderModManager/**

DDMM is a source-neutral mod manager for Helldivers 2: install mods from any site, a direct download link, an
archive, or a plain folder — no site is privileged, and no account with any mod site is ever required.

Made with love for the AyakaMods community ❤️ — one of the best homes for Helldivers 2 modding, and proudly
DDMM's first-class citizen. [Say hi to AyakaMods](https://ayakamods.com/games/helldivers-2.119/).

## The DDMM Promise

*Every Helldiver takes an oath. This one's ours.*

- **Every mod is welcome.** DDMM will never block, blacklist, or rank mods or mod sites.
- **No account, anywhere.** No mod site's login is ever asked for. (A Nexus Mods API key for Nexus update checks is strictly optional.)
- **Your files, your choice.** Your mods live in a folder you control.
- **Open source, forever.** Apache-2.0, today and always.
- **Updates on your terms.** Update checks never run unless you ask: click **Check for Updates**, or turn on automatic checks in Settings.

## Source-neutral by design

Mods from any site — AyakaMods, Nexus Mods, ModWorkshop, GameBanana, GitHub, or a direct download link — or from a
local archive or a plain folder, are all first-class.

Concretely, that means:

- **Any archive** (`.zip`, `.7z`, `.rar`) can be added regardless of where it came from.
- **Plain folders** can be added directly, with no archive step, via "Add Folder" or by dragging a folder in.
- **Direct download links** can be added via "Add URL" — the manager downloads the archive itself.
- **Login-gated sites** (AyakaMods, Nexus Mods) never have their credentials asked for or stored. Pasting a mod
  page link from one of these instead opens it in your browser and watches your Downloads folder for the finished
  file, installing it automatically once it lands — the same "browser handoff" also kicks in for any other site
  whose link doesn't serve a downloadable archive directly.
- **Where a mod came from** is tracked without ever assuming or requiring a particular site.
- **Archive extraction is hardened** against path traversal and symlink entries: every entry is validated before
  extraction, and the result is double-checked afterward, regardless of which site an archive came from.

### The `Sources` manifest field

A mod's `manifest.json` may declare an optional, provider-neutral `Sources` array. Each entry names a `Provider`
(well-known values are `ayakamods`, `nexus`, `modworkshop`, `github`, and `gamebanana`, but any other site name is
accepted) and either an `Id` (used to build that provider's normal mod-page URL) or an explicit `Url` (which
always takes precedence over the generated one):

```json
{
  "Version": 2,
  "Guid": "...",
  "Name": "Example Mod",
  "Description": "...",
  "Sources": [
    { "Provider": "ayakamods", "Id": "4084", "Version": "2026-09-24" },
    { "Provider": "nexus", "Id": "123" },
    { "Provider": "github", "Id": "someone/example-mod" },
    { "Provider": "coolmodsite", "Url": "https://coolmodsite.example/mods/example" }
  ]
}
```

These show up as "Open on <site>" entries in a mod's menu in the manager.

The legacy `NexusData` field (`{ "ModId": ..., "Version": "..." }`) is still read for backwards compatibility and is
automatically treated as an additional nexus source — old manifests keep working unchanged, and existing `NexusData`
is never removed or rewritten.

### The `.hd2mm-origin.json` sidecar

When a mod is installed from a direct URL, or through a browser handoff, the manager records that install origin
in a small `.hd2mm-origin.json` file next to the mod's own `manifest.json` — it never edits or overwrites the mod
author's manifest. The filename keeps its original `hd2mm` prefix for compatibility with manifests and tooling
written against it; it isn't user-visible and renaming it would just be churn. It looks like:

```json
{
  "Sources": [
    { "Provider": "url", "Url": "https://example.com/downloads/example-mod.zip" }
  ],
  "InstalledAt": 1745020800
}
```

This is purely local bookkeeping so the manager can show where a mod was fetched from; it's safe to delete and has
no effect on how the mod itself works.

## Installing / building

Prerequisites: [pnpm](https://pnpm.io/), a stable [Rust toolchain](https://rustup.rs/), and (Linux only) the Tauri
system dependencies:

```sh
sudo apt install -y libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

Then:

```sh
pnpm install
pnpm tauri build
```

For local development, `pnpm tauri dev` runs the app with hot reload. The built binary is named `ddmm` (`ddmm.exe`
on Windows).

## Credits & license

DDMM is derived from Helldivers 2 Mod Manager by teutinsa
(https://github.com/teutinsa/Helldivers2ModManager), licensed under Apache-2.0.

Original project: https://teutinsa.github.io/hd2mm-site · support the original author: https://ko-fi.com/teutinsa
