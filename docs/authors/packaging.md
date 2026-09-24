---
title: Packaging your mod
---

# Packaging your mod

## Patch files

Helldivers 2 mods are made of **patch files**: DDMM looks for files named

```text
<16 lowercase hex characters>.patch_<N>
```

optionally paired with a `.gpu_resources` and/or `.stream` file sharing the exact same name (for example
`0cf14e223de06a26.patch_0`, `0cf14e223de06a26.patch_0.gpu_resources`,
`0cf14e223de06a26.patch_0.stream`). Anything else in a mod's folder is ignored when DDMM collects files to deploy.
Patch files are looked for directly inside whichever folder is in play (see below) — not in nested subfolders.

## Folder layout, by manifest type

Where DDMM looks for your patch files depends on whether — and how — your `manifest.json` declares `Options`. See
[Manifest reference](manifest.md) for the full field list of each version.

### No `Options` (or no manifest at all)

Put your patch files directly in the mod's root folder (the folder `manifest.json` sits in, or the archive/folder
root if you're not shipping a manifest). DDMM deploys them as-is.

```text
my-mod/
├── manifest.json          (optional)
├── 0cf14e223de06a26.patch_0
├── 0cf14e223de06a26.patch_0.gpu_resources
└── 0cf14e223de06a26.patch_0.stream
```

!!! tip "Shipping without a manifest.json"
    If you don't ship a `manifest.json` at all, DDMM still finds your patch files even when they're one level
    down in a single wrapper folder, or split across several top-level variant folders (like the Legacy example
    below) — it auto-detects the layout and builds the equivalent `Options` dropdown itself. Root-level files are
    still simplest and require no detection at all. See
    [Archives without a manifest.json](../using/adding-mods.md#archives-without-a-manifestjson) for exactly how
    that works, including its 4-level depth limit and what happens if no patch files are found anywhere.

### Legacy manifest with `Options`

Each entry in `Options` is a plain string naming a subfolder under the mod root; the user picks exactly one from a
dropdown, and DDMM deploys the patch files inside that one subfolder.

```text
my-mod/
├── manifest.json           (no "Version" field — Options: ["Red", "Blue"])
├── Red/
│   └── 0cf14e223de06a26.patch_0
└── Blue/
    └── 0cf14e223de06a26.patch_0
```

### V1 manifest with `Options`

Each `Option` can be toggled on/off independently, and points at one or more folders via its `Include` array
(paths relative to the mod root). An option can also declare `SubOptions`, each with its own `Include`, for a
further single choice within that option.

```text
my-mod/
├── manifest.json
│   (V1, Options: [{ Name: "HUD Color", Include: ["hud"],
│                     SubOptions: [{ Name: "Red", Include: ["hud/red"] },
│                                   { Name: "Green", Include: ["hud/green"] }] }])
├── hud/
│   ├── 0cf14e223de06a26.patch_0        (deployed whenever "HUD Color" is toggled on)
│   ├── red/
│   │   └── 1a2b3c4d5e6f7081.patch_0    (deployed if "Red" is selected)
│   └── green/
│       └── 1a2b3c4d5e6f7081.patch_0    (deployed if "Green" is selected)
```

!!! note "V2 manifests"
    `V2` uses the same `Options`/`SubOptions`/`Include` shape and deploys identically to `V1` — it just adds
    `Categories` and `Tags` for organizing options in the editor and on the mod's card. See
    [Manifest reference](manifest.md#v2).

## Icons and option images

`IconPath` (on the manifest) and `Image` (on an option or sub-option) are paths relative to the mod root, pointing
at an image file to show in the UI. DDMM matches these case-insensitively against what's actually on disk, so
`IconPath: "Icon.png"` will still find `icon.png` — useful since archives are frequently built on a
case-insensitive filesystem (Windows) but DDMM may run on a case-sensitive one (Linux).

## The mod ID (`Guid`)

Every manifest needs a `Guid` — a standard UUID uniquely identifying your mod (not a specific version of it; keep
it stable across updates). Generate one once with any UUID v4 generator and keep using it.

## Validating your manifest

See [Manifest reference](manifest.md) for the field-by-field spec and JSON Schemas you can validate against before
publishing.
