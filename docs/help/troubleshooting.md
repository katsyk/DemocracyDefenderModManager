---
title: Troubleshooting
---

# Troubleshooting

## DDMM won't start (Linux)

DDMM is a [Tauri](https://tauri.app/) app and needs WebKitGTK installed to run. If nothing happens when you run
`./ddmm`, or it exits immediately, install your distribution's WebKitGTK runtime package. On Debian/Ubuntu-based
systems that's `libwebkit2gtk-4.1-0` (the project's own CI installs `libwebkit2gtk-4.1-dev` plus
`build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev` to *build* DDMM
on Ubuntu — see [Building from source](../development/building.md) — but just running a pre-built binary only
needs the WebKitGTK runtime library itself).

## Windows SmartScreen warns about an unknown publisher

Expected for these builds — they aren't code-signed. Click **"More info"**, then **"Run anyway"**. This applies to
both the installer and the app itself the first time you run it.

## DDMM opens straight to Settings

On startup DDMM checks your [Game Path](../using/settings.md#game-path); if it's invalid or empty (including on a
completely fresh install), it first tries to auto-detect your Helldivers 2 install via Steam and, if that
succeeds, fills in and saves the path for you automatically — you'll see a "Found Helldivers 2 at ..." message
and DDMM continues straight to Mods, no trip to Settings needed.

You only land on Settings when auto-detection also fails (Steam isn't installed, or the game isn't). Set the path
yourself, or click **Auto-detect** again after installing/moving the game — see
[First-time setup](../getting-started/setup.md).

## "Loading failed!" on the Mods page

DDMM loads every mod's `manifest.json` when it starts. If exactly one of them is malformed (invalid JSON, or
missing a required field), loading the whole list fails with this error and its underlying message.

To fix it:

1. Open the `mods/` folder — click **Open Folder** next to **Data Folder** in Settings to find it, then go into
   `mods/` from there (see [Data location](../getting-started/download.md#data-location) if you're not sure
   which folder that is).
2. Move mod subfolders out one at a time (or check the error text — it often reports which file the failure came
   from) until DDMM loads successfully again.
3. Fix or re-download the offending mod, then move it back in — or leave it out and re-add it through DDMM
   normally.

## A mod I added doesn't show up

- A mod folder under `mods/` with **no** `manifest.json` in it at all is silently skipped when DDMM loads the mod
  list (rather than erroring) — this can happen if an install was interrupted. Delete the incomplete folder and
  re-add the mod.
- Check the [log file](logs.md) — a failed add shows a popup with the specific error, and the same detail is in
  the log.

## Deploy or Purge fails immediately

Both refuse to run against an invalid [Game Path](../using/settings.md#game-path) — double-check Settings first.
If the path is valid but deploy still fails partway through, check the log for which file operation failed (for
example, a patch file the mod claims to include that doesn't actually exist in its folder).

## A specific mod won't deploy, or deploying it errors out

`V1` and `V2` manifests deploy the same way. If an enabled option (or the selected sub-option) doesn't actually
contribute any files, check the [log file](logs.md) for a warning naming the mod and an out-of-range option/
sub-option index, or an `Include` folder that doesn't exist in the mod's own directory — deploy skips that
option rather than failing the whole deploy, but it also means nothing gets installed for it.

## Browser handoff fails immediately with a Downloads-folder error

"the downloads folder isn't set or doesn't exist -- check it in Settings" means your
[Downloads Folder](../using/settings.md#downloads-folder) setting is empty or points at a folder that no longer
exists — fix it in Settings, then retry.

## "a handoff is already in progress"

Only one [browser handoff](../using/mod-sites.md#how-the-browser-handoff-works) can run at a time. Cancel or wait
for the current one (Waiting/Installing popup) to finish before starting another.

## Still stuck?

See [Reporting bugs](bugs.md).
