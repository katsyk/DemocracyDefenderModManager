---
title: Download & install
---

# Download & install

DDMM is **portable**: there is no installer, and nothing is written outside the folder you put it in. The
download is a single executable — `ddmm.exe` on Windows, `ddmm` on Linux — plus, once you run it, a few small
files it creates next to itself (`settings.json`, `profiles.json`, a `mods/` folder, and its log file).

!!! warning "Preview software"
    Releases are marked as pre-releases (`2.0.0-preview3` and similar). Expect bugs, and back up any manually
    edited mod files before updating.

## Get the download

All releases are published on the
[GitHub Releases page](https://github.com/katsyk/DemocracyDefenderModManager/releases).

=== "Windows"

    1. Open the [releases page](https://github.com/katsyk/DemocracyDefenderModManager/releases) and download the
       latest `ddmm.exe`.
    2. Put it in its own folder — anywhere you like, as long as you (and DDMM) have write access to that folder.
       DDMM will store your mods and settings next to the executable.
    3. Run `ddmm.exe`. Windows SmartScreen may warn about an unrecognized publisher on unsigned preview builds;
       this is expected for a preview release.

=== "Linux"

    1. Open the [releases page](https://github.com/katsyk/DemocracyDefenderModManager/releases) and download the
       latest `ddmm` binary.
    2. Put it in its own folder, and make it executable:

       ```sh
       chmod +x ddmm
       ```

    3. Run it: `./ddmm`.
    4. DDMM is a [Tauri](https://tauri.app/) application and needs WebKitGTK at runtime. If it fails to launch,
       install your distribution's WebKitGTK package (Debian/Ubuntu: `libwebkit2gtk-4.1-0`) — see
       [Troubleshooting](../help/troubleshooting.md).

## Where things are stored

Because DDMM is portable, everything it manages lives next to the executable you run:

| Path (relative to the DDMM executable) | What it is |
| --- | --- |
| `mods/` | Every mod you've added, one subfolder per mod, each with its own `manifest.json` |
| `settings.json` | Your [settings](../using/settings.md) (game path, skip list) |
| `profiles.json` | Your [profiles](../using/profiles.md) and mod ordering |
| a rolling `*.log` file | DDMM's [log file](../help/logs.md) |

Moving the DDMM folder moves all of this with it. Deleting the folder removes DDMM and everything it manages —
your actual Helldivers 2 installation is untouched either way.

## Next step

Continue to [First-time setup](setup.md).
