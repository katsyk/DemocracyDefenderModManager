<!--
Starting point for the next release's notes: copy this to v<version>.md
(e.g. v2.0.0-rc.6.md), replace every <version>, and fill in the
"What's new" section. build.yml only reads v<version>.md for the tag being
released, so this file never gets published itself.
-->

Democracy Defender Mod Manager (DDMM), release candidate <N>: *your mods, your democracy, defended.* It's still an
early build, but it's meant to be usable by anyone, not just developers.

## Quick start

1. **Download** the right file for you (see the table below).
2. **Run it.** DDMM looks for your Helldivers 2 install automatically. If Steam is in its usual place, there's
   nothing to set up.
3. **Add mods and click Deploy.** Drag in an archive or folder, or click "Add URL" and paste a link.

## Which file do I download?

| You want... | Download |
| --- | --- |
| A normal install on Windows (Start Menu shortcut, uninstaller) | `DDMM-<version>-windows-x64-setup.exe` |
| Windows without installing anything (unzip and run) | `DDMM-<version>-windows-x64-portable.zip` |
| Linux: Fedora, Nobara, openSUSE (`dnf`/`zypper`) | `DDMM-<version>-linux-x86_64.rpm` |
| Linux: Debian, Ubuntu, Mint (`apt`) | `DDMM-<version>-linux-amd64.deb` |
| Linux: other distros, no install (uses your desktop's graphics libraries) | `DDMM-<version>-linux-x86_64.AppImage` |
| Linux: Arch/CachyOS, or just the binary (needs WebKitGTK 4.1) | `DDMM-<version>-linux-x64.tar.gz` |

Linux install commands, the dependencies for each distro, and fixes for a blank window or AppImage/FUSE errors
are in [Installing on Linux](https://katsyk.github.io/DemocracyDefenderModManager/getting-started/linux/).

**One-click install (optional):** also grab the browser extension: `ddmm-extension-chrome-<ext-version>.zip`
(Chrome, Edge, Brave) or `ddmm-extension-firefox-<ext-version>.zip` (Firefox). See
[One-click install](https://katsyk.github.io/DemocracyDefenderModManager/using/one-click-install/).

Check `SHA256SUMS.txt` if you want to verify your download.

## What's new in <version>

-

## Known limitations

- **Windows builds are unsigned.** Windows SmartScreen will likely show a warning the first time you run the
  installer or the app. Click **"More info" → "Run anyway"** to continue.
- **The Linux .rpm and .deb are unsigned**, so openSUSE needs `zypper install --allow-unsigned-rpm`.
- **This is still a release candidate.** Expect some rough edges, and please report what you find.

## Reporting bugs / getting help

Open an issue: <https://github.com/katsyk/DemocracyDefenderModManager/issues>

Full documentation: <https://katsyk.github.io/DemocracyDefenderModManager/>
