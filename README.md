# Helldivers2ModManager

A simple mod manager for the game Helldivers 2.

Read more about it on the [website](https://teutinsa.github.io/hd2mm-site/index.html).

## About this fork

This fork ([katsyk/Helldivers2ModManager](https://github.com/katsyk/Helldivers2ModManager)) is source-neutral by design:
mods from any site — Nexus Mods, ModWorkshop, GameBanana, GitHub, or a direct download link — or from a local
archive or a plain folder, are all first-class. No account with any mod site is required to use this manager, and
none ever will be.

Concretely, that means:

- **Any archive** (`.zip`, `.7z`, `.rar`) can be added regardless of where it came from.
- **Plain folders** can be added directly, with no archive step, via "Add Folder" or by dragging a folder in.
- **Direct download links** can be added via "Add URL" — the manager downloads the archive itself. This only works
  for links that serve the archive directly; sites that gate downloads behind a login page or JavaScript (Nexus's
  mod page, for example, as opposed to its direct file link) aren't supported this way, but the archive can still be
  downloaded manually and added like any other file.
- **Where a mod came from** is tracked without ever assuming or requiring a particular site.

### The `Sources` manifest field

A mod's `manifest.json` may declare an optional, provider-neutral `Sources` array. Each entry names a `Provider`
(well-known values are `nexus`, `modworkshop`, `github`, and `gamebanana`, but any other site name is accepted) and
either an `Id` (used to build that provider's normal mod-page URL) or an explicit `Url` (which always takes
precedence over the generated one):

```json
{
  "Version": 2,
  "Guid": "...",
  "Name": "Example Mod",
  "Description": "...",
  "Sources": [
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

When a mod is installed from a direct URL, the manager records that install origin in a small
`.hd2mm-origin.json` file next to the mod's own `manifest.json` — it never edits or overwrites the mod author's
manifest. It looks like:

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