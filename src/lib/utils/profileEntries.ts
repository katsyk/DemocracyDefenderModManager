// Pure helpers for a profile's entries and the mods they point at. No
// imports but types, so the unit tests in tests/unit can run them with
// plain Node.
import type { Config } from "../models/profile";
import type { Manifest } from "../models/manifest";

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
