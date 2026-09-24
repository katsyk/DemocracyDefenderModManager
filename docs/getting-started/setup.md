---
title: First-time setup
---

# First-time setup

The first time DDMM starts, it checks whether your settings are valid. On a completely fresh install (or whenever
the game path becomes invalid — say, after moving your Steam library), the game path starts out empty, so DDMM
tries to help before bothering you about it:

1. It looks for a Helldivers 2 install via Steam automatically — checking common Steam install locations, every
   library Steam knows about, and Helldivers 2's own install manifest.
2. If that finds a valid install, DDMM fills in and saves **Game Path** for you, shows a
   "Found Helldivers 2 at ..." notification, and you go straight to the Mods page — no Settings visit needed.
3. If it *doesn't* find anything (Steam isn't installed, the game isn't installed, or it's somewhere
   auto-detection doesn't check), you're sent to the **Settings** page to set it yourself.

## Setting the Game Path by hand

If you land on Settings, fill in **Game Path** — the folder your Helldivers 2 install lives in. For a default
Steam install this looks like:

```text
Steam/steamapps/common/Helldivers 2/
```

Two ways to fill it in without typing the full path:

- **Auto-detect** — runs the same Steam lookup described above, on demand. Useful if you install Helldivers 2
  *after* first starting DDMM, or if the automatic check on startup didn't find it for some reason.
- **Browse...** — opens a normal folder picker.

DDMM validates the folder as you type, and won't let you leave the Settings page until it's valid. It checks that
the path:

- exists,
- contains a `tools` directory,
- contains a `data` directory, and
- contains a `bin` directory, which in turn contains `helldivers2.exe`.

If any of those checks fail, DDMM tells you exactly which one, right under the field:

| Message | Meaning |
| --- | --- |
| Game path can not be empty! | Nothing entered yet |
| Game path does not exist! | The folder itself wasn't found |
| Game path is invalid! | The path couldn't be read at all |
| Game path does not contain a directory named "tools"! | Not a Helldivers 2 install folder |
| Game path does not contain a directory named "data"! | Not a Helldivers 2 install folder |
| Game path does not contain a directory named "bin"! | Not a Helldivers 2 install folder |
| Game path's "bin" directory does not contain the "helldivers2.exe"! | `bin` exists, but the game executable doesn't |

## The rest of Settings

Two other things live on the Settings page, but neither blocks you from getting started:

- **Downloads Folder** — already defaults to your OS's normal Downloads folder; only matters once you use a
  [browser handoff](../using/mod-sites.md#how-the-browser-handoff-works).
- **Skip List** — a more advanced setting related to how mods are deployed, see
  [Settings reference](../using/settings.md#skip-list). Safe to leave empty.

Once Game Path is valid, move on to [Your first mod](first-mod.md).
