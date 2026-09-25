---
title: Settings reference
---

# Settings reference

The Settings page has three actual settings, stored in `settings.json` in DDMM's
[data folder](../getting-started/download.md#data-location), plus a line showing where that data folder is, with
buttons to open it or [move it](data-folder.md).

## Game Path

The folder your Helldivers 2 install lives in — see [First-time setup](../getting-started/setup.md) for the
validation rules, error messages, and how automatic detection works. This is required: DDMM won't let you leave
the Settings page, and refuses to [deploy or purge](deploy-purge.md), until it's valid.

- **Label:** Game Path
- **Placeholder:** `eg. Steam/steamapps/common/Helldivers 2/`
- **Auto-detect button:** looks for Helldivers 2 via Steam (the same check DDMM runs automatically on startup
  when the path is invalid) and fills in the field if it finds a valid install
- **Browse... button:** opens a folder picker

## Downloads Folder

The folder DDMM watches during a [browser handoff](mod-sites.md#how-the-browser-handoff-works) — its own
description reads: "Where DDMM watches for a mod you download from a site it can't fetch directly (e.g. AyakaMods,
Nexus Mods)." Defaults to your OS's normal Downloads folder.

- **Label:** Downloads Folder
- **Placeholder:** `eg. Downloads`
- **Browse... button:** opens a folder picker

Unlike Game Path, this field isn't validated on the Settings page itself — an invalid or missing folder only
surfaces as an error at the moment you actually start a handoff ("the downloads folder isn't set or doesn't
exist -- check it in Settings"). Only the browser handoff uses this setting; every other install method (Add,
Add Folder, Add URL for a direct link) is unaffected by it.

It can't be DDMM's own `mods/` folder or a folder inside it (saving Settings says so, and neither the handoff nor
auto-import will watch such a folder). A downloads folder that *contains* DDMM's data folder, such as a portable
DDMM kept in Downloads, is fine: only files directly in the downloads folder are ever looked at.

## Skip List

A list of 16-character lowercase hex patch-name prefixes (for example `0cf14e223de06a26`) that already occupy
`.patch_0` in your `data` folder — typically a DLC's own patch file. When a patch name you're deploying is in this
list, DDMM starts numbering its files at `.patch_1` instead of `.patch_0`, so a mod touching the same asset
doesn't collide with — or overwrite the slot used by — that DLC content. See
[Deploy & purge](deploy-purge.md#the-skip-list-and-patch-numbering).

- **Label:** Skip List
- Add an entry with the **+** button: a popup asks for exactly 16 hexadecimal characters
  (placeholder: `eg. 0cf14e223de06a26`).
- Select an entry in the list and use the **-** button to remove it.

You won't normally need to touch this unless a specific mod's documentation tells you to add an entry for it.

## Mod updates

Update checks never run unless you ask: click **Check for Updates** on the Mods page, or turn on automatic checks
here. See [Updating mods](updating-mods.md).

- **Check for mod updates when DDMM starts** — off by default. When on, DDMM checks shortly after it starts and
  shows a notice plus the update badges if something can be updated. It never downloads or installs by itself,
  and it skips Nexus Mods mods (Nexus is only checked when you click **Check for Updates**).
- **then every N hours while DDMM is open** — off by default, and only available with the option above. Accepts
  1 to 168 hours.
- **Nexus Mods (optional)** — optional, and only used for Nexus update checks. Either:
    - **Sign in to Nexus Mods**: opens Nexus Mods in your browser, where you approve DDMM; Settings then shows
      *Signed in to Nexus Mods as &lt;account&gt;*. **Sign out** removes the sign-in (and asks Nexus to revoke it).
      Shown as *Coming soon* in versions that can't sign in yet.
    - **Or enter a personal API key manually** (collapsible): paste your personal key (Nexus Mods account → API
      Keys → Personal API Key; **Get a key** opens that page) and click **Save & verify**: DDMM checks it with
      Nexus and shows the account name. **Remove key** deletes it.

    If both are set, the sign-in is used. Both are stored in your system keychain (or, on Linux without one, an
    owner-only file in the data folder, which Settings points out), never in `settings.json`. See
    [Nexus Mods: optional sign-in or API key](updating-mods.md#nexus-mods-and-the-optional-api-key).

## After a Browser Install

What happens once a mod installed through the [browser extension or a `ddmm://` link](one-click-install.md)
finishes downloading:

- **Add to library only** — installed, but not added to any profile or deployed.
- **Add to active profile** — installed and added to your currently active profile.
- **Add to active profile and deploy** (the default) — installed, added to your active profile, and deployed
  immediately, so a browser install really is one click end to end.

The same setting decides what happens after DDMM [updates mods](updating-mods.md#after-updating-redeploy): with
*deploy*, it redeploys by itself; otherwise it asks.

If Helldivers 2 is currently running, DDMM skips the deploy step (the mod is still installed and added to your
profile) rather than risk writing into the game's files while it's open — deploy it yourself once you've closed
the game.

## Sites Allowed to Install Through the Extension

The sites you've clicked **Always Allow** for when the [browser extension](one-click-install.md) asked
permission to install a mod. Select **-** next to a site to revoke it — DDMM will ask again the next time that
site tries to install something.

## Auto-import from Downloads

Off by default. When enabled, DDMM watches your Downloads folder the entire time it's running — not just during a
[browser handoff](mod-sites.md#how-the-browser-handoff-works) — and offers to install any new archive that looks
like a Helldivers 2 mod, even from sites without a browser-extension button. It never installs anything without
you clicking to confirm.

## Browser Integration

Shows, per browser, whether DDMM is currently registered as a native-messaging host for the
[browser extension](one-click-install.md) — this happens automatically on every launch, so you normally won't
need to touch it.

- **Repair** — re-registers DDMM with that browser; use this if the extension reports it can't find DDMM (for
  example after moving a portable install to a new folder).
- **Remove** — unregisters DDMM from that browser.
- **Repair All** / **Remove All** — the same, for every browser at once.
- **Get the Browser Extension** — opens the extension's install page.

## Data Folder

Shows the folder DDMM is currently keeping `mods/`, `settings.json`, `profiles.json` and its logs in. Which folder
that is by default depends on how you installed DDMM; see
[Data location](../getting-started/download.md#data-location). The field itself isn't editable: use **Change...**
to move it. See [Data folder](data-folder.md) for everything about moving it.

- **Label:** Data Folder
- **Open Folder button:** opens that folder in your system's file manager
- **Change... button:** pick another folder; DDMM checks it, shows the size to move and the free space there, then
  moves your data and restarts. If the folder already holds DDMM data, it offers to use that data instead.
- **Reset to Default button** (only when you chose a folder): moves the data back to the default location the
  same way.

## Where settings live

Settings are saved to `settings.json` in DDMM's data folder (see **Data Folder** above), in a versioned format
(currently `V1`). You generally shouldn't need to hand-edit this file — use the Settings page instead.
