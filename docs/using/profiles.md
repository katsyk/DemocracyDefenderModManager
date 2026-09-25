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

Order matters: it's the load order. See the next section.

## Load order and conflicts

**Mods lower in the list win.** When two mods change the same game file, the one further down the list is the one
you see in game. The top of the list is the lowest priority, and the bottom is the highest.

The Mods page says this above the list ("Load order: when two mods change the same game file, the one lower in the
list wins.") and labels the two ends **Lowest priority (loaded first)** and **Highest priority (wins conflicts)**.
Hover the grip at the left of any mod for a reminder.

Why: Helldivers 2 loads each game file's patches in number order (`<name>.patch_0`, then `.patch_1`, and so on),
and a higher number overrides a lower one. When you [deploy](deploy-purge.md), DDMM numbers each file's patches
in your list order, top to bottom. So the first mod in the list that touches a file gets `.patch_0`, the next one
that touches the same file gets `.patch_1`, and the last one gets the highest number. A mod that ships several
patches for the same file keeps them in its own order. (If that file is in your
[Skip List](settings.md#skip-list), numbering starts at `.patch_1` instead; the order is the same.)

Mods that change *different* files don't conflict, and their order doesn't matter.

To change a mod's priority, drag it, or use **Move Up**, **Move Down**, **To Top** and **To Bottom** in its
three-dot menu, then **Deploy** again. The new order only reaches the game after a deploy.

For example, with two armor retextures that both change the same armor:

| List position | Mod | Result |
| --- | --- | --- |
| 1 (top) | Red Armor | `.patch_0`, overridden |
| 2 (bottom) | Blue Armor | `.patch_1`, **shown in game** |

To see Red Armor instead, move it below Blue Armor and deploy.

## Deleting a mod entirely

The Library panel's trash-can button permanently uninstalls a mod: after confirming ("Are you sure you want to
delete this mod?"), DDMM deletes the mod's files from `mods/` and removes it from every profile that referenced
it. This is different from a profile's **Remove**, which only takes the mod out of the current profile and leaves
it installed in your Library.

## Saving

Profiles (and their mod order, enabled state, and options) are saved automatically whenever you navigate away
from the Mods page or close DDMM — there's no separate "save" button.
