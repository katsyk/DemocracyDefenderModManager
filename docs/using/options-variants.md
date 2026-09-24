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

!!! warning "V2 deploy support"
    DDMM can read `V2` manifests, show their options, and let you configure them — but deploying a `V2`-manifest
    mod is **not implemented yet** in this codebase (it's a `todo!()` in the deploy command, which currently
    causes deploying that mod to fail/panic rather than install its files). Use `V1` for a mod you need to
    actually deploy today; see [Manifest reference](../authors/manifest.md) for the version differences.

## Mods without declared options

A mod with no `Options` at all (including every auto-generated manifest for a mod added without its own
`manifest.json`) has nothing to choose — its files deploy as-is, no dropdown or edit button shown.

!!! info "Variant folders without a manifest"
    Detecting multiple top-level folders in a manifestless archive (e.g. `Red/`, `Blue/`) and turning them into
    selectable options automatically is a landing feature — see the note in
    [Adding mods](adding-mods.md#archives-without-a-manifestjson).
