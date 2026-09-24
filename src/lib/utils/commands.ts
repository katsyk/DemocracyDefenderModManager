import { invoke } from '@tauri-apps/api/core';
import * as log from '@tauri-apps/plugin-log';
import { Mod, type ResolvedSource } from '../models/mod';
import type { Config, ProfilesConfig } from '$lib/models/profile';
import type { Manifest } from '$lib/models/manifest';
import type { RustResult } from '$lib/types/results';
import type { UUID } from '$lib/types/uuid';
import type { Settings } from '$lib/models/settings';

type RawMod = { Manifest: Manifest, Directory: string, Sources?: ResolvedSource[] };
type RawInstalledMod = RawMod & { Warning?: string };

export type InstalledMod = { mod: Mod, warning?: string };

export function rawModToMod(raw: RawMod): Mod {
    return new Mod(raw.Manifest, raw.Directory, raw.Sources ?? []);
}

function toInstalledMod(raw: RawInstalledMod): InstalledMod {
    return { mod: rawModToMod(raw), warning: raw.Warning };
}

export async function getMods(): Promise<Mod[]> {
    log.debug("Invoking `get_mods`.");
    const mods = await invoke<RawMod[]>("get_mods");
    return mods.map(rawModToMod);
}

export async function deleteMod(guid: UUID): Promise<void> {
    log.debug("Invoking `delete_mod`.");
    await invoke<void>("delete_mod", { guid });
}

export async function addMod(archiveFile: string): Promise<InstalledMod> {
    log.debug("Invoking `add_mod`.");
    const mod = await invoke<RawInstalledMod>("add_mod", { archiveFile });
    return toInstalledMod(mod);
}

export async function addMods(archiveFiles: string[]): Promise<RustResult<InstalledMod>[]> {
    log.debug("Invoking `add_mods`.");
    const results = await invoke<RustResult<RawInstalledMod>[]>("add_mods", { archiveFiles });
    return results.map(result => {
        if ("Ok" in result) {
            return {
                Ok: toInstalledMod(result.Ok)
            };
        }
        return result;
    });
}

export async function addModFolder(folder: string): Promise<InstalledMod> {
    log.debug("Invoking `add_mod_folder`.");
    const mod = await invoke<RawInstalledMod>("add_mod_folder", { folder });
    return toInstalledMod(mod);
}

export async function addPaths(paths: string[]): Promise<RustResult<InstalledMod>[]> {
    log.debug("Invoking `add_paths`.");
    const results = await invoke<RustResult<RawInstalledMod>[]>("add_paths", { paths });
    return results.map(result => {
        if ("Ok" in result) {
            return {
                Ok: toInstalledMod(result.Ok)
            };
        }
        return result;
    });
}

export async function addModFromUrl(url: string): Promise<InstalledMod> {
    log.debug("Invoking `add_mod_from_url`.");
    const mod = await invoke<RawInstalledMod>("add_mod_from_url", { url });
    return toInstalledMod(mod);
}

export type UrlClassification = {
    Provider: string,
    DisplayName: string,
    RequiresHandoff: boolean
};

/** Classify a pasted URL before deciding whether to try a direct download
 * or go straight to a browser handoff. Never touches the network. */
export async function classifyDownloadUrl(url: string): Promise<UrlClassification> {
    log.debug("Invoking `classify_download_url`.");
    return await invoke<UrlClassification>("classify_download_url", { url });
}

export async function startHandoff(pageUrl: string, existingGuid?: UUID): Promise<void> {
    log.debug("Invoking `start_handoff`.");
    await invoke<void>("start_handoff", { pageUrl, existingGuid: existingGuid ?? null });
}

export async function cancelHandoff(): Promise<void> {
    log.debug("Invoking `cancel_handoff`.");
    await invoke<void>("cancel_handoff");
}

export async function installHandoffFile(file: string, pageUrl: string, existingGuid?: UUID): Promise<InstalledMod> {
    log.debug("Invoking `install_handoff_file`.");
    const mod = await invoke<RawInstalledMod>("install_handoff_file", { file, pageUrl, existingGuid: existingGuid ?? null });
    return toInstalledMod(mod);
}

export async function loadProfiles(): Promise<ProfilesConfig> {
    log.debug("Invoking `load_profiles`.");
    return await invoke<ProfilesConfig>("load_profiles");
}

export async function saveProfiles(config: ProfilesConfig): Promise<void> {
    log.debug("Invoking `save_profiles`.");
    await invoke<void>("save_profiles", { config });
}

export async function loadSettings(): Promise<Settings> {
    log.debug("Invoking `load_settings`.");
    return await invoke<Settings>("load_settings");
}

export async function saveSettings(settings: Settings): Promise<void> {
    log.debug("Invoking `save_settings`.");
    await invoke<void>("save_settings", { settings });
}

export async function checkSettings(): Promise<boolean> {
    log.debug("Invoking `check_settings`.");
    return await invoke<boolean>("check_settings");
}

export async function deploy(configs: Config[]): Promise<void> {
    log.debug("Invoking `deploy`.");
    await invoke<void>("deploy", { configs });
}

export async function purge(): Promise<void> {
    log.debug("Invoking `purge`.");
    await invoke<void>("purge");
}