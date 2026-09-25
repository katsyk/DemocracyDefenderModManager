import { invoke as tauriInvoke, type InvokeArgs } from '@tauri-apps/api/core';
import * as log from '@tauri-apps/plugin-log';
import { Mod, type ResolvedSource } from '../models/mod';
import type { Config, ProfilesConfig } from '$lib/models/profile';
import type { Manifest } from '$lib/models/manifest';
import type { RustResult } from '$lib/types/results';
import type { UUID } from '$lib/types/uuid';
import type { Settings } from '$lib/models/settings';

/** `invoke`, but every failure is also written to the log file with the
 * command's name, so a user's log shows *why* something failed even if
 * they only saw (or closed) the popup. */
async function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
    try {
        return await tauriInvoke<T>(cmd, args);
    } catch (ex: unknown) {
        const message = typeof ex === "string" ? ex : ex instanceof Error ? ex.message : JSON.stringify(ex);
        log.error(`\`${cmd}\` failed: ${message}`).catch(() => {});
        throw ex;
    }
}

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

export type UpdateStateKind =
    | "UpToDate" | "UpdateAvailable" | "Skipped" | "Unknown" | "NeedsApiKey" | "Unsupported" | "Error";
export type UpdateState = { Kind: UpdateStateKind, message?: string };
export type UpdateMethod = "Direct" | "Browser";

/** A file a one-click update can download directly (GitHub release asset,
 * GameBanana/ModWorkshop file). */
export type UpdateFile = {
    Id: string,
    Name: string,
    Label?: string,
    Size?: number,
    UploadedAt?: number,
    Url: string
};

export type UpdateStatusEntry = {
    Guid: UUID,
    Provider: string,
    DisplayName: string,
    SourceId?: string,
    InstalledVersion?: string,
    LatestVersion?: string,
    LatestFileName?: string,
    Status: UpdateState,
    PageUrl?: string,
    Method?: UpdateMethod,
    Files?: UpdateFile[],
    PreselectedFile?: string
};

export type CheckTrigger = "Manual" | "Startup" | "Scheduled";

export type UpdateCheckReport = {
    Trigger: CheckTrigger,
    CheckedAt: number,
    Results: UpdateStatusEntry[],
    NexusRateLimit?: { HourlyRemaining?: number, DailyRemaining?: number }
};

/** Runs an update check now. Called when the user clicks "Check for
 * Updates"; automatic checks (opt-in, off by default) run on the Rust side
 * and arrive as the `updates://checked` event. */
export async function checkUpdates(): Promise<UpdateCheckReport> {
    log.debug("Invoking `check_updates`.");
    return await invoke<UpdateCheckReport>("check_updates");
}

/** The latest check's results from this session, if any. */
export async function getLastUpdateReport(): Promise<UpdateCheckReport | null> {
    return await invoke<UpdateCheckReport | null>("get_last_update_report");
}

/** "Skip this version" (a version) or stop skipping (null). */
export async function skipUpdateVersion(guid: UUID, provider: string, version: string | null): Promise<void> {
    log.debug("Invoking `skip_update_version`.");
    await invoke<void>("skip_update_version", { guid, provider, version });
}

/** One-click in-place update from a public direct download. Progress comes
 * through the `updates://progress` event. */
export async function updateModDirect(guid: UUID, provider: string, file: UpdateFile, version: string | null): Promise<InstalledMod> {
    log.debug("Invoking `update_mod_direct`.");
    const mod = await invoke<RawInstalledMod>("update_mod_direct", { guid, provider, file, version });
    return toInstalledMod(mod);
}

/** Whether the browser extension has talked to DDMM recently. */
export async function browserExtensionActive(): Promise<boolean> {
    return await invoke<boolean>("browser_extension_active");
}

export type NexusKeyStatus = {
    Present: boolean,
    Storage?: "Keychain" | "File",
    Username?: string,
    IsPremium?: boolean
};

export async function getNexusKeyStatus(): Promise<NexusKeyStatus> {
    return await invoke<NexusKeyStatus>("get_nexus_key_status");
}

/** Validates the key with Nexus Mods and, if accepted, stores it in the OS
 * keychain. The key is never logged or kept in settings. */
export async function setNexusApiKey(key: string): Promise<NexusKeyStatus> {
    log.debug("Invoking `set_nexus_api_key`.");
    return await invoke<NexusKeyStatus>("set_nexus_api_key", { key });
}

export async function removeNexusApiKey(): Promise<NexusKeyStatus> {
    log.debug("Invoking `remove_nexus_api_key`.");
    return await invoke<NexusKeyStatus>("remove_nexus_api_key");
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

/** The directory DDMM is currently keeping mods/settings/profiles/logs in. */
export async function getDataDir(): Promise<string> {
    log.debug("Invoking `get_data_dir`.");
    return await invoke<string>("get_data_dir");
}

/** Result of checking a game path on the backend (see `game_path.rs`). */
export type GamePathReport = {
    valid: boolean;
    /** The real game root, when valid -- may differ from the input if a
     * folder above/below it was picked. */
    resolvedPath: string | null;
    /** Why it's invalid: "empty" | "not_found" | "not_a_directory" |
     * "portal_path" | "unreadable" | "missing_tools" | "missing_data" |
     * "missing_bin" | "missing_exe". */
    code: string | null;
    detail: string | null;
    message: string | null;
};

/** Check a game path and get the specific reason it's invalid, if it is. */
export async function validateGamePath(path: string): Promise<GamePathReport> {
    return await invoke<GamePathReport>("validate_game_path", { path });
}

/** Look for a Helldivers 2 install via Steam, without touching settings. */
export async function detectGamePath(): Promise<string | null> {
    log.debug("Invoking `detect_game_path`.");
    return await invoke<string | null>("detect_game_path");
}

/** Detect a Helldivers 2 install and, if found, save it as the game path
 * immediately. Returns the detected path, if any. */
export async function autoDetectAndSaveGamePath(): Promise<string | null> {
    log.debug("Invoking `auto_detect_and_save_game_path`.");
    return await invoke<string | null>("auto_detect_and_save_game_path");
}

export async function deploy(configs: Config[]): Promise<void> {
    log.debug("Invoking `deploy`.");
    await invoke<void>("deploy", { configs });
}

export async function purge(): Promise<void> {
    log.debug("Invoking `purge`.");
    await invoke<void>("purge");
}

// --- Browser bridge (one-click install) ---------------------------------

export type BridgeConsentDecision = "AlwaysAllow" | "JustOnce" | "Deny";

/** Tells the backend whether this page can handle bridge events right now
 * (listeners registered, profiles loaded). Browser installs wait for this
 * before asking for consent or running the afterInstall step. */
export async function setBridgeFrontendReady(ready: boolean): Promise<void> {
    await invoke<void>("set_bridge_frontend_ready", { ready });
}

/** Answers a pending `bridge://consent-request` -- see BridgeConsentPopup. */
export async function resolveBridgeConsent(requestId: string, decision: BridgeConsentDecision): Promise<void> {
    log.debug("Invoking `resolve_bridge_consent`.");
    await invoke<void>("resolve_bridge_consent", { requestId, decision });
}

export type BridgeSoftError = { code: "GAME_NOT_FOUND" | "DEPLOY_FAILED", message: string };

/** Answers a pending `bridge://mod-installed` -- reports what the frontend
 * actually did (or couldn't do) for the mod's `afterInstall` step. */
export async function resolveBridgeInstallCompletion(
    requestId: string,
    addedToProfile: string | undefined,
    deployed: boolean,
    warnings: string[],
    softError?: BridgeSoftError,
): Promise<void> {
    log.debug("Invoking `resolve_bridge_install_completion`.");
    await invoke<void>("resolve_bridge_install_completion", {
        requestId,
        addedToProfile: addedToProfile ?? null,
        deployed,
        warnings,
        softErrorCode: softError?.code ?? null,
        softErrorMessage: softError?.message ?? null,
    });
}

/** Registrable domains ("Always allow") the user has approved for
 * browser-triggered installs -- revocable in Settings. */
export async function getBridgeAllowedSites(): Promise<string[]> {
    log.debug("Invoking `get_bridge_allowed_sites`.");
    return await invoke<string[]>("get_bridge_allowed_sites");
}

export async function revokeBridgeSite(site: string): Promise<void> {
    log.debug("Invoking `revoke_bridge_site`.");
    await invoke<void>("revoke_bridge_site", { site });
}

export type BrowserIntegrationStatus = {
    browserId: string,
    displayName: string,
    registered: boolean,
    detail: string,
};

type RawBrowserIntegrationStatus = { browser_id: string, display_name: string, registered: boolean, detail: string };

function toBrowserIntegrationStatus(raw: RawBrowserIntegrationStatus): BrowserIntegrationStatus {
    return { browserId: raw.browser_id, displayName: raw.display_name, registered: raw.registered, detail: raw.detail };
}

/** Re-registers (idempotent) every browser's native-messaging manifest and
 * reports per-browser status. Used both at startup and as Settings'
 * "Repair all" button. */
export async function repairBrowserIntegration(): Promise<BrowserIntegrationStatus[]> {
    log.debug("Invoking `repair_browser_integration`.");
    const raw = await invoke<RawBrowserIntegrationStatus[]>("repair_browser_integration");
    return raw.map(toBrowserIntegrationStatus);
}

export async function removeBrowserIntegration(): Promise<void> {
    log.debug("Invoking `remove_browser_integration`.");
    await invoke<void>("remove_browser_integration");
}

export async function repairBrowserIntegrationOne(browserId: string): Promise<BrowserIntegrationStatus | null> {
    log.debug("Invoking `repair_browser_integration_one`.");
    const raw = await invoke<RawBrowserIntegrationStatus | null>("repair_browser_integration_one", { browserId });
    return raw ? toBrowserIntegrationStatus(raw) : null;
}

export async function removeBrowserIntegrationOne(browserId: string): Promise<void> {
    log.debug("Invoking `remove_browser_integration_one`.");
    await invoke<void>("remove_browser_integration_one", { browserId });
}

/** Cheap check for whether Helldivers 2 is currently running -- used to
 * skip deploying a browser-installed mod into a running game. */
export async function isGameRunning(): Promise<boolean> {
    log.debug("Invoking `is_game_running`.");
    return await invoke<boolean>("is_game_running");
}

/** Last-resort way to close the app: tells the Rust side to exit the
 * process directly, bypassing the window's own `destroy` IPC call. Only
 * used as a fallback if that call fails. */
export async function forceExit(): Promise<void> {
    log.debug("Invoking `force_exit`.");
    await invoke<void>("force_exit");
}

/** Tells the Rust-side close watchdog (see `lib.rs`) that the frontend is
 * alive and handling a close request, so it won't force-exit from under
 * us. Call this as the very first thing the close handler does. */
export async function ackCloseRequested(): Promise<void> {
    await invoke<void>("ack_close_requested");
}