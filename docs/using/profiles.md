---
title: Profiles
---

# Profiles

A profile is a named, ordered list of mods with their own enabled/disabled state and options — think of it as a
loadout. You always have at least one profile (a fresh install starts you with one named "Default").

## Switching profiles

The dropdown at the top of the Mods page selects the active profile. Switching profiles swaps the whole mod list
below it for that profile's own list; nothing is deployed until you click **Deploy**.

## Creating and removing profiles

- **+** (tip: "New profile") prompts for a name (at least 3 characters) and switches to the new, empty profile.
- **-** (tip: "Remove profile") asks for confirmation ("Are you sure you want to remove this profile?") and
  deletes the current profile. This button is disabled while you only have one profile — you can't remove your
  last one.

## Building a profile's mod list

Mods you've added live in the **Library** panel (the collapsible strip on the right edge of the mod list). From
there:

- **Insert at top** / **insert at bottom** adds a library mod into the current profile's list, enabled by default.
- Mods already in the current profile don't show up in the Library at the same time — the two lists never overlap.

Within the profile's list, each mod's context menu (the three-dot button) offers **Remove** (takes it back out of
the profile and into the Library — this does not uninstall it), **Move Up**, **Move Down**, **To Top**, and
**To Bottom**. You can also drag entries to reorder them directly, and search (the search box above the list)
filters the visible list by name/description — reordering and the Library toggle are disabled while a search is
active.

Order matters for [mod options](options-variants.md) and how files overwrite each other during
[deploy](deploy-purge.md): later entries in the list are written after earlier ones.

## Deleting a mod entirely

The Library panel's trash-can button permanently uninstalls a mod: after confirming ("Are you sure you want to
delete this mod?"), DDMM deletes the mod's files from `mods/` and removes it from every profile that referenced
it. This is different from a profile's **Remove**, which only takes the mod out of the current profile and leaves
it installed in your Library.

## Saving

Profiles (and their mod order, enabled state, and options) are saved automatically whenever you navigate away
from the Mods page or close DDMM — there's no separate "save" button.
