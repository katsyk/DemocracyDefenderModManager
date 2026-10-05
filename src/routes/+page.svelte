<script lang="ts">
    import { SvelteMap } from "svelte/reactivity";
    import { Plus, Dash, Backspace, ArrowBarRight, ArrowBarLeft, Arrow90degLeft, ArrowReturnLeft, PencilSquare, Download, ThreeDotsVertical, CaretUpFill, CaretDownFill, Trash3, ArrowBarUp, ArrowBarDown, CaretUp, CaretDown, Eraser, FolderPlus, Link45deg, BoxArrowUpRight, ArrowRepeat, CloudArrowDownFill, GripVertical, InfoCircle, SkipForward, ArrowCounterclockwise, Key, BoxArrowInDown } from "svelte-bootstrap-icons";
    import { getCurrentWindow } from "@tauri-apps/api/window";
    import { getCurrentWebview } from "@tauri-apps/api/webview";
    import { listen } from "@tauri-apps/api/event";
    import { openUrl } from "@tauri-apps/plugin-opener";
    import { open } from "@tauri-apps/plugin-dialog";
    import * as log from "@tauri-apps/plugin-log";
    import { SortableList } from "@rodrigodagostino/svelte-sortable-list"
    import { useLocalization } from "$lib/state/localization.svelte";
    import { Mod } from "$lib/models/mod";
    import type {Config, Profile, ProfilesConfig} from "$lib/models/profile";
    import { defaultConfigFor, deployableEntries, fitConfig, removeDuplicateEntries, removeEntriesOf } from "$lib/utils/profileEntries";
    import {
        addMod, addMods, addModFolder, addPaths, addModFromUrl, deleteMod, getMods, loadProfiles, saveProfiles,
        loadSettings, deploy, purge, checkSettings, classifyDownloadUrl, checkUpdates, autoDetectAndSaveGamePath,
        resolveBridgeConsent, resolveBridgeInstallCompletion, isGameRunning, forceExit, ackCloseRequested,
        setBridgeFrontendReady, getLastUpdateReport, skipUpdateVersion, browserExtensionActive,
        detectImportSources, takePendingDeepLinks, type ImportSource,
        type UpdateStatusEntry, type UpdateCheckReport, type BridgeConsentDecision, type BridgeSoftError
    } from "$lib/utils/commands";
    import type { UUID } from "$lib/types/uuid";
    import { usePopup } from "$lib/state/popup.svelte";
    import { useToast } from "$lib/state/toast.svelte";
    import {
        ConfirmPopup,
        InputPopup,
        WaitPopup,
        NotificationPopup,
        ErrorPopup,
        AddResultPopup,
        ModConfigPopup,
        HandoffPopup,
        BridgeConsentPopup,
        AutoImportPopup,
        UpdatesPopup,
        UpdateFilePickPopup,
        UpdateDownloadPopup,
        BrowserUpdatePopup,
        ImportPopup
    } from "$lib/types/popup";
    import type { HandoffResult } from "$lib/types/popup";
    import type { AfterBrowserInstall } from "$lib/models/settings";
    import ToggleSwitch from "$lib/components/ToggleSwitch.svelte";
    import PopupMenuButton from "$lib/components/PopupMenuButton.svelte";
    import { goto, onNavigate } from "$app/navigation";
    import type { ModAddResult } from "$lib/types/results";
    import { onMount } from "svelte";
    import Select from "$lib/components/Select.svelte";
    import ModsLoadError from "$lib/components/ModsLoadError.svelte";
    import { FALLBACK_MOD_IMAGE, useFallbackImage } from "$lib/utils/modImages";

    const { t } = useLocalization();
    const popups = usePopup();
    const { show: showPopup } = popups;
    const { show: showToast } = useToast();
    const appWindow = getCurrentWindow();

    /** How long to wait for the on-close profile save before giving up and
     * asking the user whether to close anyway. */
    const CLOSE_SAVE_TIMEOUT_MS = 5000;

    let mods = $state<Mod[]>([]);
    let profiles = $state<Profile[]>([]);
    let activeProfile = $state<number>(0);
    let searchText = $state<string>("");
    let profileConfigs = $state<Config[]>([]);
    let iconPaths = new SvelteMap<UUID, string | null>();
    let libraryExtended = $state<boolean>(false);
    let libraryVisible = $state<boolean>(false);
    let isDragging = $state<boolean>(false);
    let initPromise = $state<Promise<void>>();
    let downloadsPath = $state<string>("");
    let updateStatuses = $state<UpdateStatusEntry[]>([]);
    /** What happens after a mod lands (Settings); also decides whether an
     * update redeploys by itself or asks first. */
    let afterBrowserInstall = $state<AfterBrowserInstall>("deploy");
    /** Mods (not sources) with an update available and not skipped. */
    let updatesAvailableCount = $derived(
        new Set(updateStatuses.filter(u => u.Status.Kind === "UpdateAvailable").map(u => u.Guid)).size
    );
    /** True once `init()` has actually loaded profiles from disk into
     * `profiles`. Closing before this is true must not try to save --
     * there's nothing loaded yet to save, and `currentProfile` is
     * undefined, which would make `doSaveProfiles` fail every time. */
    let profilesLoaded = $state<boolean>(false);
    /** True while a deploy is in flight. Closing mid-deploy can leave the
     * game's data directory half-written, so the close handler asks for
     * confirmation instead of either hanging or silently interrupting it. */
    let deploying = $state<boolean>(false);
    /** Guards against re-entrant close requests (e.g. the window manager's
     * close and a stray second click both firing before the first request
     * has been resolved). */
    let closeRequestInFlight = false;
    /** Set when this page is left; queued `ddmm://` links then stay queued
     * until it's shown again. */
    let unmounted = false;
    /** How many "Import mods" popups are open (normally 0 or 1). */
    let importWindows = $state(0);
    /** True while an "Import mods" popup is open (an import may be
     * running in it); closing then asks first, like during a deploy. */
    let importing = $derived(importWindows > 0);
    /** Other mod managers' folders (or Downloads) with mods in them, shown
     * on the empty mod list as a way in. */
    let importSources = $state<ImportSource[]>([]);

    let currentProfile = $derived<Profile | undefined>(profiles[activeProfile]);
    /** The profile's entries as shown: each config, its mod (`undefined`
     * when that mod isn't in the library -- deleted, or its manifest.json
     * can't be read -- which keeps its place in the load order), and its
     * index in `profileConfigs` (which the list's own index isn't while
     * searching). */
    let profileEntries = $derived<[Config, Mod | undefined, number][]>(
        profileConfigs
            .map((config, i) => [config, mods.find(m => m.guid === config.Guid), i] as [Config, Mod | undefined, number])
            .filter(([_, mod]) =>
                searchText.length === 0 ||
                (mod !== undefined && [mod.name, mod.description].some(field =>
                    field.toLowerCase().includes(searchText.toLowerCase())
                ))
            )
    );
    let enableRemoveProfile = $derived<boolean>(profiles.length > 1);
    let enableClearSearch = $derived<boolean>(searchText.length > 1);
    let libraryMods = $derived<Mod[]>(
        mods.filter((m) => !profileConfigs.some(config => config.Guid === m.guid)),
    );
    let libraryEnabled = $derived<boolean>(searchText.length === 0);
    let allowReorder = $derived<boolean>(searchText.length === 0);
    
    $effect(() => {
        if (!currentProfile) return;

        switch (currentProfile.Version) {
            case "V1":
                profileConfigs = currentProfile.Configs;
                break;
        }

        return applyCurrentConfigChanges;
    });

    $effect(() => {
        for (const mod of mods) {
            if (iconPaths.has(mod.guid)) continue;
            mod.iconPath()
                .then(path => iconPaths.set(mod.guid, path ?? null))
                .catch(() => iconPaths.set(mod.guid, null));
        }
    });

    onMount(() => {
        // A failed init shows the error (the `{:catch}` below), and says
        // why in the log file.
        initPromise = init().catch((ex: unknown) => {
            log.error(`The Mods page couldn't load: ${describeError(ex)}`);
            throw ex;
        });

        const unlisten = appWindow.onCloseRequested(async (event) => {
            // We take full control of closing here rather than letting the
            // `@tauri-apps/api` wrapper call `destroy()` on our behalf
            // afterwards: that call isn't guarded, and if it ever fails
            // (wrong permission, IPC hiccup, ...) the rejection goes
            // nowhere and the window is simply stuck open with no way out
            // for the user. `closeWindow()` below does the same thing but
            // with a fallback.
            event.preventDefault();
            log.info("Close requested.");
            try {
                // Tell the Rust-side watchdog we're alive and on it, so it
                // doesn't force-exit out from under a legitimate save or
                // confirmation popup. See `ackCloseRequested`'s doc comment.
                await ackCloseRequested();
            } catch (ex: unknown) {
                log.error(`Failed to acknowledge close request: ${errorMessage(ex)}`);
            }

            if (closeRequestInFlight) {
                log.info("Close already in progress; ignoring re-entrant request.");
                return;
            }
            closeRequestInFlight = true;
            try {
                await handleCloseRequest();
            } finally {
                closeRequestInFlight = false;
            }
        });

        const unlistenDragDrop = getCurrentWebview().onDragDropEvent(async (e) => {
            switch (e.payload.type) {
                case "enter":
                case "over":
                    isDragging = true;
                    break;
                case "drop":
                    isDragging = false;
                    // A window is open (an import wizard, a question, ...):
                    // a second import or add started under it would get in
                    // its way, so the drop waits for the user to finish.
                    if (popups.isShown) {
                        log.info(`Ignored a drop of ${e.payload.paths.length} file(s): a window is open.`);
                        showToast("warning", t("toast.drop_while_busy"));
                        break;
                    }
                    if (e.payload.paths.length >= BULK_ADD_THRESHOLD) {
                        await onImport(e.payload.paths);
                    } else {
                        await doAddPaths(...e.payload.paths);
                    }
                    break;
                case "leave":
                    isDragging = false;
                    break;
            }
        });

        // Browser bridge: the backend pushes these when a browser install
        // needs a consent decision or has finished (see bridge::server on
        // the Rust side). See docs/development/bridge-protocol.md.
        const unlistenBridgeConsent = listen<{ requestId: string; site: string | null; fileName?: string }>(
            "bridge://consent-request",
            (e) => onBridgeConsentRequest(e.payload),
        );
        const unlistenBridgeInstalled = listen<BridgeModInstalledPayload>(
            "bridge://mod-installed",
            (e) => onBridgeModInstalled(e.payload),
        );
        // ddmm:// deep links (see deep_link.rs on the Rust side): the
        // backend queues them and says so; this page takes them once it's
        // ready (below) and whenever a new one is queued.
        const unlistenDeepLinkPending = listen(
            "deep-link://pending",
            () => showPendingDeepLinks(),
        );
        // Auto-import from Downloads (see auto_import.rs on the Rust side;
        // opt-in, off by default).
        const unlistenAutoImport = listen<{ file: string }>(
            "auto-import://candidate",
            (e) => onAutoImportCandidate(e.payload),
        );
        // Results of an automatic update check (opt-in in Settings, off by
        // default) -- see commands::updates::spawn_auto_check.
        const unlistenUpdatesChecked = listen<UpdateCheckReport>(
            "updates://checked",
            (e) => onUpdatesChecked(e.payload),
        );

        // Only now can a browser install's consent prompt / afterInstall
        // step actually be handled; the backend holds them until then (on a
        // cold start the install arrives before this page has loaded).
        Promise.all([initPromise, unlistenBridgeConsent, unlistenBridgeInstalled])
            .then(() => {
                if (profilesLoaded) return setBridgeFrontendReady(true);
            })
            .catch((ex: unknown) => log.error(`Failed to mark the bridge frontend ready: ${errorMessage(ex)}`));

        // Links that arrived before this page could show them: the one DDMM
        // was started with (issue #32), or any that came in while another
        // page or the data-folder recovery screen was shown.
        unlistenDeepLinkPending
            .then(() => showPendingDeepLinks())
            .catch((ex: unknown) => log.error(`Failed to listen for ddmm:// links: ${errorMessage(ex)}`));

        return () => {
            unmounted = true;
            setBridgeFrontendReady(false).catch(() => {});
            unlisten.then(f => f());
            unlistenDragDrop.then(f => f());
            unlistenBridgeConsent.then(f => f());
            unlistenBridgeInstalled.then(f => f());
            unlistenDeepLinkPending.then(f => f());
            unlistenAutoImport.then(f => f());
            unlistenUpdatesChecked.then(f => f());
        };
    });

    onNavigate(async () => {
        if (!profilesLoaded) return;

        const result = await doSaveProfiles();
        if (!result.ok) log.warn(`Failed to save profiles on navigation: ${result.error}`);
    });

    async function init() {
        log.info("Initializing...");

        if (!await checkSettings()) {
            log.info("Settings invalid or missing; trying to auto-detect Helldivers 2...");
            const detected = await autoDetectAndSaveGamePath();
            if (detected) {
                log.info(`Auto-detected Helldivers 2 at ${detected}.`);
                showPopup(new NotificationPopup(
                    "info",
                    t("pages.mods.popup.notification.game_path_detected.message", { path: detected }),
                ));
            } else {
                showPopup(new NotificationPopup(
                    "warning",
                    t("pages.mods.popup.notification.game_path_not_found.message"),
                ));
                goto("/settings");
                return;
            }
        }

        const [loadedMods, loadedConfig]: [Mod[], ProfilesConfig] = await Promise.all([
            getMods(),
            loadProfiles()
        ]);

        // Entries whose mod isn't in the library (deleted by hand, or its
        // manifest.json can't be read right now) are kept, shown as
        // missing, so a mod that comes back gets its old place in the load
        // order. Deploy skips them.
        const missing = loadedConfig.Profiles
            .flatMap(p => p.Configs)
            .filter(c => !loadedMods.some(mod => mod.guid === c.Guid)).length;
        if (missing > 0) log.warn(`${missing} profile entr(ies) refer to mods that couldn't be loaded; keeping them.`);
        // A mod listed twice in a profile (saved by older versions, see
        // `removeDuplicateEntries`) would keep the list from showing at all.
        const repeated = removeRepeatedEntries(loadedConfig.Profiles, loadedMods);
        const resetOptions = fitProfilesToMods(loadedConfig.Profiles, loadedMods);
        if (resetOptions.length > 0) {
            log.warn(`Options reset to defaults (they no longer fit the mod): ${resetOptions.join(", ")}`);
            showPopup(new NotificationPopup("warning", t("pages.mods.popup.notification.options_reset.message", { names: resetOptions.join(", ") })));
        }

        mods = loadedMods;
        profiles = loadedConfig.Profiles;
        activeProfile = loadedConfig.Active;
        profilesLoaded = true;

        if (repeated.length > 0) {
            // Saved at once, so the repaired list is what's on disk even if
            // DDMM isn't closed normally.
            try {
                await saveProfiles({ Profiles: profiles, Active: activeProfile });
            } catch (ex: unknown) {
                log.warn(`Couldn't save the profiles after removing repeated entries: ${errorMessage(ex)}`);
            }
        }

        const settings = await loadSettings();
        if (settings.Version === "V1") {
            downloadsPath = settings.DownloadsPath;
            afterBrowserInstall = settings.AfterBrowserInstall;
        }

        // A check may already have run this session (at startup, if the user
        // turned that on, or before navigating away and back).
        try {
            const last = await getLastUpdateReport();
            if (last) updateStatuses = last.Results;
        } catch (ex: unknown) {
            log.warn(`Couldn't load the last update check: ${errorMessage(ex)}`);
        }

        // An empty library: offer what can be imported right away.
        if (loadedMods.length === 0) {
            detectImportSources()
                .then(found => importSources = found)
                .catch((ex: unknown) => log.warn(`Couldn't look for mods to import: ${errorMessage(ex)}`));
        }

        log.info("Initialization complete.");
    }

    function applyCurrentConfigChanges() {
        switch (currentProfile!.Version) {
            case "V1":
                currentProfile!.Configs = profileConfigs;
                break;
        }
    }

    /** Take a deleted mod out of every profile (all of its entries). */
    function removeModFromProfiles(guid: UUID) {
        for (const profile of profiles) {
            switch (profile.Version) {
                case "V1":
                    removeEntriesOf(profile.Configs, guid);
                    break;
            }
        }
        // The active profile's list as shown (normally the same array).
        removeEntriesOf(profileConfigs, guid);
    }

    function updatesFor(guid: UUID): UpdateStatusEntry[] {
        return updateStatuses.filter(u => u.Guid === guid);
    }

    function hasUpdateAvailable(guid: UUID): boolean {
        return updatesFor(guid).some(u => u.Status.Kind === "UpdateAvailable");
    }

    /** The update to use for a mod: prefer a one-click (direct) source. */
    function bestUpdateFor(guid: UUID): UpdateStatusEntry | undefined {
        const available = updatesFor(guid).filter(u => u.Status.Kind === "UpdateAvailable");
        return available.find(u => u.Method === "Direct" && (u.Files?.length ?? 0) > 0) ?? available[0];
    }

    function needsNexusKey(guid: UUID): boolean {
        return updatesFor(guid).some(u => u.Status.Kind === "NeedsApiKey");
    }

    function makeConfigForMod(mod: Mod): Config {
        return defaultConfigFor(mod.Manifest);
    }

    /** Take repeated entries of a mod out of every profile (keeping the
     * first); returns the names of those mods (their GUID when missing). */
    function removeRepeatedEntries(allProfiles: Profile[], loaded: Mod[]): string[] {
        const names: string[] = [];
        for (const profile of allProfiles) {
            const repeated = removeDuplicateEntries(profile.Configs);
            if (repeated.length === 0) continue;
            const named = repeated.map(guid => loaded.find(m => m.guid.toLowerCase() === guid.toLowerCase())?.name ?? guid);
            log.warn(`Profile "${profile.Name}" listed some mods more than once; removed the extra entries of: ${named.join(", ")}`);
            for (const name of named) if (!names.includes(name)) names.push(name);
        }
        if (names.length > 0) {
            showPopup(new NotificationPopup("warning", t("pages.mods.popup.notification.duplicate_entries.message", { names: names.join(", ") })));
        }
        return names;
    }

    /** Reset the option choices of entries that no longer fit their mod
     * (it came back after being missing, or changed underneath), keeping
     * on/off and position; returns the names of the mods that were reset. */
    function fitProfilesToMods(allProfiles: Profile[], loaded: Mod[]): string[] {
        const reset: string[] = [];
        for (const profile of allProfiles) {
            profile.Configs = profile.Configs.map(config => {
                const mod = loaded.find(m => m.guid === config.Guid);
                if (!mod) return config;
                const fitted = fitConfig(config, mod.Manifest);
                if (fitted.reset && !reset.includes(mod.name)) reset.push(mod.name);
                return fitted.config;
            });
        }
        return reset;
    }

    /** The delete in progress, if any: closing waits for it (see
     * `handleCloseRequest`), so the profiles saved on close already have
     * the deleted mod taken out. */
    let deleteInFlight: Promise<void> | null = null;

    async function doDeleteMod(guid: string) {
        const run = deleteModNow(guid);
        deleteInFlight = run;
        try {
            await run;
        } finally {
            if (deleteInFlight === run) deleteInFlight = null;
        }
    }

    async function deleteModNow(guid: string) {
        const mod = mods.find(m => m.guid == guid);
        if (!mod) return;

        const wait = new WaitPopup(t("pages.mods.popup.wait.delete.message"));
        showPopup(wait);

        try {
            // The backend deletes first. Only a mod it really removed leaves
            // the lists here: a delete that fails changes nothing, on either
            // side, so the mod can't linger unseen and block adding it again.
            await deleteMod(mod.guid);
            const i = mods.findIndex(m => m.guid === mod.guid);
            if (i !== -1) mods.splice(i, 1);
            removeModFromProfiles(mod.guid);
            updateStatuses = updateStatuses.filter(u => u.Guid !== mod.guid);
            // Saved now, not only on close: a deleted mod must never come
            // back as "Mod not found".
            const saved = await doSaveProfiles();
            if (!saved.ok) log.warn(`Failed to save profiles after deleting a mod: ${saved.error}`);
        } catch(ex: unknown) {
            showPopup(new ErrorPopup(t("pages.mods.popup.error.delete.message"), errorMessage(ex)));
            // Show what the backend really has (e.g. the mod was already gone).
            try {
                mods = await getMods();
            } catch (reloadEx: unknown) {
                log.warn(`Couldn't reload the mod list after a failed delete: ${errorMessage(reloadEx)}`);
            }
        } finally {
            wait.close();
        }
    }

    async function doAddMod(filename: string): Promise<Mod | undefined> {
        const wait = new WaitPopup(t("pages.mods.popup.wait.add.message"));
        showPopup(wait);
        try {
            const { mod, warning } = await addMod(filename);
            mods.push(mod);
            if (warning) showPopup(new NotificationPopup("warning", warning));
            return mod;
        } catch(ex: unknown) {
            let message: string;
            if (ex instanceof Error) {
                message = ex.message;
            } else if (typeof ex === "string") {
                message = ex;
            } else {
                message = "Unknown error!";
            }
            showPopup(new ErrorPopup(t("pages.mods.popup.error.add.message"), message));
            return undefined;
        } finally {
            wait.close();
        }
    }

    async function doAddMods(...filenames: string[]) {
        const wait = new WaitPopup(t("pages.mods.popup.wait.add_multiple.message"));
        showPopup(wait);

        try {
            const results = await addMods(filenames);

            const addResults = results.map<ModAddResult>((r, i) => {
                if ("Ok" in r) {
                    return {
                        success: true,
                        archiveFile: filenames[i],
                        warning: r.Ok.warning
                    };
                } else {
                    return {
                        success: false,
                        archiveFile: filenames[i],
                        errorMessage: r.Err
                    }
                }
            });
            const popup = new AddResultPopup(addResults);
            showPopup(popup);

            const modsToAdd = results.filter(r => "Ok" in r).map(r => r.Ok.mod);
            mods.push(...modsToAdd);
        } catch(ex: unknown) {
            let message: string;
            if (ex instanceof Error) {
                message = ex.message;
            } else if (typeof ex === "string") {
                message = ex;
            } else {
                message = "Unknown error!";
            }
            showPopup(new ErrorPopup(t("pages.mods.popup.error.add.message"), message));
        } finally {
            wait.close();
        }
    }

    async function doAddModFolder(folder: string) {
        const wait = new WaitPopup(t("pages.mods.popup.wait.add.message"));
        showPopup(wait);
        try {
            const { mod, warning } = await addModFolder(folder);
            mods.push(mod);
            if (warning) showPopup(new NotificationPopup("warning", warning));
        } catch(ex: unknown) {
            let message: string;
            if (ex instanceof Error) {
                message = ex.message;
            } else if (typeof ex === "string") {
                message = ex;
            } else {
                message = "Unknown error!";
            }
            showPopup(new ErrorPopup(t("pages.mods.popup.error.add.message"), message));
        } finally {
            wait.close();
        }
    }

    async function doAddPaths(...paths: string[]) {
        const wait = new WaitPopup(t("pages.mods.popup.wait.add_multiple.message"));
        showPopup(wait);

        try {
            const results = await addPaths(paths);

            const addResults = results.map<ModAddResult>((r, i) => {
                if ("Ok" in r) {
                    return {
                        success: true,
                        archiveFile: paths[i],
                        warning: r.Ok.warning
                    };
                } else {
                    return {
                        success: false,
                        archiveFile: paths[i],
                        errorMessage: r.Err
                    }
                }
            });
            const popup = new AddResultPopup(addResults);
            showPopup(popup);

            const modsToAdd = results.filter(r => "Ok" in r).map(r => r.Ok.mod);
            mods.push(...modsToAdd);
        } catch(ex: unknown) {
            let message: string;
            if (ex instanceof Error) {
                message = ex.message;
            } else if (typeof ex === "string") {
                message = ex;
            } else {
                message = "Unknown error!";
            }
            showPopup(new ErrorPopup(t("pages.mods.popup.error.add.message"), message));
        } finally {
            wait.close();
        }
    }

    async function doAddModFromUrl(url: string) {
        const wait = new WaitPopup(t("pages.mods.popup.wait.add_url.message"));
        showPopup(wait);
        try {
            const { mod, warning } = await addModFromUrl(url);
            mods.push(mod);
            if (warning) showPopup(new NotificationPopup("warning", warning));
        } catch(ex: unknown) {
            let message: string;
            if (ex instanceof Error) {
                message = ex.message;
            } else if (typeof ex === "string") {
                message = ex;
            } else {
                message = "Unknown error!";
            }
            wait.close();

            // The site didn't serve the archive directly (login/JS-gated
            // download, or just an HTML page) -- offer a browser handoff
            // instead of a bare error.
            if (message.includes("not a supported archive")) {
                const classification = await classifyDownloadUrl(url);
                const tryHandoff = await showPopup(new ConfirmPopup(
                    t("pages.mods.popup.confirm.offer_handoff.title"),
                    t("pages.mods.popup.confirm.offer_handoff.question", { site: classification.DisplayName }),
                ));
                if (tryHandoff) {
                    await doStartHandoff(url, classification.DisplayName);
                    return;
                }
            }

            const hint = t("pages.mods.popup.error.add_url.hint");
            showPopup(new ErrorPopup(t("pages.mods.popup.error.add_url.message"), `${message}\n\n${hint}`));
            return;
        }
        wait.close();
    }

    async function doStartHandoff(pageUrl: string, siteName: string, existingGuid?: UUID): Promise<HandoffResult["status"]> {
        const result = await showPopup(new HandoffPopup(pageUrl, siteName, downloadsPath, existingGuid));
        switch (result.status) {
            case "Done":
                if (!existingGuid) {
                    mods.push(result.mod);
                } else {
                    const i = mods.findIndex(m => m.guid === existingGuid || m.guid === result.mod.guid);
                    if (i === -1) {
                        mods.push(result.mod);
                    } else {
                        mods[i] = result.mod;
                    }
                }
                if (result.warning) showPopup(new NotificationPopup("warning", result.warning));
                break;
            case "TimedOut":
                showPopup(new NotificationPopup("warning", t("popup.handoff.timed_out_message")));
                break;
            case "Error":
                showPopup(new ErrorPopup(t("pages.mods.popup.error.add_url.message"), result.message));
                break;
            case "Cancelled":
                break;
        }
        return result.status;
    }

    /** Shared by the manual "Add URL" button and a `ddmm://install` deep
     * link: classify the URL and either open a browser handoff or add it
     * directly. */
    async function installFromUrl(url: string) {
        const classification = await classifyDownloadUrl(url);
        if (classification.RequiresHandoff) {
            await doStartHandoff(url, classification.DisplayName);
        } else {
            await doAddModFromUrl(url);
        }
    }

    /** `bridge://consent-request` -- the first time a site tries to
     * install through the browser extension, ask the user, per
     * docs/development/bridge-protocol.md. */
    async function onBridgeConsentRequest(payload: { requestId: string; site: string | null; fileName?: string }) {
        const decision: BridgeConsentDecision = await showPopup(
            new BridgeConsentPopup(payload.site, payload.fileName ?? ""),
        );
        await resolveBridgeConsent(payload.requestId, decision);
    }

    type BridgeModInstalledPayload = {
        requestId: string;
        mod: { guid: UUID; name: string };
        afterInstall: "library" | "profile" | "deploy";
    };

    /** `bridge://mod-installed` -- the backend already installed the mod
     * (it doesn't touch profiles.json itself); this does the
     * `afterInstall` step (add to the active profile, and/or deploy) with
     * the same logic "Insert" and "Deploy" already use, then reports back
     * what happened so the extension gets an accurate reply. */
    async function onBridgeModInstalled(payload: BridgeModInstalledPayload) {
        // The bridge installed this behind the frontend's back; refresh
        // before touching it.
        mods = await getMods();
        await refreshUpdateStatuses();
        const mod = mods.find(m => m.guid === payload.mod.guid);

        let addedToProfile: string | undefined;
        let deployed = false;
        let softError: BridgeSoftError | undefined;
        const warnings: string[] = [];

        if (mod && payload.afterInstall !== "library" && currentProfile) {
            if (!profileConfigs.some(c => c.Guid === mod.guid)) {
                profileConfigs.push(makeConfigForMod(mod));
            }
            if ((await doSaveProfiles()).ok) {
                addedToProfile = currentProfile.Name;
            } else {
                warnings.push(t("toast.bridge_install.profile_save_failed"));
            }

            if (payload.afterInstall === "deploy") {
                if (await isGameRunning()) {
                    softError = { code: "DEPLOY_FAILED", message: t("toast.bridge_install.game_running") };
                } else {
                    deploying = true;
                    try {
                        await deploy(currentProfile.Configs);
                        deployed = true;
                    } catch (ex: unknown) {
                        const message = errorMessage(ex);
                        softError = {
                            code: message.includes("invalid settings") ? "GAME_NOT_FOUND" : "DEPLOY_FAILED",
                            message,
                        };
                    } finally {
                        deploying = false;
                    }
                }
            }
        }

        await resolveBridgeInstallCompletion(payload.requestId, addedToProfile, deployed, warnings, softError);

        const name = payload.mod.name;
        if (softError) {
            showToast("warning", t("toast.bridge_install.installed_with_issue", { name, message: softError.message }));
        } else if (deployed) {
            showToast("info", t("toast.bridge_install.deployed", { name }));
        } else if (addedToProfile) {
            showToast("info", t("toast.bridge_install.added_to_profile", { name, profile: addedToProfile }));
        } else {
            showToast("info", t("toast.bridge_install.added_to_library", { name }));
        }
    }

    /** Show the confirmation for every queued `ddmm://install` link, once
     * this page is ready for it (profiles loaded; not when init sent the
     * user to Settings instead). Called on mount and on each
     * `deep-link://pending`. The backend hands each link out only once, so
     * overlapping calls can't show one twice, and a link that arrives
     * before the page is ready simply waits in the queue. */
    async function showPendingDeepLinks() {
        try {
            await initPromise;
        } catch {
            return;
        }
        if (unmounted || !profilesLoaded) return;

        let urls: string[];
        try {
            urls = await takePendingDeepLinks();
        } catch (ex: unknown) {
            log.error(`Failed to get pending ddmm:// links: ${errorMessage(ex)}`);
            return;
        }
        for (const url of urls) {
            onDeepLinkInstallRequest({ url })
                .catch((ex: unknown) => log.error(`ddmm:// install failed: ${errorMessage(ex)}`));
        }
    }

    /** `ddmm://install` targets whose confirmation (or the install after
     * it) is still in progress -- the same link arriving again meanwhile
     * (one click delivered twice) isn't shown a second time. */
    const deepLinksInProgress = new Set<string>();

    /** A `ddmm://install?url=` link (see `showPendingDeepLinks`).
     * Always confirms (no "always allow"), then runs the same flow as
     * manually pasting the URL into "Add URL". */
    async function onDeepLinkInstallRequest(payload: { url: string }) {
        if (deepLinksInProgress.has(payload.url)) {
            log.info("This ddmm:// link is already being handled; not asking again.");
            return;
        }
        deepLinksInProgress.add(payload.url);
        try {
            let site: string;
            try {
                site = new URL(payload.url).hostname;
            } catch {
                site = payload.url;
            }

            const confirmed = await showPopup(new ConfirmPopup(
                t("pages.mods.popup.confirm.deep_link_install.title"),
                t("pages.mods.popup.confirm.deep_link_install.question", { site }),
            ));
            if (!confirmed) return;

            await installFromUrl(payload.url);
        } finally {
            deepLinksInProgress.delete(payload.url);
        }
    }

    /** `auto-import://candidate` -- a new, finished archive appeared in
     * Downloads and looked like a Helldivers 2 mod (opt-in, off by
     * default; see auto_import.rs). Never installs without this click. */
    async function onAutoImportCandidate(payload: { file: string }) {
        const decision = await showPopup(new AutoImportPopup(payload.file));
        if (decision === "Ignore") return;

        const mod = await doAddMod(payload.file);
        if (!mod) return;

        if (decision === "InstallAndDeploy" && currentProfile) {
            if (!profileConfigs.some(c => c.Guid === mod.guid)) {
                profileConfigs.push(makeConfigForMod(mod));
            }
            const saved = await doSaveProfiles();
            if (!saved.ok) log.warn(`Failed to save profiles after auto-import: ${saved.error}`);

            deploying = true;
            try {
                await deploy(currentProfile.Configs);
                showToast("info", t("toast.auto_import.deployed", { name: mod.name }));
            } catch (ex: unknown) {
                showToast("warning", t("toast.auto_import.deploy_failed", { message: errorMessage(ex) }));
            } finally {
                deploying = false;
            }
        }
    }

    type SaveResult = { ok: true } | { ok: false; error: string };

    function errorMessage(ex: unknown): string {
        if (ex instanceof Error) return ex.message;
        if (typeof ex === "string") return ex;
        return "Unknown error!";
    }

    /** An error for the log file and the error screen: its message, and
     * where it happened when known. */
    function describeError(ex: unknown): string {
        if (ex instanceof Error) return ex.stack && !ex.stack.includes(ex.message) ? `${ex.message}\n${ex.stack}` : (ex.stack ?? ex.message);
        if (typeof ex === "string") return ex;
        try {
            return JSON.stringify(ex);
        } catch {
            return String(ex);
        }
    }

    /** The mod list failed to render (or to update): the page shows the
     * error instead, and the log says what it was. */
    function onRenderError(ex: unknown) {
        log.error(`The Mods page couldn't be shown: ${describeError(ex)}`);
    }

    async function withTimeout<T>(promise: Promise<T>, ms: number, timeoutMessage: string): Promise<T> {
        let timer: ReturnType<typeof setTimeout>;
        const timeout = new Promise<never>((_, reject) => {
            timer = setTimeout(() => reject(new Error(timeoutMessage)), ms);
        });

        try {
            return await Promise.race([promise, timeout]);
        } finally {
            clearTimeout(timer!);
        }
    }

    async function doSaveProfiles(): Promise<SaveResult> {
        const wait = new WaitPopup(t("pages.mods.popup.wait.saving.message"));
        showPopup(wait);

        try {
            applyCurrentConfigChanges()
            await saveProfiles({ Profiles: profiles, Active: activeProfile });
            return { ok: true };
        } catch (ex: unknown) {
            return { ok: false, error: errorMessage(ex) };
        } finally {
            wait.close();
        }
    }

    /** Same as `doSaveProfiles`, but gives up (rather than hanging
     * indefinitely) after `CLOSE_SAVE_TIMEOUT_MS`. Used only on the close
     * path, where we must always eventually decide whether to close. */
    async function saveProfilesBeforeClose(): Promise<SaveResult> {
        try {
            return await withTimeout(
                doSaveProfiles(),
                CLOSE_SAVE_TIMEOUT_MS,
                t("pages.mods.popup.confirm.close_save_failed.timeout_error"),
            );
        } catch (ex: unknown) {
            return { ok: false, error: errorMessage(ex) };
        }
    }

    /** Actually closes the window. Tries the normal `destroy()` IPC call
     * first; if that throws for any reason, falls back to a Rust command
     * that exits the process directly, so a broken `destroy()` call can
     * never strand the user with an unclosable window again. */
    async function closeWindow() {
        log.info("Destroying window.");
        try {
            await appWindow.destroy();
            log.info("Window destroy() resolved.");
        } catch (ex: unknown) {
            log.error(`Window destroy() failed, falling back to force_exit: ${errorMessage(ex)}`);
            try {
                await forceExit();
            } catch (ex2: unknown) {
                log.error(`force_exit fallback also failed: ${errorMessage(ex2)}`);
            }
        }
    }

    async function handleCloseRequest() {
        log.info(`Handling close request (deploying=${deploying}, importing=${importing}, profilesLoaded=${profilesLoaded}).`);
        if (importing) {
            const confirmed = await showPopup(new ConfirmPopup(
                t("pages.mods.popup.confirm.close_importing.title"),
                t("pages.mods.popup.confirm.close_importing.question"),
            ));
            if (!confirmed) {
                log.info("Close cancelled: import window open.");
                return;
            }
        }
        if (deploying) {
            const confirmed = await showPopup(new ConfirmPopup(
                t("pages.mods.popup.confirm.close_deploying.title"),
                t("pages.mods.popup.confirm.close_deploying.question"),
            ));
            if (!confirmed) {
                log.info("Close cancelled: deploy in progress.");
                return;
            }
        }

        // A delete that's still running would otherwise race this save:
        // profiles saved with the mod still in them would bring it back as
        // "Mod not found". Wait for it (it's quick: its files are deleted
        // in the background).
        if (deleteInFlight) {
            log.info("Close requested during a delete; waiting for it first.");
            try {
                await withTimeout(deleteInFlight, CLOSE_SAVE_TIMEOUT_MS, "delete still running");
            } catch (ex: unknown) {
                log.warn(`Closing without waiting any longer for the delete: ${errorMessage(ex)}`);
            }
        }

        // Nothing was ever loaded (init() hasn't finished, or errored out
        // before loading profiles) -- there's nothing to save, and trying
        // to would just fail on the undefined `currentProfile`.
        if (profilesLoaded) {
            const result = await saveProfilesBeforeClose();
            log.info(`Save before close: ${result.ok ? "ok" : `failed (${result.error})`}.`);
            if (!result.ok) {
                const confirmed = await showPopup(new ConfirmPopup(
                    t("pages.mods.popup.confirm.close_save_failed.title"),
                    t("pages.mods.popup.confirm.close_save_failed.question", { error: result.error }),
                ));
                if (!confirmed) {
                    log.info("Close cancelled: user chose not to close after a failed save.");
                    return;
                }
            }
        }

        await closeWindow();
    }

    async function onAddProfile() {
        const input = await showPopup(new InputPopup(
            t("pages.mods.popup.input.add_profile.placeholder"),
            false,
            3,
        ));
        if (!input) return;
        
        profiles.push({
            Version: "V1",
            Name: input,
            Configs: []
        });
        activeProfile = profiles.length - 1;
    }

    async function onRemoveProfile() {
        const confirm = await showPopup(
            new ConfirmPopup(
                t("pages.mods.popup.confirm.remove_profile.title"),
                t("pages.mods.popup.confirm.remove_profile.question"),
            ),
        );
        if (!confirm) return;

        profiles.splice(activeProfile, 1);
        if (activeProfile > 0) activeProfile--;
    }

    function onDragEnd(e: SortableList.RootEvents["ondragend"]) {
        const { draggedItemIndex, targetItemIndex, isCanceled } = e;

        if (isCanceled || typeof targetItemIndex !== "number" || draggedItemIndex === targetItemIndex) return;

        const [elm] = profileConfigs.splice(draggedItemIndex, 1);
        profileConfigs.splice(targetItemIndex, 0, elm);
    }

    async function onEditConfig(i: number) {
        if (i < 0 || i >= profileConfigs.length) return;
        const config = profileConfigs[i];
        const mod = mods.find(m => m.guid === config.Guid);
        if (!mod) return;
        const newConfig = await showPopup(new ModConfigPopup(mod, config));
        if (!newConfig) return;
        // The list may have changed while the popup was open: write back
        // only to this mod's own entry, never over another mod's (that
        // would list a mod twice; see issue #71).
        const at = profileConfigs[i]?.Guid === config.Guid ? i : profileConfigs.findIndex(c => c.Guid === config.Guid);
        if (at === -1 || newConfig.Guid !== config.Guid) return;
        profileConfigs[at] = newConfig;
    }

    function onRemove(i: number) {
        profileConfigs.splice(i, 1);
    }

    /** "Remove" on a "Mod not found" entry: gone at once, and saved right
     * away so it doesn't come back on the next start. */
    async function onRemoveMissing(i: number) {
        profileConfigs.splice(i, 1);
        const saved = await doSaveProfiles();
        if (!saved.ok) log.warn(`Failed to save profiles after removing a missing entry: ${saved.error}`);
    }

    function onMoveUp(i: number) {
        if (i === 0) return;

        const [elm] = profileConfigs.splice(i, 1);
        profileConfigs.splice(i - 1, 0, elm);
    }

    function onMoveDown(i: number) {
        if (i === profileConfigs.length - 1) return;

        const [elm] = profileConfigs.splice(i, 1);
        profileConfigs.splice(i + 1, 0, elm);
    }

    function onToTop(i: number) {
        if (i === 0) return;

        const [elm] = profileConfigs.splice(i, 1);
        profileConfigs.splice(0, 0, elm);
    }

    function onToBottom(i: number) {
        if (i === profileConfigs.length - 1) return;

        const [elm] = profileConfigs.splice(i, 1);
        profileConfigs.push(elm);
    }

    async function onToggleLibrary() {
        libraryExtended = !libraryExtended;

        if (libraryExtended) {
            await new Promise((r) => setTimeout(r, 150));
            libraryVisible = true;
        } else {
            libraryVisible = false;
        }
    }

    function onInsertTop(i: number) {
        const mod = libraryMods[i];
        const config = makeConfigForMod(mod);
        profileConfigs.splice(0, 0, config);
    }

    function onInsertBottom(i: number) {
        const mod = libraryMods[i];
        const config = makeConfigForMod(mod);
        profileConfigs.push(config);
    }

    async function onDelete(i: number) {
        const confirm = new ConfirmPopup(
            t("pages.mods.popup.confirm.delete.title"),
            t("pages.mods.popup.confirm.delete.question"),
        );
        if (!await showPopup(confirm)) return;
        const mod = libraryMods[i];
        await doDeleteMod(mod.guid);
    }

    // --- Mod updates -------------------------------------------------------
    // Checks only run when the user asks: "Check for Updates", or the
    // opt-in automatic checks in Settings (off by default).

    async function refreshUpdateStatuses() {
        try {
            const last = await getLastUpdateReport();
            if (last) updateStatuses = last.Results;
        } catch (ex: unknown) {
            log.warn(`Couldn't refresh update statuses: ${errorMessage(ex)}`);
        }
    }

    function onUpdatesChecked(report: UpdateCheckReport) {
        updateStatuses = report.Results;
        const count = new Set(report.Results.filter(u => u.Status.Kind === "UpdateAvailable").map(u => u.Guid)).size;
        if (report.Trigger !== "Manual" && count > 0) {
            showToast("info", t("toast.updates.available", { count }));
        }
    }

    function replaceMod(oldGuid: UUID, updated: Mod) {
        const i = mods.findIndex(m => m.guid === oldGuid || m.guid === updated.guid);
        if (i === -1) mods.push(updated);
        else mods[i] = updated;
        iconPaths.delete(updated.guid);
    }

    /** "updated" -- DDMM installed it (needs the redeploy step);
     * "updatedByExtension" -- the extension's install already ran the
     * afterInstall step. */
    type UpdateOutcome = "updated" | "updatedByExtension" | "skipped" | "stopped" | "failed";

    async function runUpdate(
        entry: UpdateStatusEntry,
        position?: { index: number; total: number },
        failures?: string[],
    ): Promise<UpdateOutcome> {
        const mod = mods.find(m => m.guid === entry.Guid);
        if (!mod) return "failed";

        const reportFailure = (message: string) => {
            if (failures) failures.push(`${mod.name}: ${message}`);
            else showPopup(new ErrorPopup(t("pages.mods.popup.error.update.message", { name: mod.name }), message));
        };

        if (entry.Method === "Direct" && (entry.Files?.length ?? 0) > 0) {
            let file = entry.Files!.find(f => f.Id === entry.PreselectedFile);
            if (!file) {
                const picked = await showPopup(new UpdateFilePickPopup(mod.name, entry));
                if (!picked) return "skipped";
                file = picked;
            }
            const result = await showPopup(new UpdateDownloadPopup(mod.name, entry, file, position));
            if (!result.ok) {
                reportFailure(result.message);
                return "failed";
            }
            replaceMod(entry.Guid, result.mod);
            if (result.warning) {
                if (failures) failures.push(`${mod.name}: ${result.warning}`);
                else showToast("warning", result.warning);
            }
            return "updated";
        }

        if (!entry.PageUrl) {
            reportFailure(t("pages.mods.popup.error.update.no_page"));
            return "failed";
        }

        let extensionActive = false;
        try {
            extensionActive = await browserExtensionActive();
        } catch {
            // Treat as not connected: the Downloads-folder handoff still works.
        }

        if (extensionActive || position) {
            const decision = await showPopup(new BrowserUpdatePopup(mod.name, entry, extensionActive, position));
            switch (decision) {
                case "Done":
                    mods = await getMods();
                    return "updatedByExtension";
                case "Skip":
                    return "skipped";
                case "Stop":
                    return "stopped";
                case "Handoff":
                    break;
            }
        }

        const status = await doStartHandoff(entry.PageUrl, entry.DisplayName, mod.guid);
        return status === "Done" ? "updated" : status === "Cancelled" ? "stopped" : "failed";
    }

    /** After DDMM itself updated mods in place: redeploy per the
     * after-install setting ("deploy" redeploys automatically; otherwise
     * ask), but only if an updated mod is enabled in the active profile. */
    async function afterUpdates(updatedGuids: UUID[]) {
        if (updatedGuids.length === 0 || !currentProfile) return;
        const affectsProfile = updatedGuids.some(g => profileConfigs.some(c => c.Guid === g && c.Enabled));
        if (!affectsProfile) return;

        if (afterBrowserInstall !== "deploy") {
            const confirmed = await showPopup(new ConfirmPopup(
                t("pages.mods.popup.confirm.redeploy_after_update.title"),
                t("pages.mods.popup.confirm.redeploy_after_update.question"),
            ));
            if (!confirmed) return;
        } else if (await isGameRunning()) {
            showToast("warning", t("toast.updates.game_running"));
            return;
        }

        const saved = await doSaveProfiles();
        if (!saved.ok) log.warn(`Failed to save profiles before redeploying: ${saved.error}`);

        const wait = new WaitPopup(t("pages.mods.popup.wait.deploy.message"));
        showPopup(wait);
        deploying = true;
        try {
            await deploy(currentProfile.Configs);
            showToast("info", t("toast.updates.redeployed"));
        } catch (ex: unknown) {
            showPopup(new ErrorPopup(t("pages.mods.popup.error.deploy.message"), errorMessage(ex)));
        } finally {
            deploying = false;
            wait.close();
        }
    }

    async function onUpdateEntry(entry: UpdateStatusEntry) {
        const outcome = await runUpdate(entry);
        await refreshUpdateStatuses();
        if (outcome === "updated") {
            const mod = mods.find(m => m.guid === entry.Guid);
            showToast("info", t("toast.updates.updated", { name: mod?.name ?? "" }));
            await afterUpdates([entry.Guid]);
        }
    }

    async function onUpdateMod(mod: Mod) {
        const entry = bestUpdateFor(mod.guid);
        if (entry) await onUpdateEntry(entry);
    }

    async function onSkipVersion(entry: UpdateStatusEntry, skip: boolean) {
        try {
            await skipUpdateVersion(entry.Guid, entry.Provider, skip ? (entry.LatestVersion ?? null) : null, skip ? (entry.LatestModifiedAt ?? null) : null);
            await refreshUpdateStatuses();
        } catch (ex: unknown) {
            showPopup(new ErrorPopup(t("pages.mods.popup.error.skip_version.message"), errorMessage(ex)));
        }
    }

    /** "Update all (N)": one-click (direct) updates run automatically
     * first; updates that need the browser are then offered one page at a
     * time. */
    async function onUpdateAll() {
        const seen = new Set<UUID>();
        const entries: UpdateStatusEntry[] = [];
        for (const u of updateStatuses) {
            if (u.Status.Kind !== "UpdateAvailable" || seen.has(u.Guid)) continue;
            const best = bestUpdateFor(u.Guid);
            if (!best) continue;
            seen.add(u.Guid);
            entries.push(best);
        }
        const direct = entries.filter(e => e.Method === "Direct" && (e.Files?.length ?? 0) > 0);
        const browser = entries.filter(e => !direct.includes(e));
        const ordered = [...direct, ...browser];

        const failures: string[] = [];
        const redeploy: UUID[] = [];
        let updated = 0;
        for (let i = 0; i < ordered.length; i++) {
            const outcome = await runUpdate(ordered[i], { index: i + 1, total: ordered.length }, failures);
            if (outcome === "updated") redeploy.push(ordered[i].Guid);
            if (outcome === "updated" || outcome === "updatedByExtension") updated++;
            if (outcome === "stopped") break;
        }

        await refreshUpdateStatuses();
        if (failures.length > 0) {
            showPopup(new ErrorPopup(
                t("pages.mods.popup.error.update_all.message", { updated, failed: failures.length }),
                failures.join("\n"),
            ));
        } else if (updated > 0) {
            showToast("info", t("toast.updates.bulk_done", { count: updated }));
        }
        await afterUpdates(redeploy);
    }

    async function showUpdatesPopup(report: UpdateCheckReport) {
        for (;;) {
            const names = new Map(mods.map(m => [m.guid, m.name] as [UUID, string]));
            const action = await showPopup(new UpdatesPopup({ ...report, Results: updateStatuses }, names));
            if (!action) return;
            switch (action.kind) {
                case "update":
                    await onUpdateEntry(action.entry);
                    break;
                case "skip":
                    await onSkipVersion(action.entry, true);
                    break;
                case "unskip":
                    await onSkipVersion(action.entry, false);
                    break;
                case "updateAll":
                    await onUpdateAll();
                    return;
                case "nexusSettings":
                    goto("/settings#nexus-api-key");
                    return;
            }
        }
    }

    function onUpdate(i: number) {
        onUpdateMod(libraryMods[i]);
    }

    async function onCheckUpdates() {
        const wait = new WaitPopup(t("pages.mods.popup.wait.check_updates.message"));
        showPopup(wait);
        let report: UpdateCheckReport;
        try {
            report = await checkUpdates();
            updateStatuses = report.Results;
        } catch(ex: unknown) {
            wait.close();
            showPopup(new ErrorPopup(t("pages.mods.popup.error.check_updates.message"), errorMessage(ex)));
            return;
        }
        wait.close();
        await showUpdatesPopup(report);
    }

    /** From this many files on, Add (and drag & drop) go through the import
     * checklist instead of installing straight away: duplicates and
     * already-installed mods are sorted out first, with progress, cancel and
     * one summary instead of a wall of results. */
    const BULK_ADD_THRESHOLD = 10;

    /** "Import mods": scan another mod manager's folder, a folder of
     * archives, or `paths`; then add what was imported to the active
     * profile (keeping the source's order, on/off state and options). */
    async function onImport(paths?: string[]) {
        importWindows++;
        let result;
        try {
            result = await showPopup(new ImportPopup(currentProfile?.Name, paths));
        } finally {
            importWindows--;
        }
        mods = await getMods();
        importSources = [];
        if (!result || result.report.Imported.length === 0) return;
        await refreshUpdateStatuses();

        if (result.addToProfile && currentProfile) {
            // Ones with a known place in the source's load order first, in
            // that order; the rest after them, in import order.
            const imported = [...result.report.Imported].sort((a, b) =>
                (a.Order ?? Number.MAX_SAFE_INTEGER) - (b.Order ?? Number.MAX_SAFE_INTEGER));
            for (const entry of imported) {
                if (profileConfigs.some(c => c.Guid === entry.Guid)) continue;
                const mod = mods.find(m => m.guid === entry.Guid);
                if (!mod) continue;
                const config = entry.Config ?? makeConfigForMod(mod);
                if (!entry.Config && entry.Enabled !== undefined) config.Enabled = entry.Enabled;
                profileConfigs.push(config);
            }
            const saved = await doSaveProfiles();
            if (saved.ok) {
                showToast("info", t("toast.import.added_to_profile", { profile: currentProfile.Name }));
            } else {
                showPopup(new NotificationPopup("warning", t("toast.import.profile_save_failed", { error: saved.error })));
            }
        }
    }

    async function onAddMod() {
        const filenames = await open({
            multiple: true,
            directory: false,
            filters: [
                {
                    name: "Archives",
                    extensions: ["zip", "7z", "rar"]
                }
            ]
        });
        if (!filenames) return;

        if (filenames.length >= BULK_ADD_THRESHOLD) {
            await onImport(filenames);
        } else if (filenames.length == 1) {
            await doAddMod(filenames[0]);
        } else {
            await doAddMods(...filenames);
        }
    }

    async function onAddFolder() {
        const folders = await open({
            multiple: true,
            directory: true
        });
        if (!folders) return;

        if (folders.length == 1) {
            await doAddModFolder(folders[0]);
        } else {
            await doAddPaths(...folders);
        }
    }

    async function onAddUrl() {
        const url = await showPopup(new InputPopup(
            t("pages.mods.popup.input.add_url.placeholder"),
            false,
            undefined,
            undefined,
            /^https:\/\//,
            t("pages.mods.popup.input.add_url.description"),
        ));
        if (!url) return;

        await installFromUrl(url);
    }

    async function onPurge() {
        const confirm = await showPopup(
            new ConfirmPopup(
                t("pages.mods.popup.confirm.purge.title"),
                t("pages.mods.popup.confirm.purge.question"),
            ),
        );
        if (!confirm) return;

        const wait = new WaitPopup(t("pages.mods.popup.wait.purge.message"));
        showPopup(wait);

        try {
            await purge();
            showPopup(new NotificationPopup("info", t("pages.mods.popup.notification.purge_success.message")));
        } catch(ex: unknown) {
            let message: string;
            if (ex instanceof Error) {
                message = ex.message;
            } else if (typeof ex === "string") {
                message = ex;
            } else {
                message = "Unknown error!";
            }
            showPopup(new ErrorPopup(t("pages.mods.popup.error.purge.message"), message));
        } finally {
            wait.close();
        }
    }

    async function onDeploy() {
        if (!currentProfile) return;
        
        // A mod that came back (re-added, repaired) may not fit its old
        // option choices any more.
        const resetOptions = fitProfilesToMods([currentProfile], mods);
        if (resetOptions.length > 0) {
            profileConfigs = currentProfile.Configs;
            log.warn(`Options reset to defaults before deploying: ${resetOptions.join(", ")}`);
            showToast("warning", t("pages.mods.popup.notification.options_reset.message", { names: resetOptions.join(", ") }));
        }

        // Entries whose mod isn't in the library are skipped by deploy; a
        // profile of only those would just remove every mod from the game.
        const { loaded, missing } = deployableEntries(currentProfile.Configs, mods.map(m => m.guid));
        if (loaded.length === 0) {
            showPopup(new NotificationPopup("error", missing > 0
                ? t("pages.mods.popup.notification.empty_deploy_error.all_missing", { count: missing })
                : t("pages.mods.popup.notification.empty_deploy_error.message")));
            return;
        }

        const wait = new WaitPopup(t("pages.mods.popup.wait.deploy.message"));
        showPopup(wait);
        deploying = true;

        try {
            await deploy(currentProfile.Configs);
            showPopup(missing > 0
                ? new NotificationPopup("warning", t("pages.mods.popup.notification.deploy_success.missing_skipped", { count: missing }))
                : new NotificationPopup("info", t("pages.mods.popup.notification.deploy_success.message")));
        } catch(ex: unknown) {
            showPopup(new ErrorPopup(t("pages.mods.popup.error.deploy.message"), errorMessage(ex)));
        } finally {
            deploying = false;
            wait.close();
        }
    }

    function onLaunch() {
        openUrl("steam://launch/553850");
    }
</script>

{#await initPromise}
    <div class="w-full h-full flex justify-center items-center">
        <div
            class="p-4 bg-zinc-800 border-2 border-zinc-500 flex flex-col gap-1 items-center"
        >
            <div
                class="w-6 h-6 rounded-full border-4 border-transparent border-b-yellow-300 animate-spin"
            ></div>
            <span class="text-zinc-300">{t("pages.mods.loading.text")}</span>
        </div>
    </div>
{:then _}
    <svelte:boundary onerror={onRenderError}>
    <div class="w-full h-full flex flex-col gap-1 relative">
        <div class="flex flex-row gap-1">
            <!-- Profiles -->
            <select class="flex-1 hd2mm-select" bind:value={activeProfile}>
                {#each profiles as profile, i}
                    <option value={i}>
                        {#if profile.Version === "V1"}
                            {profile.Name}
                        {/if}
                    </option>
                {/each}
            </select>
            <button
                class="hd2mm-button"
                title={t("pages.mods.add_profile_button.tip")}
                onclick={onAddProfile}
            >
                <Plus width="24" height="24" />
            </button>
            <button
                class="hd2mm-button"
                title={t("pages.mods.remove_profile_button.tip")}
                onclick={onRemoveProfile}
                disabled={!enableRemoveProfile}
            >
                <Dash width="24" height="24" />
            </button>
        </div>
        <!-- Search -->
        <div class="flex flex-row gap-1">
            <input
                class="flex-1 hd2mm-input"
                type="text"
                placeholder={t("pages.mods.search_input.placeholder")}
                autocomplete="off"
                autocorrect="off"
                autocapitalize="off"
                spellcheck="false"
                bind:value={searchText}
            />
            <button
                class="hd2mm-button"
                title={t("pages.mods.clear_search_button.tip")}
                onclick={() => (searchText = "")}
                disabled={!enableClearSearch}
            >
                <Backspace width="24" height="24" />
            </button>
        </div>
        <!-- Center -->
        <div class="flex-1 flex flex-row relative min-h-0">
            <!-- Mod List -->
            <div class="flex-1 mr-7 pr-1 overflow-y-scroll h-full">
                {#if mods.length === 0}
                    <div class="h-full flex flex-col items-center justify-center gap-1 text-center px-4">
                        <span class="text-zinc-400 text-lg">{t("pages.mods.empty_state.title")}</span>
                        <span class="text-zinc-500 text-sm max-w-100">{t("pages.mods.empty_state.works_with")}</span>
                        <span class="text-zinc-400 text-sm max-w-100 mt-4">{t("pages.mods.empty_state.import_hint")}</span>
                        {#each withUniqueKeys(importSources.slice(0, 3), s => s.Path) as [source, key] (key)}
                            <span class="text-zinc-500 text-xs max-w-120 break-all" data-testid="empty-import-found">
                                {source.Count === 1
                                    ? t("pages.mods.empty_state.import_found_one", { path: source.Path })
                                    : t("pages.mods.empty_state.import_found", { count: source.Count, path: source.Path })}
                            </span>
                        {/each}
                        <button class="hd2mm-button flex flex-row gap-1 items-center mt-1" onclick={() => onImport()} data-testid="empty-import-button">
                            <BoxArrowInDown />
                            {t("pages.mods.empty_state.import_button")}
                        </button>
                    </div>
                {/if}
                {#if profileEntries.length > 0}
                    <div class="flex flex-row items-center gap-1 px-1 pb-1 text-xs text-zinc-400" data-testid="load-order-hint">
                        <InfoCircle class="shrink-0" width="12" height="12" />
                        <span>{t("pages.mods.load_order.hint")}</span>
                    </div>
                {/if}
                {#if profileEntries.length > 1}
                    <div class="flex flex-row items-center gap-1 px-1 pb-1 text-xs text-zinc-500">
                        <CaretUpFill class="shrink-0" width="10" height="10" />
                        <span>{t("pages.mods.load_order.lowest")}</span>
                    </div>
                {/if}
                <SortableList.Root
                    ondragend={onDragEnd}
                    isLocked={!allowReorder}
                    gap={4}
                >
                    {#each profileEntries as [config, mod, ci], i (config.Guid)}
                        {@const iconPath = iconPaths.get(config.Guid)}
                        <SortableList.Item
                            id={config.Guid}
                            index={i}
                        >
                            {#if !mod}
                            <div class="p-2 text-zinc-400 bg-zinc-800/60 border border-dashed border-zinc-600 rounded flex flex-row gap-1 items-center" data-testid="missing-mod-entry">
                                <span
                                    class="shrink-0 text-zinc-500 {allowReorder ? 'cursor-grab' : 'opacity-40'}"
                                    title={t("pages.mods.load_order.drag_handle")}
                                    aria-label={t("pages.mods.load_order.drag_handle")}
                                >
                                    <GripVertical width="16" height="16" />
                                </span>
                                <div class="flex-1 flex flex-col gap-0.5 min-w-0">
                                    <span class="text-lg truncate">{t("pages.mods.missing_entry.title")}</span>
                                    <span class="text-xs truncate">{t("pages.mods.missing_entry.message")}</span>
                                    <span class="text-xs text-zinc-500 truncate">{config.Guid}</span>
                                </div>
                                <PopupMenuButton insertTarget="main">
                                    <button onclick={() => onRemoveMissing(ci)}>
                                        <Eraser />
                                        <span>Remove</span>
                                    </button>
                                    <hr>
                                    <button disabled={ci === 0} onclick={() => onMoveUp(ci)}>
                                        <CaretUp />
                                        <span>Move Up</span>
                                    </button>
                                    <button disabled={ci === profileConfigs.length - 1} onclick={() => onMoveDown(ci)}>
                                        <CaretDown />
                                        <span>Move Down</span>
                                    </button>
                                </PopupMenuButton>
                            </div>
                            {:else}
                            <div class="p-2 text-zinc-300 bg-zinc-800 rounded flex flex-row gap-1 items-center">
                                <span
                                    class="shrink-0 text-zinc-500 {allowReorder ? 'cursor-grab' : 'opacity-40'}"
                                    title={t("pages.mods.load_order.drag_handle")}
                                    aria-label={t("pages.mods.load_order.drag_handle")}
                                >
                                    <GripVertical width="16" height="16" />
                                </span>
                                <img
                                    class="w-14 h-14 object-contain"
                                    src={iconPath ?? FALLBACK_MOD_IMAGE}
                                    onerror={useFallbackImage}
                                    alt="Mod icon"
                                />
                                <div class="flex-1 flex flex-col gap-0.5 justify-between min-w-0">
                                    <span class="text-2xl truncate flex flex-row items-center gap-1">
                                        {mod.name}
                                        {#if hasUpdateAvailable(mod.guid)}
                                            <span title={t("pages.mods.update_available_badge.tip")}><CloudArrowDownFill class="text-yellow-300 shrink-0" width="16" height="16" /></span>
                                        {/if}
                                    </span>
                                    {#if "Version" in mod.Manifest && mod.Manifest.Version === 2 && mod.Manifest.Tags}
                                        <div class="flex flex-row gap-1 overflow-hidden">
                                            {#each mod.Manifest.Tags as tag}
                                                <span class="px-1 bg-zinc-700 text-xs rounded">{tag}</span>
                                            {/each}
                                        </div>
                                    {/if}
                                    <span class="text-sm truncate">{mod.description}</span>
                                </div>
                                {#if hasUpdateAvailable(mod.guid)}
                                    <button
                                        class="hd2mm-success-button text-sm px-2 shrink-0 flex flex-row gap-1 items-center"
                                        title={t("pages.mods.update_mod_button.tip", { version: bestUpdateFor(mod.guid)?.LatestVersion ?? "", site: bestUpdateFor(mod.guid)?.DisplayName ?? "" })}
                                        onclick={() => onUpdateMod(mod)}
                                    >
                                        <CloudArrowDownFill />
                                        {t("pages.mods.update_mod_button.text")}
                                    </button>
                                {/if}
                                <ToggleSwitch bind:checked={config.Enabled} />
                                {#if config.For === "Legacy"}
                                    {#if !("Version" in mod.Manifest) && mod.Manifest.Options}
                                        <select class="w-32 hd2mm-select" bind:value={config.Selected}>
                                            {#each mod.Manifest.Options as option, i }
                                                <option value={i}>{option}</option>
                                            {/each}
                                        </select>
                                    {/if}
                                {:else}
                                    <button
                                        class="hd2mm-button-nop p-2"
                                        class:invisible={!Array.isArray(mod.Manifest.Options)}
                                        onclick={() => onEditConfig(ci)}
                                    >
                                        <PencilSquare class="block mx-auto" />
                                    </button>
                                {/if}
                                <PopupMenuButton insertTarget="main">
                                    <button onclick={() => onRemove(ci)}>
                                        <Eraser />
                                        <span>Remove</span>
                                    </button>
                                    <hr>
                                    <button
                                        disabled={ci === 0}
                                        onclick={() => onMoveUp(ci)}
                                    >
                                        <CaretUp />
                                        <span>Move Up</span>
                                    </button>
                                    <button
                                        disabled={ci === profileConfigs.length - 1}
                                        onclick={() => onMoveDown(ci)}
                                    >
                                        <CaretDown />
                                        <span>Move Down</span>
                                    </button>
                                    <button
                                        disabled={ci === 0}
                                        onclick={() => onToTop(ci)}
                                    >
                                        <ArrowBarUp />
                                        <span>To Top</span>
                                    </button>
                                    <button
                                        disabled={ci === profileConfigs.length - 1}
                                        onclick={() => onToBottom(ci)}
                                    >
                                        <ArrowBarDown />
                                        <span>To Bottom</span>
                                    </button>
                                    {#if mod.Sources.some((s) => s.PageUrl)}
                                        <hr>
                                        {#each mod.Sources.filter((s) => s.PageUrl) as source}
                                            <button onclick={() => openUrl(source.PageUrl!)}>
                                                <BoxArrowUpRight />
                                                <span>{t("pages.mods.open_source_page", { name: source.DisplayName })}</span>
                                            </button>
                                        {/each}
                                    {/if}
                                    {#if updatesFor(mod.guid).some(u => u.Status.Kind === "UpdateAvailable" || u.Status.Kind === "Skipped" || u.Status.Kind === "NeedsApiKey" || u.Status.Kind === "NeedsManualCheck")}
                                        <hr>
                                        {#each updatesFor(mod.guid).filter(u => u.Status.Kind === "UpdateAvailable") as entry}
                                            <button onclick={() => onUpdateEntry(entry)}>
                                                <CloudArrowDownFill />
                                                <span>{t("pages.mods.update_from_site", { name: entry.DisplayName })}</span>
                                            </button>
                                            <button onclick={() => onSkipVersion(entry, true)}>
                                                <SkipForward />
                                                <span>{t("pages.mods.skip_version", { version: entry.LatestVersion ?? "" })}</span>
                                            </button>
                                        {/each}
                                        {#each updatesFor(mod.guid).filter(u => u.Status.Kind === "Skipped") as entry}
                                            <button onclick={() => onSkipVersion(entry, false)}>
                                                <ArrowCounterclockwise />
                                                <span>{t("pages.mods.unskip_version", { version: entry.LatestVersion ?? "" })}</span>
                                            </button>
                                        {/each}
                                        {#if updatesFor(mod.guid).some(u => u.Status.Kind === "NeedsManualCheck")}
                                            <button onclick={onCheckUpdates}>
                                                <ArrowRepeat />
                                                <span>{t("popup.updates.state.needs_manual_check")}</span>
                                            </button>
                                        {/if}
                                        {#if needsNexusKey(mod.guid)}
                                            <button onclick={() => goto("/settings#nexus-api-key")}>
                                                <Key />
                                                <span>{t("pages.mods.needs_nexus_key")}</span>
                                            </button>
                                        {/if}
                                    {/if}
                                </PopupMenuButton>
                            </div>
                            {/if}
                        </SortableList.Item>
                    {/each}
                </SortableList.Root>
                {#if profileEntries.length > 1}
                    <div class="flex flex-row items-center gap-1 px-1 pt-1 text-xs text-yellow-300/80">
                        <CaretDownFill class="shrink-0" width="10" height="10" />
                        <span>{t("pages.mods.load_order.highest")}</span>
                    </div>
                {/if}
            </div>
            <!-- Library -->
            <div
                class="flex flex-row gap-1 absolute z-10 transition-all right-0 h-full bg-zinc-900
                    {libraryExtended
                    ? 'w-90'
                    : 'w-6'}"
            >
                <button
                    class="hd2mm-button-nop w-6"
                    title={t("pages.mods.library_button.tip")}
                    onclick={onToggleLibrary}
                    disabled={!libraryEnabled}
                >
                    {#if libraryExtended}
                        <ArrowBarRight class="block m-auto" width="16" height="16" />
                    {:else}
                        <ArrowBarLeft class="block m-auto" width="16" height="16" />
                    {/if}
                </button>
                {#if libraryVisible}
                    <ul class="py-1 flex-1 border-y border-zinc-500 overflow-y-auto">
                        {#each libraryMods as mod, i}
                            <li class="grid grid-cols-[min-content_1fr_min-content] gap-1 text-zinc-300 bg-zinc-800 rounded mb-1 p-1">
                                <button
                                    class="hd2mm-button-nop p-1"
                                    title={t("pages.mods.library.insert_top_button.tip")}
                                    onclick={() => onInsertTop(i)}
                                >
                                    <Arrow90degLeft class="block m-auto" width="16" height="16" />
                                </button>
                                <span class="text-2xl truncate flex flex-row items-center gap-1">
                                    {mod.name}
                                    {#if hasUpdateAvailable(mod.guid)}
                                        <span title={t("pages.mods.update_available_badge.tip")}><CloudArrowDownFill class="text-yellow-300 shrink-0" width="16" height="16" /></span>
                                    {/if}
                                </span>
                                <button
                                    class="hd2mm-button-nop p-1"
                                    title={t("pages.mods.library.delete_button.tip")}
                                    onclick={() => onDelete(i)}
                                >
                                    <Trash3 class="block m-auto" width="16" height="16" />
                                </button>
                                <button
                                    class="hd2mm-button-nop p-1"
                                    title={t("pages.mods.library.insert_bottom_button.tip")}
                                    onclick={() => onInsertBottom(i)}
                                >
                                    <ArrowReturnLeft class="block m-auto" width="16" height="16" />
                                </button>
                                <span class="text-sm truncate">{mod.description}</span>
                                <button
                                    class="hd2mm-button-nop p-1"
                                    title={t("pages.mods.library.update_button.tip")}
                                    onclick={() => onUpdate(i)}
                                    disabled={!hasUpdateAvailable(mod.guid)}
                                >
                                    <Download class="block m-auto" width="16" height="16" />
                                </button>
                            </li>
                        {/each}
                    </ul>
                {/if}
            </div>
        </div>
        <!-- Buttons -->
        <div class="flex flex-row gap-1">
            <button
                class="hd2mm-button"
                title={t("pages.mods.add_button.tip")}
                onclick={onAddMod}
            >
                {t("pages.mods.add_button.text")}
            </button>
            <button
                class="hd2mm-button flex flex-row gap-1 items-center"
                title={t("pages.mods.add_folder_button.tip")}
                onclick={onAddFolder}
            >
                <FolderPlus />
                {t("pages.mods.add_folder_button.text")}
            </button>
            <button
                class="hd2mm-button flex flex-row gap-1 items-center"
                title={t("pages.mods.add_url_button.tip")}
                onclick={onAddUrl}
            >
                <Link45deg />
                {t("pages.mods.add_url_button.text")}
            </button>
            <button
                class="hd2mm-button flex flex-row gap-1 items-center"
                title={t("pages.mods.import_button.tip")}
                onclick={() => onImport()}
                data-testid="import-button"
            >
                <BoxArrowInDown />
                {t("pages.mods.import_button.text")}
            </button>
            <button
                class="hd2mm-button flex flex-row gap-1 items-center"
                title={t("pages.mods.check_updates_button.tip")}
                onclick={onCheckUpdates}
            >
                <ArrowRepeat />
                <!-- Icon-only while "Update all" is shown, so the row fits at the default window size. -->
                {#if updatesAvailableCount === 0}
                    {t("pages.mods.check_updates_button.text")}
                {/if}
            </button>
            {#if updatesAvailableCount > 0}
                <button
                    class="hd2mm-success-button flex flex-row gap-1 items-center whitespace-nowrap"
                    title={t("pages.mods.update_all_button.tip")}
                    onclick={onUpdateAll}
                >
                    <CloudArrowDownFill />
                    {t("pages.mods.update_all_button.text", { count: updatesAvailableCount })}
                </button>
            {/if}
            <div class="flex-1"></div>
            <button
                class="hd2mm-danger-button"
                title={t("pages.mods.purge_button.tip")}
                onclick={onPurge}
            >
                {t("pages.mods.purge_button.text")}
            </button>
            <button
                class="hd2mm-success-button"
                title={t("pages.mods.deploy_button.tip")}
                onclick={onDeploy}
            >
                {t("pages.mods.deploy_button.text")}
            </button>
            <button
                class="hd2mm-button"
                title={t("pages.mods.launch_button.tip")}
                onclick={onLaunch}
            >
                {t("pages.mods.launch_button.text")}
            </button>
        </div>
        <!--Drag overlay-->
        {#if isDragging}
            <div class="absolute z-10 bg-black/30 top-0 left-0 bottom-0 right-0 flex flex-col items-center justify-center">
                <span class="text-2xl text-zinc-300">{t("pages.mods.drop_message")}</span>
                <Download class="text-zinc-300" width="32" height="32" />
            </div>
        {/if}
    </div>
    {#snippet failed(error)}
        <ModsLoadError message={describeError(error)} />
    {/snippet}
    </svelte:boundary>
{:catch ex}
    <ModsLoadError message={describeError(ex)} />
{/await}
