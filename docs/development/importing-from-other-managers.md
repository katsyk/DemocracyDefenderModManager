---
title: Importing from other mod managers (research)
---

# Importing from other mod managers: how they store mods

Internal notes behind **Import mods** (`src-tauri/src/mod_import.rs`, `mod_import/layouts.rs`,
`commands/import.rs`). This page names the tools because the code has to know their formats. The UI and the
user docs ([Importing mods](../using/importing-mods.md)) never do. They only say "another mod manager".

Researched September 2026. None of these formats is a published contract, so every parser is tolerant: an
unknown or missing field is skipped, never treated as an error, and the generic folder scan always works as a
fallback.

## Design constraints

- **Files on disk only.** Nothing is downloaded, from Nexus Mods or anywhere else. Re-downloading a user's
  Nexus mods automatically is ruled out: the Nexus API can't be used for downloads, and Nexus's "Slow download"
  and its timer are never automated or skipped. Import works only from what the other manager (or the user's
  Downloads folder) already has.
- **The source is read-only.** DDMM copies or extracts from it and never writes, moves or deletes anything
  there. `ensure_source_allowed` refuses a source that is, contains or is inside DDMM's data folder, and the
  check runs again for every item.
- **Nexus metadata** comes from what the source recorded, or from the Nexus download file name. No API call is
  made during import. A later update check, with the user's optional key or sign-in, works from what was
  recorded.

## HD2 Arsenal

Confidence: **high for 0.36.2** (the last public build). Newer builds were not checked.

- **Source.** The development repo `github.com/Orbit-Studios/hd2arsenal` is private. The only public source is
  the Debian build at [leguteape/hd2arsenal-release](https://github.com/leguteape/hd2arsenal-release) (v0.36.2,
  2026-06-26). Its `app.asar` holds obfuscated JavaScript, which was run through `webcrack` for this research.
  - Nexus page: <https://www.nexusmods.com/helldivers2/mods/4664>.
  - Site: rsnl.gg.
- **Data folder.** `path.join(app.getPath("appData").replace("Roaming", "Local"), "hd2arsenal")`
  (`main_constants.js`).
  - Windows: `%LOCALAPPDATA%\hd2arsenal\`. A third-party helper,
    [CurrCa7/Helldivers2-Arsenal-Download-Accelerator](https://github.com/CurrCa7/Helldivers2-Arsenal-Download-Accelerator)
    (`nxm_handler.js`), hardcodes the same path.
  - Linux (native .deb): `~/.config/hd2arsenal/`. There is no "Roaming" in that path, so the replace does
    nothing.
  - Subfolders: `mods/`, `temp/`, `logs/`, `profile_images/`. The user can override the mods folder with
    `userModsDir`, and the library stores absolute paths.
- **`hd2a_data.json`** is the database. It is written atomically and has `.bak` copies. Library version 1
  (`librarySystemVersion: 1`) looks like this:

```json
{
  "librarySystemVersion": 1,
  "selectedProfile": "default",
  "setTopPriority": false,
  "modsLibrary": [{
    "uuid": "<manifest Guid or random UUID>",
    "path": "C:\\Users\\X\\AppData\\Local\\hd2arsenal\\mods\\<cleaned archive name>",
    "label": "Mod Name", "description": "",
    "nexusData": { "modId": "1234", "fileId": "5678", "version": "1.2", "updateTimestamp": 1712345678 },
    "options": [{ "name": "Opt", "description": "", "include": ["Folder"], "enabled": true,
                  "suboptions": [{ "name": "Sub", "include": ["Folder/Sub"], "enabled": true }] }]
  }],
  "modsList": {
    "default": { "label": "Default", "mods": [
      { "uuid": "...", "enabled": true, "optionsConfig": [{ "name": "Opt", "enabled": true,
                                                            "suboptions": [{ "name": "Sub", "enabled": true }] }] },
      { "type": "separator", "label": "..." }
    ] }
  }
}
```

  How each part is read:
  - **Nexus ids.** `nexusData` is set only for mods installed through Arsenal's `nxm://` flow
    (`wk_uploadWorker.js`). `version` and `updateTimestamp` come from Nexus's file info
    (`main_ipcHandlers.js`: `updateTimestamp: fileInfo.uploaded_at`). Local installs have `null`.
  - **Enabled state, load order and options.** These are per profile, in `modsList[selectedProfile].mods`.
    Array order is the deploy order: index 0 first, reversed when `setTopPriority` is true (`wk_deployWorker.js`).
    Separators are skipped. `optionsConfig` is matched to the library entry's options **by name**
    (`mergeOptionsWithConfig`).
  - **Older format.** Before library version 1, each profile's `mods[]` held full mod objects that included
    `path`. DDMM falls back to reading those.
  - **Option includes** are relative to the mod folder. Icon paths point into Arsenal's own cache and aren't
    imported.
  - `mod_headers.db` (SQLite) is only used for conflict detection and isn't needed.
  - `temp/` holds Nexus downloads in progress or awaiting install. It is skipped.

**What DDMM does with it:**
- When a picked folder, or its parent, has `hd2a_data.json`, the mod list comes from `modsLibrary`. The
  folder isn't walked.
- For a mod folder without its own `manifest.json`, DDMM writes a v1 manifest from Arsenal's label and options,
  and keeps the Arsenal `uuid` as its GUID. The GUID is what lets a second import recognize the mod.
- `nexusData` is recorded in `.hd2mm-origin.json` so update checks work: the mod id, the file id, the version,
  and the upload time.
- `enabled`, order and `optionsConfig` become the DDMM profile entry.

## Helldivers 2 Mod Manager (teutinsa)

Its manifest format is the one DDMM already reads.

### 1.x (WPF, `master`, 1.3.0.1, end of life)

Confidence: high. Source: [teutinsa/Helldivers2ModManager](https://github.com/teutinsa/Helldivers2ModManager).

- **Settings.** `settings.json` is opened by a relative path, so it sits next to the exe
  (`Services/SettingsService.cs`). Its keys are `GameDirectory`, `StorageDirectory`, `TempDirectory`,
  `LogLevel`, `Opacity`, `SkipList` and `CaseSensitiveSearch`.
- **Default storage:** `%LOCALAPPDATA%\Helldivers2ModManager`. The default temp folder is
  `%LOCALAPPDATA%\Temp\Helldivers2ModManager`.
- **Mods** are stored in `<Storage>\Mods\<manifest.Name>\` with their `manifest.json` (`ModService.cs`). A mod
  without a manifest gets a generated one: `Name` is the archive name, the `Guid` is new, and the first-level
  folders become `Options` (`ModManifest.cs`).
- **Enabled state and options** are in `<Storage>\enabled.json`, an array in load order
  (`ProfileService.cs`, `Models/EnabledData.cs`):
  `[{"Guid": "...", "Enabled": true, "Toggled": [true, false], "Selected": [0, 2]}]`. For a legacy manifest,
  `Selected` has a single element.
- There are no Nexus ids anywhere in 1.x.

### 2.0 rewrite (Tauri, `2.0-rewrite` branch, Preview 3 from 2026-04-25)

- **Location.** Everything lives next to the exe (`current_exe().parent()`), which can't be auto-detected. The
  user picks the folder.
- **Mods** are in `mods/<archive name>/manifest.json`.
- **Profiles** are in `profiles.json`, in the same format DDMM uses (`{"Profiles": [{"Version": "V1",
  "Name": ..., "Configs": [...]}], "Active": 0}`). `Configs` order is the load order.
- **Nexus ids.** Manifest v1/v2 can carry `NexusData.ModId`, and v1 can also carry `Version`.

### Original 2024 manager (archived `teutinsa/HD2ModManager`)

- **Data** is under `%APPDATA%\HD2ModManager\`, with `Mods\<archive name>\manifest.json`.
- **`enabled.json`** is a map `{"<guid>": optionIndex}` that lists the enabled mods only
  (`HD2ModManagerLib/HD2ModManager.cs`).

**What DDMM does with them.** It auto-detects `%LOCALAPPDATA%\Helldivers2ModManager\Mods` and
`%APPDATA%\HD2ModManager\Mods`. Any folder of mod folders works, though, because each mod folder has a
`manifest.json`. `profiles.json` or `enabled.json` in the folder, or in its parent, supplies on/off state, order
and options by GUID. The folder's own manifest, and so its GUID, is copied with it, so the profile entries fit
as they are.

## Vortex (with the Helldivers 2 extension)

Confidence: high for the paths and file formats; not verified for the state database encoding.

**Sources:**
- Vortex, [Nexus-Mods/Vortex](https://github.com/Nexus-Mods/Vortex): `getInstallPath.ts`,
  `getDownloadPath.ts`, `stagingDirectory.ts`, `activationStore.ts`, `LevelPersist.ts`, `guessModID.ts`.
- The HD2 extension by ChemBoy1, `ChemGuy1611/ChemBoy1-Vortex-Games`, `game-helldivers2/index.js` v1.1.1
  ([Nexus site mod 845](https://www.nexusmods.com/site/mods/845)).

**Layout:**
- **Staging** defaults to `%APPDATA%\Vortex\helldivers2\mods`. It holds one folder per mod, named after the
  archive (so it keeps the Nexus-style name), and a `__vortex_staging_folder` marker file. Only the options
  chosen at install time are staged, flattened.
- **Downloads** default to `%APPDATA%\Vortex\downloads\helldivers2`, and keep the original Nexus file names.
- **Patch order.** The extension keeps each profile's order in
  `%APPDATA%\Vortex\helldivers2\<profileId>_patch_order.json`, next to the staging folder. The file is an array
  of `{id, modId (the staging folder name), name, enabled, locked, data}`.
- **`vortex.deployment*.json`** in the game folder lists deployed files and the staging folder each came from.
  It has no Nexus ids and isn't needed.
- **Nexus mod id, file id and version** exist only in Vortex's state database, a LevelDB at
  `%APPDATA%\Vortex\state.v2` (keys like `persistent###mods###helldivers2###<id>###attributes###modId`).
  Reading it would need a LevelDB reader and an unverified value encoding. **Not done:** the staging folder
  names and download file names already carry the mod id and version.

**What DDMM does with it:**
- It auto-detects both folders.
- In the staging folder each mod folder is one item. The Nexus id and version come from the folder name, and
  the newest `*_patch_order.json` supplies order and on/off state.
- The download folder is scanned like any archive folder. Mods already imported from staging show as "already
  in DDMM" (matched by Nexus id and version).
- Vortex doesn't run natively on Linux. Under Wine/Proton its folders are inside a prefix, which can't be
  guessed, so the user picks them.

## Nexus Mods download file names

Confidence: high. DDMM parses both schemes (`sources::parse_nexus_archive_name`).

- **Until 11 June 2026:** `<name>-<modId>-<version, dots as dashes>-<unix upload time>.<ext>`, for example
  `Better Stims-1234-1-2-0-1718000000.zip`. Vortex's `guessModID.ts` and MO2's `nexusinterface.cpp` parse the
  same pattern.
- **Since 11 June 2026:** `<name> <modId> <version> <YYYY-MM-DDTHH-MMZ> <slug>.<ext>`, space-separated, for
  example `PawnCompanion 1377 1.35 2026-06-24T03-45Z G8alq8bQH.zip`. The name itself can contain spaces and
  version-like parts (a user's sample was `GiftOfSpellCraft 0.0.1 184672 1 2026-07-07T17-09Z GkqKVMPSF.zip`), so
  the name is parsed from the right.
  - Nexus says the name is "intentionally not designed to uniquely identify a file… not a contract" and
    recommends MD5 lookups instead. Source: the Nexus forum thread
    [Changes to downloaded file names](https://forums.nexusmods.com/topic/13539100-changes-to-downloaded-file-names/),
    the 18 June announcement, and a staff follow-up on 7 July.
  - The announcement's `{FileName}_{Version}_{UniqueSlug}` form carries no mod id and isn't recognized.
  - The new time has minute precision, and Nexus's update matching compares exact upload times. So a new-style
    name records the mod id and version, but no `uploaded_at`. Matching then falls back to the file name.
- A browser's ` (1)` duplicate suffix is ignored.

## Other managers found (not auto-detected)

- **h2mm-cli (v4n00, bash):** `<game>/data/mods.csv` holds `id,ENABLED|DISABLED,name,nexusModId,nexusVersion,files…`.
  Disabled files are renamed inside the game folder. Its data *is* the game folder, so there is nothing to
  import beyond the generic folder scan.
- **yahd2mm (tairasoul, C#):** `%LOCALAPPDATA%\yahd2mm\` with `mods/`, `downloads/` and `nexus-ids.json`.
  Import it with the folder picker (generic scan). The formats of its state, choices and priority files weren't
  examined.
- Also seen, minor or unverified: modocracy, hd2-mod-manager-electron, GenericProgram/HD2MM. A repo called
  "Helldivers-2-Mod-Installer" has a spam-like description and was ignored.

## Could not confirm

- Arsenal builds after 0.36.2, including whether the Nexus-only builds changed `hd2a_data.json`.
- Whether Arsenal deletes the archive in `temp/` after installing it.
- The exact type of Arsenal's `updateTimestamp`. DDMM accepts seconds or an ISO string.
- How Vortex encodes its LevelDB values. Not read.
- The paths of Arsenal and Vortex under Proton. The user picks the folder.

## How the importer decides things

**What counts as a mod:**
- A folder is one mod when it has a `manifest.json`, or patch files within 5 levels and no subfolder with a
  `manifest.json` of its own. The second kind is a folder *of* mods.
- An archive is a mod when it has patch files or a `manifest.json`. Otherwise it is shown as "no Helldivers 2
  mod files" and left unticked.

**Already in DDMM.** An item counts as installed when any of these match:
- the same manifest GUID (for a manager-generated `LOCAL…` GUID, only when importing a folder);
- the same archive (size and SHA-256, recorded as `ImportedArchive` in the sidecar);
- the same archive file name as one DDMM installed, or as a mod folder's name;
- the same Nexus mod plus the same file id, file name or version.

The same Nexus mod with a different version and a matching file title is "another version is in DDMM", left
unticked.

**Within one scan:**
- Identical archives are duplicates. The original is kept and ` (1)`/` - Copy` is the duplicate.
- The same author GUID twice is also a duplicate.
- Several downloads of the same Nexus file (same mod id and same file title) keep only the newest ticked.
  Optional files of the same Nexus page have different titles and all stay.

**Import:**
- One mod at a time, through `install_from_archive_as` / `install_from_folder_as`, the normal install code with
  the same archive hardening.
- The data-folder move lock is held for the whole run.
- Free space is checked first: the unpacked size plus 256 MB.
- Cancel stops after the current mod, and removes it.
- Failures are collected into the report.
