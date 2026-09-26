# Import: other mod managers' on-disk formats (maintainer note)

Not published. This note sits next to the code, not in `docs/`, because it names other tools. The UI and the
user docs (`docs/using/importing-mods.md`) never do: they only say "another mod manager". Code comments may
name Arsenal and Vortex. The older HD2 mod manager is never named anywhere, including code, tests and
fixtures. Its on-disk folder names appear only as string literals the importer has to detect.

This covers `mod_import.rs`, `mod_import/layouts.rs` and `commands/import.rs`. Researched in September 2026.

**Only file-format knowledge is used.** DDMM reads the data files these tools leave on disk. No code from any
of them was copied into DDMM.

None of these formats is a published contract, so every parser is tolerant: an unknown or missing field is
skipped, never an error, and the generic folder scan always works as the fallback.

## Design constraints

- **Files on disk only.** Nothing is downloaded, from Nexus Mods or anywhere else. The Nexus API is never used
  for downloads, and Nexus's "Slow download" page and its timer are never automated or skipped.
- **The source is read-only.** DDMM never writes, moves or deletes anything there. `ensure_source_allowed`
  refuses a source that is, contains or is inside DDMM's data folder, and the check runs again for every item.
- **Nexus metadata** comes from what the source recorded, or from the download's file name. No API call is made
  during import. Later update checks, with the user's optional key or sign-in, work from what was recorded.

## HD2 Arsenal

Confidence: **high for 0.36.2**. This section is documented from the data files written by the publicly
distributed 0.36.2 Linux package (a .deb in `leguteape/hd2arsenal-release`, 2026-06-26). Later, Nexus-only
builds were not checked.

**Data folder:**

- Windows: `%LOCALAPPDATA%\hd2arsenal\`. That is Electron's appData with "Roaming" swapped for "Local".
  [CurrCa7/Helldivers2-Arsenal-Download-Accelerator](https://github.com/CurrCa7/Helldivers2-Arsenal-Download-Accelerator)
  uses the same path.
- Linux (native build): `~/.config/hd2arsenal/`.
- Subfolders are `mods/`, `temp/`, `logs/` and `profile_images/`. The mods folder can be overridden with
  `userModsDir`, and the library stores absolute paths.

**`hd2a_data.json`** is the database. It keeps `.bak` copies. This is library format 1
(`librarySystemVersion: 1`):

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

- **Nexus data.** `nexusData` is set for mods installed from Nexus through Arsenal and is `null` for local
  installs. `updateTimestamp` is the file's Nexus upload time.
- **Per profile.** `modsList[selectedProfile].mods` holds the on/off state, the load order and the options:
  - Array order is the deploy order, reversed when `setTopPriority` is true.
  - Separators (`type: "separator"`) are skipped.
  - `optionsConfig` is matched to the library entry's options **by name**.
- **Older format.** Before format 1, each profile's `mods[]` held full mod objects that include `path`. DDMM
  falls back to reading those.
- **Skipped:** icon paths (they point into Arsenal's own cache), `mod_headers.db` (SQLite, used only for
  conflict detection) and `temp/` (downloads in progress).

**What DDMM does with it:**

- When a picked folder, or its parent, has `hd2a_data.json`, the mod list comes from `modsLibrary` and the
  folder isn't walked.
- A mod folder without its own `manifest.json` gets a v1 manifest built from Arsenal's label, description and
  options. It keeps Arsenal's `uuid` as its GUID, so a second import recognizes it.
- `nexusData` is recorded in `.hd2mm-origin.json`, and on/off state, order and options become the profile
  entry.

## The older HD2 mod manager (1.x / 2024 original / 2.0 rewrite)

This section is documented from its publicly available source and the files it writes. It uses the same
manifest format DDMM reads.

**1.x (end of life).**

- `settings.json` sits next to its exe. Its `StorageDirectory` defaults to
  `%LOCALAPPDATA%\<its folder name>` (the literal is in `mod_import::candidate_sources`).
- Mods live in `<Storage>\Mods\<manifest Name>\` with their `manifest.json`. A manifest it generated for an
  archive without one has the archive name as `Name`, a new GUID, and the first-level folders as `Options`.
- `<Storage>\enabled.json` is an array in load order:
  `[{"Guid": "...", "Enabled": true, "Toggled": [true, false], "Selected": [0, 2]}]`. For a legacy manifest,
  `Selected` has one element.
- It stores no Nexus ids.

**2024 original.**

- Data lives in `%APPDATA%\<its short folder name>\`, with `Mods\<archive name>\manifest.json`.
- `enabled.json` is a map `{"<guid>": optionIndex}` that lists only the enabled mods.

**2.0 rewrite.**

- Everything is next to its exe, which can't be auto-detected, so the user picks the folder.
- Mods are in `mods/<archive name>/manifest.json`.
- `profiles.json` has the same format DDMM uses, with `Configs` in load order.
- Manifests can carry `NexusData.ModId`, and v1 manifests also `Version`.

**What DDMM does with them:**

- It auto-detects the 1.x and 2024 storage folders.
- Any folder of mod folders works anyway, since each has a `manifest.json`.
- `profiles.json` or `enabled.json` in the folder, or in its parent, supplies on/off state, order and options by
  GUID.
- The mod's own manifest, and so its GUID, is copied with it, so the profile entries fit unchanged.

## Vortex (with the Helldivers 2 extension)

Confidence: high for the paths and file formats. The state database encoding was not verified.

Documented from the public sources of [Vortex](https://github.com/Nexus-Mods/Vortex) and of ChemBoy1's HD2
extension (`game-helldivers2`, [Nexus site mod 845](https://www.nexusmods.com/site/mods/845)), and from the
files they write.

**Layout:**

- **Staging** defaults to `%APPDATA%\Vortex\helldivers2\mods`, with one folder per mod named after the archive
  (so it keeps the Nexus-style name) and a `__vortex_staging_folder` marker. Only the options picked at install
  time are staged.
- **Downloads** default to `%APPDATA%\Vortex\downloads\helldivers2`, under their Nexus file names.
- **Patch order.** The extension writes `<profileId>_patch_order.json` next to the staging folder. It is an array
  of `{id, modId (staging folder name), name, enabled, locked, data}`.
- **Nexus ids.** The mod id, file id and version are otherwise only in the LevelDB state database
  (`%APPDATA%\Vortex\state.v2`). DDMM doesn't read it: the encoding is unverified, and the folder and file names
  already carry the id and version.

**What DDMM does with it:**

- It auto-detects both folders.
- In the staging folder, each mod folder is one item. The Nexus id and version come from the folder name, and
  order and on/off state from the newest `*_patch_order.json`.
- Mods already imported from staging show as "already in DDMM" when the downloads folder is scanned.
- Under Wine/Proton the folders are inside a prefix, so the user picks them.

## Nexus Mods download file names

Confidence: high. See `sources::parse_nexus_archive_name`.

- **Until 11 June 2026:** `<name>-<modId>-<version, dots as dashes>-<unix upload time>.<ext>`.
- **Since 11 June 2026:** `<name> <modId> <version> <YYYY-MM-DDTHH-MMZ> <slug>.<ext>`, space-separated. For
  example `PawnCompanion 1377 1.35 2026-06-24T03-45Z G8alq8bQH.zip`.
  - Names can contain spaces and version-like parts, so the name is parsed from the right.
  - Nexus says the name is not a contract. Source:
    [Changes to downloaded file names](https://forums.nexusmods.com/topic/13539100-changes-to-downloaded-file-names/).
  - The time has minute precision, while Nexus update matching compares exact upload times. So a new-style name
    records the mod id and version but no `uploaded_at`.
- A browser's ` (1)` suffix is ignored.

## Could not confirm

- Arsenal builds after 0.36.2.
- Whether Arsenal deletes its `temp/` archive after installing.
- The exact type of `updateTimestamp`: seconds and ISO strings are both accepted.
- The Vortex LevelDB value encoding. It isn't read.
- Arsenal's and Vortex's paths under Proton.

## How the importer decides things

**What counts as a mod.**

- A folder is one mod when it has a `manifest.json`, or when it has patch files within 5 levels and no
  subfolder with its own `manifest.json`. The latter case is a folder *of* mods.
- An archive is a mod when it contains patch files or a `manifest.json`.

**Already in DDMM.** Any of these counts as a match:

- the same manifest GUID (a manager-generated `LOCAL…` GUID only counts for folders);
- the same archive (size plus SHA-256, recorded as `ImportedArchive`);
- the same archive file name, or mod folder name, as something DDMM installed;
- the same Nexus mod with the same file id, file name or version.

When the installed copy has **no Nexus link** and the item does, the item shows "Already in DDMM: will add
Nexus info" and is ticked. Importing it then writes only the Nexus source and file into that mod's
`.hd2mm-origin.json`. Files, manifest, options, order and on/off state are left alone.

**Within one scan.**

- Identical archives are duplicates: the original stays and ` (1)` / ` - Copy` is marked.
- The same author GUID twice is also a duplicate.
- Several downloads of the same Nexus file (same mod id and file title) keep only the newest ticked.

**Import.**

- One mod at a time, through the normal install code.
- The data-folder move lock is held throughout.
- Free space is checked first (unpacked size + 256 MB).
- Cancel removes the mod in flight.
- Failures go into the report.
