---
title: Manifest reference
---

# Manifest reference

A mod's `manifest.json` tells DDMM its identity, description, icon, and any options it offers. DDMM supports
three formats. Which one a manifest is depends only on its `Version` field: no `Version` field means **Legacy**;
`"Version": 1` means **V1**; `"Version": 2` means **V2**. Any other value is rejected as an unknown version.

All field names are `PascalCase`. This page mirrors `src-tauri/src/models/manifest.rs` exactly — if in doubt,
that file is authoritative.

## Legacy

No `Version` field at all. The original, simplest format.

```json
{
  "Guid": "b7f2c1a0-6e3d-4b8a-9f1e-2d3c4b5a6e7f",
  "Name": "Example Mod",
  "Description": "A short description.",
  "IconPath": "icon.png",
  "Options": ["Red", "Blue"]
}
```

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `Guid` | UUID string | Yes | The mod's stable identifier |
| `Name` | string | Yes | |
| `Description` | string | Yes | |
| `IconPath` | string | No | Relative path to an icon image |
| `Options` | array of strings | No | Each string is a subfolder name; the user picks exactly one — see [Packaging your mod](packaging.md#legacy-manifest-with-options) |

Legacy manifests can't declare `Sources` or `NexusData` — DDMM has no way to associate a source with a Legacy
manifest beyond whatever it records itself when you install from a URL (the `.hd2mm-origin.json` sidecar; see
[The Sources field](sources.md#the-hd2mm-originjson-sidecar)). Auto-generated manifests (for a mod added without
its own `manifest.json`) are always Legacy, with no `Options`.

## V1

`"Version": 1`. Adds structured, toggleable options with sub-choices, and the provider-neutral `Sources` field.

```json
{
  "Version": 1,
  "Guid": "b7f2c1a0-6e3d-4b8a-9f1e-2d3c4b5a6e7f",
  "Name": "Example Mod",
  "Description": "A short description.",
  "IconPath": "icon.png",
  "Options": [
    {
      "Name": "HUD Color",
      "Description": "Recolors the HUD.",
      "Include": ["hud"],
      "SubOptions": [
        { "Name": "Red", "Description": "Red HUD.", "Include": ["hud/red"] },
        { "Name": "Green", "Description": "Green HUD.", "Include": ["hud/green"] }
      ]
    }
  ],
  "Sources": [
    { "Provider": "nexus", "Id": "123", "Version": "1.2.0" }
  ]
}
```

`Manifest`:

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `Version` | integer | Yes | Must be `1` |
| `Guid` | UUID string | Yes | |
| `Name` | string | Yes | |
| `Description` | string | Yes | |
| `IconPath` | string | No | |
| `Options` | array of [`Option`](#v1-option) | No | |
| `NexusData` | [`NexusData`](#v1-nexusdata) | No | Legacy Nexus identification; still read, merged into `Sources` at runtime |
| `Sources` | array of [`Source`](sources.md#the-source-object) | No | See [The Sources field](sources.md) |

#### V1 `Option`

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `Name` | string | Yes | |
| `Description` | string | Yes | |
| `Include` | array of strings (paths) | No | Folders (relative to mod root) deployed when this option is toggled on |
| `Image` | string | No | Relative path to a preview image |
| `SubOptions` | array of [`SubOption`](#v1-suboption) | No | A further single choice within this option |

#### V1 `SubOption`

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `Name` | string | Yes | |
| `Description` | string | Yes | |
| `Include` | array of strings (paths) | **Yes** | Unlike `Option.Include`, this is required |
| `Image` | string | No | |

#### V1 `NexusData`

| Field | Type | Required |
| --- | --- | --- |
| `ModId` | integer | Yes |
| `Version` | string | Yes |

### JSON Schema

[`docs/schemas/manifest-v1.schema.json`](../schemas/manifest-v1.schema.json) — validate your manifest against it
with any JSON Schema (2020-12) validator.

## V2

`"Version": 2`. Adds `Categories` (to group options) and `Tags` (shown on the mod's card).

!!! note "V2 deploy semantics"
    `V2` deploys the same way `V1` does: each toggled-on option contributes its own `Include` directories, plus
    the selected sub-option's `Include` directories for any option that declares `SubOptions`. `CategoryRef` and
    `Categories` only affect how options are grouped in the options editor (see
    [Mod options & variants](../using/options-variants.md#v2-categories)) — they have no effect on what gets
    deployed.

```json
{
  "Version": 2,
  "Guid": "b7f2c1a0-6e3d-4b8a-9f1e-2d3c4b5a6e7f",
  "Name": "Example Mod",
  "Description": "A short description.",
  "IconPath": "icon.png",
  "Tags": ["cosmetic", "hud"],
  "Categories": [
    { "Guid": "b1e3c5d7-1111-2222-3333-444455556666", "Name": "Visuals", "Description": "Visual changes." }
  ],
  "Options": [
    {
      "Guid": "c2f4d6e8-7777-8888-9999-aaaabbbbcccc",
      "Name": "HUD Color",
      "CategoryRef": "b1e3c5d7-1111-2222-3333-444455556666",
      "Description": "Recolors the HUD.",
      "Include": ["hud"]
    }
  ],
  "Sources": [
    { "Provider": "ayakamods", "Id": "4084", "Version": "2026-09-24" },
    { "Provider": "nexus", "Id": "123" },
    { "Provider": "github", "Id": "someone/example-mod" }
  ]
}
```

`Manifest`:

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `Version` | integer | Yes | Must be `2` |
| `Guid` | UUID string | Yes | |
| `Name` | string | Yes | |
| `Description` | string | Yes | |
| `IconPath` | string | No | |
| `Options` | array of [`Option`](#v2-option) | No | |
| `Categories` | array of [`Category`](#v2-category) | No | |
| `Tags` | array of strings | No | |
| `NexusData` | [`NexusData`](#v2-nexusdata) | No | Note: no `Version` field, unlike V1's `NexusData` |
| `Sources` | array of [`Source`](sources.md#the-source-object) | No | |

#### V2 `Option`

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `Guid` | UUID string | Yes | Unlike V1, each V2 option has its own id |
| `Name` | string | Yes | |
| `CategoryRef` | UUID string | No | `Guid` of an entry in `Categories` |
| `Description` | string | Yes | |
| `Include` | array of strings (paths) | No | |
| `Image` | string | No | |
| `SubOptions` | array of [`SubOption`](#v2-suboption) | No | |

#### V2 `SubOption`

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `Guid` | UUID string | Yes | |
| `Name` | string | Yes | |
| `Description` | string | Yes | |
| `Include` | array of strings (paths) | **Yes** | |
| `Image` | string | No | |

#### V2 `Category`

| Field | Type | Required |
| --- | --- | --- |
| `Guid` | UUID string | Yes |
| `Name` | string | Yes |
| `Description` | string | Yes |

#### V2 `NexusData`

| Field | Type | Required |
| --- | --- | --- |
| `ModId` | integer | Yes |

### JSON Schema

[`docs/schemas/manifest-v2.schema.json`](../schemas/manifest-v2.schema.json).

## Notes for all formats

- Field names are case-sensitive `PascalCase` (`Guid`, not `guid` or `GUID`).
- Paths (`IconPath`, `Image`, `Include` entries) are relative to the mod's root folder and use forward slashes;
  DDMM matches them against what's on disk case-insensitively.
- Unknown/extra fields in a manifest are ignored, not rejected — DDMM only reads the fields it knows about.
- See [The Sources field](sources.md) for `Sources` in depth, including how it interacts with legacy `NexusData`
  and DDMM's own install-time bookkeeping.
