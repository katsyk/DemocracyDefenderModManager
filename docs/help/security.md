---
title: Security
---

# Security

## No credentials, ever

DDMM never asks for, stores, or transmits a username, password, or API key for any mod site. Sites that require
being logged in to download hand the actual download off to your own browser, where your own session (and your
own login) does the work — see [Mod sites](../using/mod-sites.md). Update checks work the same way: they read
public page/release information, never anything gated behind your account.

## Archive extraction hardening

Every archive (`.zip`, `.7z`, `.rar`) is checked before any of its contents are written to disk:

- Every entry's path is validated against path traversal: an absolute path, a Windows drive-letter or UNC prefix,
  or a `..` component anywhere in the path is rejected — checked against a backslash-normalized copy of the name
  so a Windows-style `..\..\evil.txt` is caught the same as a Unix-style `../../evil.txt`.
- Symlink entries are rejected outright. Helldivers 2 mods never need them, and a symlink entry is a classic way
  to smuggle a write outside the destination folder.
- After extraction, DDMM re-walks what actually landed on disk (without following any symlinks it finds there)
  and confirms every file's fully resolved path is still inside the mod's own folder — a backstop for archive
  formats whose extraction routines don't sanitize paths themselves.

If any check fails, the whole archive is rejected and nothing from it is kept.

DDMM's `.7z` library dependency (`sevenz-rust2`) had its own path-traversal issue, tracked as
[CVE-2026-61725 / GHSA-qh76-45cr-8xrc](https://github.com/advisories/GHSA-qh76-45cr-8xrc); the dependency has been
upgraded to a version with the fix. It's worth noting DDMM's own entry-path checks above already caught and
rejected this exact kind of malicious `.7z` archive before the fix landed — this upgrade removes a second point
of exposure, it wasn't the only thing standing in the way.

## Downloads are `https://` only, size-capped, and content-checked

[Add URL](../using/adding-mods.md#add-url) only accepts `https://` links — plain `http://` and any other scheme
(`file:`, `javascript:`, etc.) are refused outright. Downloads are capped at 2 GiB, enforced both from the
server's reported size and by counting bytes as they stream in. Once downloaded, the file's actual content is
checked against known zip/7z/rar file signatures (not just its name or URL) before it's treated as a mod archive
— so a site that serves back an HTML login page instead of a file is rejected rather than "installed" as garbage.

## Links opened in your browser

A source's "Open on &lt;site&gt;" page link — whether declared by a mod's manifest, or recorded by DDMM when you
add a mod from a URL — is only ever opened if it's a plain `http://` or `https://` URL. Anything else a manifest
might contain (`javascript:`, `file:`, a custom scheme) is silently dropped and never handed to your OS's URL
opener.

## Filesystem safety when adding a local folder

Installing a mod from a plain folder (["Add Folder"](../using/adding-mods.md#add-folder) or drag & drop) copies
files in; it never follows symlinks found inside the source folder, so a symlink pointing outside it can't be used
to pull in files you didn't intend to share.

## The browser handoff only watches, never reads

During a [browser handoff](../using/mod-sites.md#how-the-browser-handoff-works), DDMM polls your configured
Downloads folder for a new file to appear — it never opens, uploads, or inspects the contents of anything else
already in that folder, only file names and sizes. A candidate that turns out to be a symlink is refused, the
same as any other archive install.

## Update checks are opt-in and size-capped

DDMM never checks for updates in the background — only when you explicitly click
[Check for updates](../using/updates.md). Each request (an AyakaMods mod page, or the GitHub releases API) is
`https://` only and capped at 5 MB, the same belt-and-braces size-limit approach used for archive downloads.
