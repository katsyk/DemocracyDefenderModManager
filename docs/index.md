---
title: Home
---

# Democracy Defender Mod Manager

*Your mods. Your democracy. Defended.*

**DDMM** is a source-neutral mod manager for **Helldivers 2**. It installs mods from an archive, a plain folder,
or a direct download link, keeps track of where each mod came from, and deploys them into your game install with
one click — without ever assuming, or requiring, a particular mod site.

!!! abstract "Works with"
    **AyakaMods · Nexus Mods · ModWorkshop · GameBanana · GitHub · any direct link · files already on your disk**

    No account on any mod site is ever required to use DDMM.

DDMM is built on [Tauri 2](https://tauri.app/) — a Rust backend with a SvelteKit frontend — and is free, open
source software under Apache-2.0. See [Credits & license](about.md) for the full story of where it comes from.

!!! tip "Made with love for the AyakaMods community ❤️"
    AyakaMods is one of the best homes for Helldivers 2 modding, and it's DDMM's first-class citizen — no site
    is more welcome here. [Visit AyakaMods →](https://ayakamods.com/games/helldivers-2.119/)

## Quick links

<div class="grid cards" markdown>

-   :material-download:{ .lg .middle } **Get started**

    ---

    Download DDMM and point it at your Helldivers 2 install.

    [:octicons-arrow-right-24: Download & install](getting-started/download.md)

-   :material-puzzle:{ .lg .middle } **Add your first mod**

    ---

    Install from an archive, a folder, a URL, or drag & drop.

    [:octicons-arrow-right-24: Your first mod](getting-started/first-mod.md)

-   :material-web:{ .lg .middle } **Mod sites**

    ---

    How AyakaMods, Nexus Mods, and other sites work with DDMM.

    [:octicons-arrow-right-24: Mod sites](using/mod-sites.md)

-   :material-file-code:{ .lg .middle } **Packaging a mod**

    ---

    Manifest formats, the `Sources` field, and JSON Schemas for mod authors.

    [:octicons-arrow-right-24: For mod authors](authors/index.md)

</div>

## The DDMM Promise

*Every Helldiver takes an oath. This one's ours.*

- **Every mod is welcome.** DDMM will never block, blacklist, or rank mods or mod sites.
- **No account, anywhere.** No mod site's login or credentials are ever asked for.
- **Your files, your choice.** Your mods live in a folder you control.
- **Open source, forever.** Apache-2.0, today and always.
- **Updates on your terms.** Update checks never run unless you ask: click **Check for Updates**, or turn on automatic checks in Settings.

## What makes DDMM source-neutral

- **Any archive** (`.zip`, `.7z`, `.rar`) can be added regardless of where it came from.
- **Plain folders** can be added directly, with no archive step, via "Add Folder" or by dragging a folder in.
- **Direct download links** can be added via "Add URL". For sites that gate downloads behind a login or
  JavaScript, DDMM hands the page to your browser and picks up the file once you download it — see
  [Mod sites](using/mod-sites.md).
- **Where a mod came from** is tracked without ever hard-coding a particular site: the manifest's `Sources` field,
  the legacy `NexusData` field, and DDMM's own install-time bookkeeping all feed into the same "Open on &lt;site&gt;"
  menu.
- **Archive extraction is hardened** against path traversal and symlink entries, regardless of which site an
  archive came from.

!!! warning "Release candidate"
    DDMM is release-candidate software (currently versioned `2.0.0-rc.8`). Expect some rough edges, and please
    [report bugs](help/bugs.md) you run into.
