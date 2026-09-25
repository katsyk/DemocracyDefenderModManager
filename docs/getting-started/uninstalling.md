---
title: Uninstalling
---

# Uninstalling

How you uninstall DDMM depends on how you installed it — see [Data location](download.md#data-location) if
you're not sure which of these applies to you.

## Installed with the Windows installer

1. If you have mods deployed, click **Purge** in DDMM first to remove them from your Helldivers 2 `data` folder
   cleanly. (See [Deploy & purge](../using/deploy-purge.md) for exactly what this deletes.)
2. Uninstall DDMM the normal Windows way — Start Menu → right-click DDMM → Uninstall, or **Settings → Apps →
   Democracy Defender Mod Manager → Uninstall**.
3. The uninstaller removes the app itself, but *not* your data directory
   (`%APPDATA%\io.github.katsyk.ddmm`) — that's deliberate, in case you reinstall later. Delete that folder by
   hand afterward if you want your mods, settings and logs gone too. **Open Folder** next to **Data Folder** in
   Settings will take you straight there before you uninstall, if you want to check what's in it first.

## Installed with the .deb or .rpm (Linux)

1. **Purge** first, as above.
2. Remove the package. It's called `democracy-defender-mod-manager`, not `ddmm`:
    - .deb: `sudo apt remove democracy-defender-mod-manager`
    - .rpm: `sudo dnf remove democracy-defender-mod-manager` (openSUSE: `sudo zypper remove democracy-defender-mod-manager`)
3. As with the Windows installer, this doesn't delete `~/.local/share/io.github.katsyk.ddmm`. Remove it by hand
   if you want a completely clean removal.

## Portable (Windows zip, or an AppImage/tar.gz you didn't move to app data)

DDMM doesn't run an installer in this case, so there's nothing to "uninstall" beyond removing the files:

1. **Purge** first, as above.
2. Close DDMM and delete the folder you put it in — the executable, `portable.txt`, `mods/`, `settings.json`,
   `profiles.json`, and `logs/` all live there, so deleting the folder removes everything DDMM created.

In every case, your Helldivers 2 installation itself is never touched beyond the `data` folder changes
Deploy/Purge make.
