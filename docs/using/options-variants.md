---
title: Mod options & variants
---

# Mod options & variants

Some mods offer a choice at deploy time — a color variant, an optional add-on, a quality preset. Whether (and how)
a mod exposes that choice depends on which [manifest](../authors/manifest.md) format it uses.

## Legacy manifests: a single dropdown

A mod with an old-style (legacy, no `Version` field) manifest that declares `Options` shows a plain dropdown next
to it in the mod list, listing each option by name. Only one can be selected at a time; deploying installs the
files from that option's own subfolder.

## V1 / V2 manifests: toggle + sub-options

A mod with a `V1` or `V2` manifest that declares structured `Options` shows an edit (pencil) button instead. Each
option:

- can be **toggled on or off independently** — multiple options can be active at once, each contributing its own
  files;
- can offer **sub-options** — a further single-choice list within that option (for example, an option "HUD Color"
  might have sub-options "Red", "Green", "Blue").

Click the pencil button to open the options editor and set these per mod, per profile. The edit button only
appears when the mod actually declares `Options`; otherwise it's hidden.

### V2 categories

A `V2` mod can additionally declare `Categories`. If it does, the options editor groups its options under a
heading per category (in the order the manifest declares them), with any option that has no `CategoryRef` (or one
that doesn't match a declared category) falling into a trailing, unheaded group. A `V2` mod with no `Categories`
declared — or a `V1` mod, which has no concept of categories at all — shows the same flat list either way.
Grouping only changes how options are laid out in the editor; deploy behaves identically either way (see
[Manifest reference](../authors/manifest.md)).

## Mods without declared options

A mod with no `Options` at all has nothing to choose — its files deploy as-is, no dropdown or edit button shown.
This includes any auto-generated manifest (a mod added without its own `manifest.json`) whose patch files sit
directly at the mod's root.

## Variant folders without a manifest

An auto-generated manifest *can* end up with `Options` too: if a manifest-less mod's patch files aren't at its
root, DDMM looks for directories that directly contain them (a single wrapper folder, or several variant folders
like `Red/`/`Blue/`) and turns them into a Legacy-style dropdown automatically — the first one (after natural
sorting) is selected by default. See
[Archives without a manifest.json](adding-mods.md#archives-without-a-manifestjson) for exactly how that detection
works.
