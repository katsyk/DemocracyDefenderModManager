---
title: First-time setup
---

# First-time setup

The first time DDMM starts, it checks whether your settings are valid. If they aren't (which is always true on
a completely fresh install, since the game path starts empty), you're sent straight to the **Settings** page.

## Set your Game Path

On the Settings page, fill in **Game Path** — the folder your Helldivers 2 install lives in. For a default Steam
install this looks like:

```text
Steam/steamapps/common/Helldivers 2/
```

Use the **Browse...** button next to the field to pick the folder instead of typing it out.

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

## The Skip List

The Settings page also has a **Skip List**, which is a more advanced setting related to how mods are deployed —
see [Settings reference](../using/settings.md#skip-list) for what it does and when you'd want one. You can safely
leave it empty for now.

## Next step

Once Game Path is valid, move on to [Your first mod](first-mod.md).
