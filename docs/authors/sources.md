---
title: The Sources field
---

# The Sources field

`Sources` is an optional array on [`V1`](manifest.md#v1) and [`V2`](manifest.md#v2) manifests that declares,
without privileging any particular mod site, everywhere a mod can be found. Each entry becomes an "Open on
&lt;site&gt;" menu item for that mod in DDMM.

## The `Source` object

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `Provider` | string | Yes | Free-form, matched case-insensitively. Well-known values below get a display name and an auto-generated page URL; any other string is still accepted and shown as-is. |
| `Id` | string | No | Provider-specific mod ID, used to build that provider's normal page URL when `Url` isn't given |
| `Url` | string | No | An explicit page URL. Always wins over an auto-generated one, for every provider, and must be `http://` or `https://` — anything else (`javascript:`, `file:`, ...) is silently dropped as unsafe |
| `Version` | string | No | The version of the mod available at this source, if you want to record it |

A `Source` needs at least an `Id` *or* a `Url` to produce a usable page link; one with neither still round-trips
fine, it just won't show a link.

## Well-known providers

These get a display name and, given just an `Id`, an automatically generated page URL:

| `Provider` | Display name | Generated URL (given `Id`) |
| --- | --- | --- |
| `nexus` | Nexus Mods | `https://www.nexusmods.com/helldivers2/mods/<Id>` |
| `modworkshop` | ModWorkshop | `https://modworkshop.net/mod/<Id>` |
| `github` | GitHub | `https://github.com/<Id>` |
| `gamebanana` | GameBanana | `https://gamebanana.com/mods/<Id>` |
| `url` | Link | (no template — give an explicit `Url`) |

Any other `Provider` string (including `ayakamods`) is shown using that exact string as its display name, and
needs an explicit `Url` to produce a link, unless your DDMM version has since added a built-in template for it —
check the [manifest.rs source](https://github.com/katsyk/DemocracyDefenderModManager/blob/main/src-tauri/src/sources.rs)
for the current list.

## Examples, per site

```json
{
  "Sources": [
    { "Provider": "ayakamods", "Url": "https://ayakamods.com/mods/example-mod" },
    { "Provider": "nexus", "Id": "123", "Version": "1.2.0" },
    { "Provider": "modworkshop", "Id": "456" },
    { "Provider": "gamebanana", "Id": "789" },
    { "Provider": "github", "Id": "someone/example-mod" },
    { "Provider": "coolmodsite", "Url": "https://coolmodsite.example/mods/example" }
  ]
}
```

A mod can declare more than one source — for example, mirrored on both Nexus Mods and GitHub — and DDMM shows an
"Open on &lt;site&gt;" entry for each one that resolves to a link.

## Legacy `NexusData`

Manifests may still carry the older `NexusData` field (`{ "ModId": ..., "Version": "..." }` on V1;
`{ "ModId": ... }` on V2 — see [Manifest reference](manifest.md)) instead of, or alongside, `Sources`. DDMM treats
it as an additional implicit `nexus` source at runtime. If a manifest declares *both* `NexusData` and a `nexus`
entry in `Sources` with the same `Id`, DDMM shows only one "Open on Nexus Mods" entry rather than two — but
`NexusData` itself is never rewritten or removed from the file. There's no need to migrate an existing
`NexusData`-only manifest; adding `Sources` is purely additive.

## The `.hd2mm-origin.json` sidecar

When you (the user, not the mod author) install a mod from a direct URL through
[Add URL](../using/adding-mods.md#add-url), DDMM records that as an install-time source in a small
`.hd2mm-origin.json` file written next to the mod's `manifest.json` — **never** into the manifest itself:

```json
{
  "Sources": [
    { "Provider": "url", "Url": "https://example.com/downloads/example-mod.zip" }
  ],
  "InstalledAt": 1745020800
}
```

This is DDMM's own bookkeeping, not something mod authors write. It merges into the same "Open on &lt;site&gt;"
menu as manifest-declared sources, keeps the historical `hd2mm` filename prefix for compatibility with tooling
written against the original manager, and is safe to delete — it has no effect on how the mod itself works.
