---
title: For mod authors
---

# For mod authors

- [Packaging your mod](packaging.md) — folder layouts and patch-file naming DDMM understands
- [Manifest reference](manifest.md) — Legacy, V1, and V2 `manifest.json` formats, plus JSON Schemas
- [The Sources field](sources.md) — declaring where your mod can be found, per site, with examples

DDMM reads [Helldivers 2 Mod Manager](https://github.com/teutinsa/Helldivers2ModManager)'s original manifest
formats unchanged, plus its own provider-neutral `Sources` field layered on top — a manifest written for the
original manager works in DDMM without modification, and a `Sources`-aware manifest still works in managers that
don't know about it.
