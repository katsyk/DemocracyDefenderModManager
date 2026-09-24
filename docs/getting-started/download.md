---
title: Download & install
---

# Download & install

## Get the download

All releases are published on the
[GitHub Releases page](https://github.com/katsyk/DemocracyDefenderModManager/releases).

!!! warning "Release candidate"
    Releases are marked as pre-releases (`2.0.0-rc.3` and similar). Expect some rough edges, and back up any
    manually edited mod files before updating.

=== "Windows"

    Two options — pick one:

    - **Installer** (`DDMM-<version>-windows-x64-setup.exe`): a normal per-user install with a Start Menu
      shortcut and a proper uninstaller. No administrator prompt — it installs for your user account only.
    - **Portable** (`DDMM-<version>-windows-x64-portable.zip`): unzip it anywhere and run `ddmm.exe` directly, no
      install step. See [Data location](#data-location) below for what "portable" actually means here.

    Windows SmartScreen will likely warn about an unrecognized publisher the first time you run either one — these
    builds aren't code-signed yet. Click **"More info" → "Run anyway"**; this is expected for an unsigned
    community build, see [Troubleshooting](../help/troubleshooting.md#windows-smartscreen-warns-about-an-unknown-publisher).

    Either way, DDMM needs [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) at runtime, which
    ships with Windows 11 and most up-to-date Windows 10 installs already. If it's missing, the installer offers
    to download it automatically.

=== "Linux"

    Three options:

    - **AppImage** (`DDMM-<version>-linux-x86_64.AppImage`): works on most distributions without installing
      anything. Make it executable and run it:

      ```sh
      chmod +x DDMM-*-linux-x86_64.AppImage
      ./DDMM-*-linux-x86_64.AppImage
      ```

    - **.deb** (`DDMM-<version>-linux-amd64.deb`): for Debian/Ubuntu-based distributions.

      ```sh
      sudo apt install ./DDMM-*-linux-amd64.deb
      ```

    - **tar.gz** (`DDMM-<version>-linux-x64.tar.gz`): just the `ddmm` binary plus a quick-start text file. Extract
      it, make the binary executable, and run it.

    DDMM is a [Tauri](https://tauri.app/) application and needs WebKitGTK at runtime. If it fails to launch,
    install your distribution's WebKitGTK package (Debian/Ubuntu: `libwebkit2gtk-4.1-0`) — see
    [Troubleshooting](../help/troubleshooting.md).

Check `SHA256SUMS.txt` on the release page if you want to verify your download.

## Data location

Where DDMM keeps your mods, settings, profiles and logs depends on how you're running it:

- **Installed** (the Windows installer, the `.deb`, or an AppImage run from a location that isn't writable) —
  DDMM uses your OS's normal per-user application data directory: `%APPDATA%\io.github.katsyk.ddmm` on Windows,
  `~/.local/share/io.github.katsyk.ddmm` on Linux.
- **Portable** — DDMM keeps everything in the same folder as its own executable instead. This is used
  automatically when either is true, *and* that folder is writable:
    - a `portable.txt` file sits next to the executable (already the case if you downloaded the Windows portable
      zip — it's included inside), or
    - the folder already has a `mods/` directory or `settings.json` file in it, from an older portable-only
      release.

  For an AppImage, "the same folder as its own executable" means the folder containing the `.AppImage` file
  itself, not the temporary location it's mounted at while running.

You can always check which one is active, and where, without guessing: open **Settings** and look at
**Data Folder** — it's read-only, and has an **Open Folder** button. The choice (and why) is also written to the
[log file](../help/logs.md) every time DDMM starts.

Moving to portable mode later: create an empty `portable.txt` next to the executable (or copy the one from a
portable download) and restart DDMM — it'll start using that folder from then on. Note this does *not* move your
existing mods/settings for you; do that by hand first if you want to keep them (see **Open Folder** in Settings
to find where they currently are).

## Next step

Continue to [First-time setup](setup.md).
