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

## Update checks are opt-in

DDMM never checks for updates in the background — only when you explicitly click
[Check for updates](../using/updates.md).
