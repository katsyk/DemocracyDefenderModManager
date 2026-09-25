---
title: Updating mods
---

# Updating mods

DDMM can tell you when a mod you installed has a newer version on the site it came from, and update it in place
(same mod, same profile settings) with one click, or all at once with **Update all**.

**Update checks never run unless you ask:** click **Check for Updates**, or turn on automatic checks in Settings.

## Checking for updates

- **Check for Updates** (Mods page, next to Add / Add Folder / Add URL) checks every installed mod right away and
  shows the results: each mod and site with its state (update available, up to date, skipped, unknown, "optional:
  sign in or add a key to check" for Nexus, or the error if the site couldn't be reached).
- **Automatic checks are off by default.** In [Settings → Mod updates](settings.md#mod-updates) you can turn on
  **Check for mod updates when DDMM starts**, and optionally **then every N hours while DDMM is open** (1–168
  hours; also off by default). An automatic check only shows a small notice and the update badges; it never
  downloads or installs anything by itself.
- **Automatic checks skip Nexus Mods.** Nexus only allows your sign-in or API key to be used for things you
  start yourself,
  so Nexus mods are checked only when you click **Check for Updates**. After an automatic check they show
  *Click Check for Updates to check Nexus mods* (or the result of the last check you ran this session). Every
  other site is still checked automatically.

A mod with an update gets a cloud badge and an **Update** button in your profile list, the Library's update button
becomes clickable, and **Update all (N)** appears next to Check for Updates.

## Which sites are supported

| Site | How DDMM checks | How the update is installed |
| --- | --- | --- |
| GitHub | The repository's latest release (public API, no key) | **One click**: DDMM downloads the release's `.zip`/`.7z`/`.rar` asset itself |
| GameBanana | The mod's public API entry (version and files, no key) | **One click**: DDMM downloads the new file itself |
| ModWorkshop | The mod's public API entry (version and files, no key) | **One click**: DDMM downloads the new file itself (a mod whose download is an external link goes through the browser) |
| AyakaMods | The version published on the mod page | **In your browser** (downloads need your login) |
| Nexus Mods | The Nexus Mods API, **only if you (optionally) sign in to Nexus Mods or add your own API key**, and only when you click Check for Updates | **In your browser**, always |
| Anything else | Not checked | — |

A mod can have more than one source (for example, declared by its author and recorded by DDMM); each is checked.
Mods with no recognized source are simply skipped.

DDMM is polite to every site: requests are spaced out per site, time out after 20 seconds, identify DDMM with a
clear User-Agent, use HTTPS only, and read at most 5 MB per response.

## Updating

### One click (GitHub, GameBanana, ModWorkshop)

Click **Update** on the mod (or **Update from &lt;site&gt;** in its menu, or **Update** in the results). DDMM
downloads the new file, shows its progress, checks the archive exactly like any other install (archive type by
content, path-traversal and symlink checks, size limit), and replaces the mod's files in place.

If the mod page has several files (say, a main file and a "no sounds" variant), DDMM picks the one matching what
you installed when it can tell. Otherwise it asks which file you want.

### In your browser (AyakaMods, Nexus Mods, other login-gated sites)

These sites only give files to logged-in people, so the download happens in your browser, with your own login:

- **With the [DDMM browser extension](one-click-install.md) connected** (it has talked to DDMM recently), DDMM
  opens the mod page and you click **Update with DDMM** there. On Nexus Mods, pick the file and click Nexus's
  own download button ("Slow download" for free accounts). DDMM never clicks that for you. The update is
  installed in place as soon as it lands, and DDMM's window notices by itself.
- **Without the extension** (or if you choose *Use Downloads-folder watch instead*), DDMM opens the page and
  watches your Downloads folder, the same [browser handoff](mod-sites.md#how-the-browser-handoff-works) used
  for adding mods.

### Update all

**Update all (N)** runs every one-click update first, one after another, asking which file only where that's
unclear. Then it goes through the updates that need your browser **one page at a time**: for each one you can
open the page, skip to the next, or stop.

### Skip this version

Don't want a particular update? Use **Skip** (in the results) or **Skip version X** (in the mod's menu). DDMM stops
showing that version; a later version shows up normally. **Stop skipping** in the same places undoes it. The
skip is stored with the mod (in its `.hd2mm-origin.json`) and is cleared when the mod is updated.

### After updating: redeploy

The game only sees updated files after a deploy. If an updated mod is enabled in your active profile:

- with **After a Browser Install** set to *Add to active profile and deploy* (the default), DDMM redeploys right
  away, unless Helldivers 2 is running (then it tells you to deploy after closing the game);
- with either other setting, DDMM asks whether to redeploy now.

Updates finished through the browser extension follow the same setting, as every extension install does.

**GUID handling:** if the update's archive ships no `manifest.json`, the mod keeps its **original** GUID, so
your profile's enabled state, options and position for it stay as they were. If the update ships its own
manifest with its own `Guid`, that GUID is used instead, and profile entries that point at the old GUID no longer
match it (DDMM doesn't migrate them).

## Nexus Mods: optional sign-in or API key { #nexus-mods-and-the-optional-api-key }

Nexus Mods only answers these questions ("is there a newer file?") for people who are signed in or have an API
key. Both are **optional**, and only used for Nexus update checks:

- **Neither** (the default): Nexus mods show *Optional: sign in or add a key to check*. Everything else in DDMM
  works exactly the same.
- **Sign in to Nexus Mods** (recommended), or **a personal API key** entered manually: DDMM can check Nexus mods
  too. If you do both, the sign-in is used.

Both live in **Settings → Mod updates → Nexus Mods (optional)**.

### Signing in { #signing-in-to-nexus-mods }

Click **Sign in to Nexus Mods**. DDMM opens Nexus Mods in your usual browser, where you log in (if you aren't
already) and approve DDMM. The browser then shows *You can close this tab and return to DDMM*, and Settings shows
*Signed in to Nexus Mods as &lt;your account&gt;*.

- You log in **on the Nexus Mods website**, never in DDMM: DDMM never sees your password. What DDMM receives is
  a sign-in token that Nexus issues for DDMM, which only allows the kind of read-only requests update checks
  make.
- DDMM waits up to 5 minutes for you to approve, and **Cancel** stops waiting. While it waits, DDMM listens on
  `127.0.0.1` port `28647` on your own computer (never reachable from the network) for your browser to hand the
  sign-in back; it stops listening right after. If another program is already using that port, DDMM says so;
  close that program and try again.
- The sign-in renews itself in the background of a check you start, shortly before it would expire.
- **Sign out** removes the sign-in from this computer (always) and asks Nexus to revoke it. You can also
  revoke DDMM's access on the Nexus Mods website at any time (your account's authorized applications); the next
  Nexus check then signs DDMM out and tells you so.

If the button is greyed out with *Coming soon*, this version of DDMM can't sign in to Nexus Mods yet. The API key
below works in every version.

### Using a personal API key instead { #using-a-personal-api-key }

Under **Or enter a personal API key manually**: on Nexus Mods, open your account's **API Keys** page (**Get a
key** opens it) and copy the **Personal API Key** at the bottom. Paste it and click **Save & verify**. DDMM checks
it with Nexus before saving and shows the account it belongs to. **Remove key** deletes it; you can also revoke the
key on that Nexus page at any time, and DDMM will then say Nexus didn't accept it.

### What DDMM does, and doesn't do, with them { #nexus-privacy }

- **Only update checks, and only when you click.** Nexus is contacted only for things you start: **Check for
  Updates**, **Sign in to Nexus Mods**, **Sign out**, and **Save & verify**. The automatic checks never use your
  sign-in or key.
- **As few requests as possible:**
    - a mod checked in the last 5 minutes isn't checked again. If that's all of them, DDMM says *All your Nexus
      mods were checked recently. Update checks are limited to save your Nexus API requests.*;
    - a mod never checked, or not checked for a month, gets its own request for its file list;
    - every other mod is covered by one "recently updated mods" request, for the last day, week or month
      (the shortest period that reaches back to the oldest check). Only the mods that list shows as changed
      get a file-list request.

  DDMM reads Nexus's rate-limit headers and stops early rather than use up your allowance.
- **Never for downloading.** Downloading through the API is a Nexus Premium feature, and DDMM doesn't use it,
  for anyone. Nexus updates always go through your browser, where you click Nexus's own download button.
- **Stored in your system's keychain** (Windows Credential Manager, macOS Keychain, or the Secret Service on
  Linux, such as GNOME Keyring or KWallet). On a Linux desktop with no keychain running, they're stored instead as
  files only your user can read (`nexus-oauth-tokens` for the sign-in, `nexus-api-key` for the key, in DDMM's
  data folder), and Settings says so.
- **Never** written to `settings.json`, never logged, never shown in DDMM's window, never passed to the browser
  extension, and only ever sent to Nexus Mods over HTTPS: the sign-in to `https://users.nexusmods.com` (to sign
  in, renew, sign out, and read your account name) and `https://api.nexusmods.com` (update checks); the key only
  to `https://api.nexusmods.com`.
- **What Nexus sees:** the requests come from your computer, identified as *Democracy Defender Mod Manager* and
  its version, as Nexus asks of every application. DDMM keeps only your account name (to show it in Settings).

## How DDMM knows a mod's installed version

A check compares the version DDMM knows is installed with the site's current version. If it doesn't know the
installed version, the result is "unknown", never a guess. The installed version comes from, in order:

1. what DDMM recorded in the mod's `.hd2mm-origin.json` when it installed or updated it (DDMM knows what it
   actually installed);
2. otherwise, `Version` on the matching entry in the mod's own manifest `Sources` (see
   [The Sources field](../authors/sources.md)).

DDMM records the version automatically wherever it can:

- **AyakaMods, GameBanana, ModWorkshop:** when a mod is installed through DDMM (browser extension, browser
  handoff or "choose file"), DDMM looks up the mod's current version on the site once and records it.
- **GitHub:** the release tag, from the release-asset link or the update itself.
- **Nexus Mods:** Nexus names every download `<name>-<mod id>-<version>-<upload time>.<ext>`, so DDMM reads the
  version and the exact file from the archive's own name. No key and no request is needed for that. For mods with
  several files (main file, optional variants), this is how DDMM follows *your* file: a newer version of the
  optional file you have counts as your update, not the main file.
- **One-click updates** record the new version and file they installed.

When a site doesn't publish a version number at all (common on GameBanana and ModWorkshop), DDMM uses the date the
newest file was uploaded (for example `2026-04-29 05:49 UTC`) as its version.

**For mod authors:** if you declare a source in your manifest's `Sources` field, set its `Version` to match what
the site publishes. That makes update checks work even for people who didn't install through DDMM. See
[The Sources field](../authors/sources.md).

## Privacy

- Nothing is checked unless you ask (the button, or the automatic checks you turned on). Nexus Mods is only
  contacted when you click.
- Checks only contact the sites your mods came from, with the mod's id on that site. No list of your mods is
  sent anywhere else, and there's no DDMM server involved.
- The only credential DDMM can hold is the optional Nexus API key described above.
