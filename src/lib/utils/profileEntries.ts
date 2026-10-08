// Pure helpers for a profile's entries and the mods they point at. No
// imports but types, so the unit tests in tests/unit can run them with
// plain Node.
import type { Config } from "../models/profile";
import type { Manifest } from "../models/manifest";
import type { UUID } from "../types/uuid";

/** A new entry for a mod: on, every option on, first choice everywhere. */
export function defaultConfigFor(manifest: Manifest): Config {
    if (!("Version" in manifest)) {
        return { For: "Legacy", Guid: manifest.Guid, Enabled: true, Selected: 0 };
    }
    const len = manifest.Options?.length ?? 0;
    const shape = { Guid: manifest.Guid, Enabled: true, Toggled: new Array<boolean>(len).fill(true), Selected: new Array<number>(len).fill(0) };
    if (manifest.Version === 1) return { For: "V1", ...shape };
    if (manifest.Version === 2) return { For: "V2", ...shape };
    throw "Unknown manifest version!";
}

/** Whether an option has sub-options to choose from, i.e. whether the
 * config popup should show a dropdown for it. Manifests may declare
 * `"SubOptions": []` (or `null`), which has nothing to pick. */
export function hasSubOptionChoice(option: { readonly SubOptions?: readonly unknown[] | null }): boolean {
    return Array.isArray(option.SubOptions) && option.SubOptions.length > 0;
}

function inRange(i: unknown, count: number): boolean {
    return Number.isInteger(i) && (i as number) >= 0 && (i as number) < Math.max(1, count);
}

/** Whether an entry's option choices still fit the mod's manifest (its
 * version, number of options, and sub-option indexes). A mod that comes
 * back after being missing -- or was replaced by a different version --
 * may not match any more. */
export function configFits(config: Config, manifest: Manifest): boolean {
    if (!("Version" in manifest)) {
        return config.For === "Legacy" && inRange(config.Selected, manifest.Options?.length ?? 0);
    }
    const expected = manifest.Version === 1 ? "V1" : manifest.Version === 2 ? "V2" : null;
    if (config.For === "Legacy" || config.For !== expected) return false;
    const options = manifest.Options ?? [];
    return config.Toggled.length === options.length
        && config.Selected.length === options.length
        && options.every((o, i) => inRange(config.Selected[i], o.SubOptions?.length ?? 0));
}

/** An entry that fits the mod: `config` itself when it does, else the
 * default options (keeping whether it was on), with `reset` set so the
 * user can be told. */
export function fitConfig(config: Config, manifest: Manifest): { config: Config; reset: boolean } {
    if (configFits(config, manifest)) return { config, reset: false };
    return { config: { ...defaultConfigFor(manifest), Enabled: config.Enabled }, reset: true };
}

/** What a deploy of `configs` would actually use: the entries whose mod is
 * in the library, and how many are missing (deploy skips those). */
export function deployableEntries(configs: Config[], loadedGuids: Iterable<string>): { loaded: Config[]; missing: number } {
    const known = new Set(loadedGuids);
    const loaded = configs.filter(c => known.has(c.Guid));
    return { loaded, missing: configs.length - loaded.length };
}

/** Move the entries of a mod that changed its ID from `oldGuid` to
 * `newGuid`, in place, keeping on/off and position (a list that then has
 * the mod twice keeps the first entry); returns how many moved. Option
 * choices are left as they are: `fitConfig` resets the ones that no longer
 * fit. */
export function renameEntries(configs: Config[], oldGuid: UUID, newGuid: UUID): number {
    let moved = 0;
    for (const config of configs) {
        if (config.Guid.toLowerCase() === oldGuid.toLowerCase()) {
            config.Guid = newGuid;
            moved++;
        }
    }
    if (moved > 0) removeDuplicateEntries(configs);
    return moved;
}

/** Take every entry of the mod `guid` out of `configs`, in place (so a
 * list shown on the page updates too); returns how many there were. */
export function removeEntriesOf(configs: Config[], guid: string): number {
    let removed = 0;
    for (let i = configs.length - 1; i >= 0; i--) {
        if (configs[i].Guid === guid) {
            configs.splice(i, 1);
            removed++;
        }
    }
    return removed;
}

/** Take repeated entries of the same mod out of `configs`, in place,
 * keeping the first (highest in the list) one; returns the GUIDs that had
 * more than one entry. The Mods page lists a profile's entries keyed by
 * mod, so a repeat makes the list fail to show at all (issue #71). Before
 * 2.0.0-rc.10, editing a mod's options while the search box filtered the
 * list could save such a repeat, and it stays in profiles.json until
 * removed. */
export function removeDuplicateEntries(configs: Config[]): string[] {
    const seen = new Set<string>();
    const repeated: string[] = [];
    for (let i = 0; i < configs.length; i++) {
        const guid = configs[i].Guid.toLowerCase();
        if (!seen.has(guid)) {
            seen.add(guid);
            continue;
        }
        if (!repeated.includes(configs[i].Guid)) repeated.push(configs[i].Guid);
        configs.splice(i, 1);
        i--;
    }
    return repeated;
}
