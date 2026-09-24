---
title: Logs
---

# Logs

DDMM writes a rotating log file named after the application (e.g. `Democracy Defender Mod Manager.log`) into a
`logs/` folder inside its [data folder](../getting-started/download.md#data-location) — the same place that holds
`mods/`, `settings.json`, and `profiles.json`. Which directory that actually is (portable, next to the
executable, or your OS's per-user app data directory) depends on how you're running DDMM; open **Settings** and
use **Open Folder** next to **Data Folder** to jump straight there without having to work it out yourself. The
very first lines of the log, on every startup, say which one was chosen and why.

The log level is `Debug` in development builds and `Info` in released builds, so a released build's log captures
warnings, errors, and the high-level step-by-step of what DDMM did (loading mods, adding a mod, deploying,
purging, and so on), without full debug-level noise.

The same log stream is also mirrored to:

- the terminal, if you ran DDMM from one, and
- the webview's developer console.

## When to check the log

If an install, deploy, or purge fails, the popup error message is usually enough — but the log file has the full
detail (including the exact file paths involved) if you need it, and it's what you should attach when
[reporting a bug](bugs.md).
