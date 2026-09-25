<script lang="ts">
    import { open } from "@tauri-apps/plugin-dialog";
    import { openPath, openUrl } from "@tauri-apps/plugin-opener";
    import { beforeNavigate, onNavigate } from "$app/navigation";
    import { tick } from "svelte";
    import { toSkipEntry, type SkipEntry } from "$lib/models/settings";
    import type { AfterBrowserInstall } from "$lib/models/settings";
    import { useLocalization } from "$lib/state/localization.svelte";
    import {
        loadSettings, saveSettings, detectGamePath, getDataDir, validateGamePath,
        getBridgeAllowedSites, revokeBridgeSite, repairBrowserIntegration, removeBrowserIntegration,
        repairBrowserIntegrationOne, removeBrowserIntegrationOne,
        getNexusKeyStatus, setNexusApiKey, removeNexusApiKey,
        getNexusSignInStatus, nexusSignIn, nexusCancelSignIn, nexusSignOut,
        type BrowserIntegrationStatus, type NexusKeyStatus, type NexusSignInStatus
    } from "$lib/utils/commands";
    import { Dash, Plus, ThreeDots, Search, Folder2Open, ArrowRepeat, BoxArrowUpRight } from "svelte-bootstrap-icons";
    import { usePopup } from "$lib/state/popup.svelte";
    import { InputPopup, NotificationPopup } from "$lib/types/popup";
    import Select from "$lib/components/Select.svelte";
    import ToggleSwitch from "$lib/components/ToggleSwitch.svelte";

    const { t } = useLocalization();
    const { show: showPopup } = usePopup();

    const ONE_CLICK_INSTALL_DOCS_URL = "https://github.com/katsyk/DemocracyDefenderModManager/blob/main/docs/using/one-click-install.md";
    const NEXUS_API_KEYS_URL = "https://next.nexusmods.com/settings/api-keys";
    const DEFAULT_AUTO_CHECK_INTERVAL_HOURS = 6;
    const AFTER_BROWSER_INSTALL_OPTIONS: AfterBrowserInstall[] = ["library", "profile", "deploy"];

    let gamePath = $state<string>("");
    let downloadsPath = $state<string>("");
    let skipList = $state<SkipEntry[]>([]);
    let selectedSkipIndex = $state<number>(-1);
    let afterBrowserInstallIndex = $state<number>(2);
    let bridgeAllowedSites = $state<string[]>([]);
    let autoImportEnabled = $state<boolean>(false);
    // Mod updates: both automatic options are opt-in (off by default).
    let autoCheckUpdates = $state<boolean>(false);
    let autoCheckIntervalEnabled = $state<boolean>(false);
    let autoCheckIntervalHours = $state<number>(DEFAULT_AUTO_CHECK_INTERVAL_HOURS);
    let nexusStatus = $state<NexusKeyStatus>({ Present: false });
    let nexusKeyInput = $state<string>("");
    let nexusBusy = $state<boolean>(false);
    let nexusError = $state<string | undefined>();
    let signIn = $state<NexusSignInStatus>({ Available: false, SignedIn: false, Port: 28647 });
    let signingIn = $state<boolean>(false);
    let signInBusy = $state<boolean>(false);
    let signInError = $state<string | undefined>();
    let browserIntegration = $state<BrowserIntegrationStatus[]>([]);
    let browserIntegrationBusy = $state<boolean>(false);
    let gamePathErrors = $state<string[]>([]);
    /** The folder that will actually be used, when it differs from what
     * was typed (e.g. the user picked `Helldivers 2/data`). */
    let resolvedGamePath = $state<string | null>(null);

    /** Translated message for a backend `GamePathReport.code`. */
    function gamePathErrorText(code: string | null, detail: string | null): string {
        switch (code) {
            case "empty": return t("pages.settings.validation_error.game_path.empty");
            case "not_found": return t("pages.settings.validation_error.game_path.exists");
            case "not_a_directory": return t("pages.settings.validation_error.game_path.not_a_directory");
            case "portal_path": return t("pages.settings.validation_error.game_path.portal_path");
            case "unreadable": return t("pages.settings.validation_error.game_path.unreadable", { detail: detail ?? "" });
            case "missing_tools": return t("pages.settings.validation_error.game_path.tools_exists");
            case "missing_data": return t("pages.settings.validation_error.game_path.data_exists");
            case "missing_bin": return t("pages.settings.validation_error.game_path.bin_exists");
            case "missing_exe": return t("pages.settings.validation_error.game_path.exe_exists");
            default: return t("pages.settings.validation_error.game_path.invalid");
        }
    }
    let dataDir = $state<string>("");
    let initPromise = $state<Promise<void>>(init());
    // Linked from the Mods page ("add a Nexus API key"): scroll there once
    // the page has actually rendered (it only renders after init).
    initPromise.then(async () => {
        if (location.hash !== "#nexus-api-key") return;
        await tick();
        document.getElementById("nexus-api-key")?.scrollIntoView({ block: "start" });
    }).catch(() => {});

    $effect(() => {
        const current = {
            gamePath,
            skipList
        };
        let cancelled = false;

        const validationPromise = async () => {
            const errors = [];
            let resolved: string | null = null;

            // Checked on the backend: the fs plugin's scope can't see paths
            // under dot-directories on Linux (~/.local/share/Steam), which
            // made every default Linux install look invalid.
            try {
                const report = await validateGamePath(current.gamePath ?? "");
                if (report.valid) {
                    resolved = report.resolvedPath;
                } else {
                    errors.push(gamePathErrorText(report.code, report.detail));
                }
            } catch (ex: unknown) {
                errors.push(t("pages.settings.validation_error.game_path.check_failed", { detail: String(ex) }));
            }

            if (!cancelled) {
                gamePathErrors = errors;
                resolvedGamePath = resolved;
            }
        };

        validationPromise();
        return () => { cancelled = true; }
    });

    beforeNavigate(({ cancel }) => {
        if (gamePathErrors.length === 0) return;
        cancel();
        showPopup(new NotificationPopup(
            "error",
            t("pages.settings.popup.notification.validation_error.message")
        ));
    });

    onNavigate(async () => {
        await saveSettings({
            Version: "V1",
            GamePath: resolvedGamePath ?? gamePath,
            SkipList: skipList,
            DownloadsPath: downloadsPath,
            AfterBrowserInstall: AFTER_BROWSER_INSTALL_OPTIONS[afterBrowserInstallIndex] ?? "deploy",
            BridgeAllowedSites: bridgeAllowedSites,
            AutoImportEnabled: autoImportEnabled,
            AutoCheckUpdates: autoCheckUpdates,
            AutoCheckIntervalHours: autoCheckIntervalEnabled ? clampInterval(autoCheckIntervalHours) : 0
        });
    })

    async function init() {
        const [settings, resolvedDataDir] = await Promise.all([loadSettings(), getDataDir()]);
        switch (settings.Version) {
            case "V1":
                gamePath = settings.GamePath;
                skipList = settings.SkipList;
                downloadsPath = settings.DownloadsPath;
                afterBrowserInstallIndex = Math.max(0, AFTER_BROWSER_INSTALL_OPTIONS.indexOf(settings.AfterBrowserInstall));
                bridgeAllowedSites = settings.BridgeAllowedSites;
                autoImportEnabled = settings.AutoImportEnabled;
                autoCheckUpdates = settings.AutoCheckUpdates ?? false;
                autoCheckIntervalEnabled = (settings.AutoCheckIntervalHours ?? 0) > 0;
                autoCheckIntervalHours = autoCheckIntervalEnabled
                    ? clampInterval(settings.AutoCheckIntervalHours)
                    : DEFAULT_AUTO_CHECK_INTERVAL_HOURS;
                break;
        }
        dataDir = resolvedDataDir;

        try {
            nexusStatus = await getNexusKeyStatus();
        } catch {
            // Show as "no key"; never blocks Settings.
        }
        try {
            signIn = await getNexusSignInStatus();
        } catch {
            // Show as "not signed in"; never blocks Settings.
        }


        // Also registers (idempotently) as a side effect -- see
        // repairBrowserIntegration's doc comment on the Rust side. Never
        // fatal to the rest of Settings loading if it fails.
        try {
            browserIntegrationBusy = true;
            browserIntegration = await repairBrowserIntegration();
        } catch {
            // Leave browserIntegration empty; the section just shows
            // nothing rather than blocking Settings from loading.
        } finally {
            browserIntegrationBusy = false;
        }
    }

    function clampInterval(hours: number): number {
        if (!Number.isFinite(hours)) return DEFAULT_AUTO_CHECK_INTERVAL_HOURS;
        return Math.min(168, Math.max(1, Math.round(hours)));
    }

    async function onSaveNexusKey() {
        const key = nexusKeyInput.trim();
        if (!key) return;
        nexusBusy = true;
        nexusError = undefined;
        try {
            nexusStatus = await setNexusApiKey(key);
            nexusKeyInput = "";
        } catch (ex: unknown) {
            nexusError = ex instanceof Error ? ex.message : String(ex);
        } finally {
            nexusBusy = false;
        }
    }

    async function onNexusSignIn() {
        signingIn = true;
        signInError = undefined;
        try {
            signIn = await nexusSignIn();
        } catch (ex: unknown) {
            signInError = ex instanceof Error ? ex.message : String(ex);
        } finally {
            signingIn = false;
        }
    }

    async function onNexusCancelSignIn() {
        try {
            await nexusCancelSignIn();
        } catch {
            // The sign-in call itself reports how it ended.
        }
    }

    async function onNexusSignOut() {
        signInBusy = true;
        signInError = undefined;
        try {
            signIn = await nexusSignOut();
        } catch (ex: unknown) {
            signInError = ex instanceof Error ? ex.message : String(ex);
        } finally {
            signInBusy = false;
        }
    }

    async function onRemoveNexusKey() {
        nexusBusy = true;
        nexusError = undefined;
        try {
            nexusStatus = await removeNexusApiKey();
        } catch (ex: unknown) {
            nexusError = ex instanceof Error ? ex.message : String(ex);
        } finally {
            nexusBusy = false;
        }
    }

    async function onRevokeSite(site: string) {
        await revokeBridgeSite(site);
        bridgeAllowedSites = bridgeAllowedSites.filter(s => s !== site);
    }

    function replaceBrowserStatus(status: BrowserIntegrationStatus) {
        const i = browserIntegration.findIndex(b => b.browserId === status.browserId);
        if (i !== -1) browserIntegration[i] = status;
    }

    async function onRepairBrowser(browserId: string) {
        browserIntegrationBusy = true;
        try {
            const status = await repairBrowserIntegrationOne(browserId);
            if (status) replaceBrowserStatus(status);
        } finally {
            browserIntegrationBusy = false;
        }
    }

    async function onRemoveBrowser(browserId: string) {
        browserIntegrationBusy = true;
        try {
            await removeBrowserIntegrationOne(browserId);
            const current = browserIntegration.find(b => b.browserId === browserId);
            if (current) {
                replaceBrowserStatus({ ...current, registered: false, detail: t("pages.settings.browser_integration.removed_detail") });
            }
        } finally {
            browserIntegrationBusy = false;
        }
    }

    async function onRepairAllBrowsers() {
        browserIntegrationBusy = true;
        try {
            browserIntegration = await repairBrowserIntegration();
        } finally {
            browserIntegrationBusy = false;
        }
    }

    async function onRemoveAllBrowsers() {
        browserIntegrationBusy = true;
        try {
            await removeBrowserIntegration();
            browserIntegration = browserIntegration.map(b => (
                { ...b, registered: false, detail: t("pages.settings.browser_integration.removed_detail") }
            ));
        } finally {
            browserIntegrationBusy = false;
        }
    }

    async function onGetExtension() {
        await openUrl(ONE_CLICK_INSTALL_DOCS_URL);
    }

    async function onBrowse() {
        const path = await open({
            directory: true,
            multiple: false,
        });
        if (!path) return;
        gamePath = path;
    }

    async function onAutoDetect() {
        const detected = await detectGamePath();
        if (detected) {
            gamePath = detected;
        } else {
            showPopup(new NotificationPopup(
                "warning",
                t("pages.settings.popup.notification.game_path_not_found.message"),
            ));
        }
    }

    async function onOpenDataDir() {
        await openPath(dataDir);
    }

    async function onBrowseDownloads() {
        const path = await open({
            directory: true,
            multiple: false,
        });
        if (!path) return;
        downloadsPath = path;
    }

    async function onAddSkipEntry() {
        const input = await showPopup(new InputPopup(
            t("pages.settings.popup.input.add_skip.placeholder"),
            false,
            16,
            16,
            /^[0-9a-f]+$/
        ));
        if (!input) return;
        skipList.push(toSkipEntry(input));
    }

    function onRemoveSkipEntry() {
        if (selectedSkipIndex < 0 || selectedSkipIndex >= skipList.length) return;
        skipList.splice(selectedSkipIndex, 1);
        selectedSkipIndex = -1;
    }
</script>

{#await initPromise}
    <div class="w-full h-full flex flex-col justify-center items-center">
        <div
            class="p-4 bg-zinc-800 border-2 border-zinc-500 flex flex-col gap-1 items-center"
        >
            <div
                class="w-6 h-6 rounded-full border-4 border-transparent border-b-yellow-300 animate-spin"
            ></div>
            <span class="text-zinc-300">{t("pages.settings.loading.text")}</span>
        </div>
    </div>
{:then _}
    <div class="w-full h-full flex flex-col justify-stretch">
        <h1 class="text-zinc-300 text-2xl font-blockletter self-center">{t("pages.settings.title")}</h1>
        <div class="pr-1 flex-1 flex flex-col gap-2 overflow-y-scroll">
            <div class="flex flex-col gap-1">
                <h2 class="text-zinc-300 text-xl">{t("pages.settings.game_path.title")}</h2>
                <div class="flex flex-row gap-1">
                    <input
                        bind:value={gamePath}
                        id="gamepath"
                        class="hd2mm-input flex-1"
                        placeholder={t("pages.settings.game_path.placeholder")}
                        autocomplete="off"
                        autocorrect="off"
                        autocapitalize="off"
                        spellcheck="false"
                    />
                    <button
                        class="hd2mm-button"
                        title={t("pages.settings.game_path.browse_button.tip")}
                        onclick={onBrowse}
                    >
                        <ThreeDots class="m-auto block" />
                    </button>
                    <button
                        class="hd2mm-button flex flex-row gap-1 items-center"
                        title={t("pages.settings.game_path.detect_button.tip")}
                        onclick={onAutoDetect}
                    >
                        <Search />
                        {t("pages.settings.game_path.detect_button.text")}
                    </button>
                </div>
                <ul class="ml-6 text-red-500 list-disc">
                    {#each gamePathErrors as error}
                        <li>{error}</li>
                    {/each}
                </ul>
                {#if gamePathErrors.length === 0 && resolvedGamePath && resolvedGamePath !== gamePath}
                    <p class="text-zinc-400 text-sm">
                        {t("pages.settings.game_path.resolved_hint", { path: resolvedGamePath })}
                    </p>
                {/if}
            </div>
            <div class="flex flex-col gap-1">
                <h2 class="text-zinc-300 text-xl">{t("pages.settings.data_dir.title")}</h2>
                <p class="text-zinc-400 text-sm">{t("pages.settings.data_dir.description")}</p>
                <div class="flex flex-row gap-1">
                    <input
                        value={dataDir}
                        id="datadir"
                        class="hd2mm-input flex-1"
                        readonly
                    />
                    <button
                        class="hd2mm-button flex flex-row gap-1 items-center"
                        title={t("pages.settings.data_dir.open_button.tip")}
                        onclick={onOpenDataDir}
                    >
                        <Folder2Open />
                        {t("pages.settings.data_dir.open_button.text")}
                    </button>
                </div>
            </div>
            <div class="flex flex-col gap-1">
                <h2 class="text-zinc-300 text-xl">{t("pages.settings.downloads_path.title")}</h2>
                <p class="text-zinc-400 text-sm">{t("pages.settings.downloads_path.description")}</p>
                <div class="flex flex-row gap-1">
                    <input
                        bind:value={downloadsPath}
                        id="downloadspath"
                        class="hd2mm-input flex-1"
                        placeholder={t("pages.settings.downloads_path.placeholder")}
                        autocomplete="off"
                        autocorrect="off"
                        autocapitalize="off"
                        spellcheck="false"
                    />
                    <button
                        class="hd2mm-button"
                        title={t("pages.settings.downloads_path.browse_button.tip")}
                        onclick={onBrowseDownloads}
                    >
                        <ThreeDots class="m-auto block" />
                    </button>
                </div>
            </div>
            <div class="flex flex-col gap-1 self-start">
                <h2 class="text-zinc-300 text-xl">{t("pages.settings.skip_list.title")}</h2>
                <ol class="w-80 h-40 border-2 border-zinc-500 overflow-y-scroll">
                    {#each skipList as entry, i (entry)}
                        <li>
                            <button
                                class="w-full px-1 text-zinc-300 font-mono text-start"
                                class:bg-yellow-300={selectedSkipIndex === i}
                                class:text-zinc-900={selectedSkipIndex === i}
                                onclick={() => selectedSkipIndex = i}
                            >
                                {entry}
                            </button>
                        </li>
                    {/each}
                </ol>
                <div class="flex flex-row gap-1 justify-end">
                    <button
                        class="hd2mm-button"
                        onclick={onAddSkipEntry}
                    >
                        <Plus class="m-auto block" />
                    </button>
                    <button
                        class="hd2mm-button"
                        disabled={selectedSkipIndex < 0 || selectedSkipIndex >= skipList.length}
                        onclick={onRemoveSkipEntry}
                    >
                        <Dash class="m-auto block" />
                    </button>
                </div>
            </div>
            <div class="flex flex-col gap-1">
                <h2 class="text-zinc-300 text-xl">{t("pages.settings.updates.title")}</h2>
                <p class="text-zinc-400 text-sm max-w-lg">{t("pages.settings.updates.description")}</p>
                <div class="flex flex-row gap-2 items-center">
                    <ToggleSwitch bind:checked={autoCheckUpdates} />
                    <span class="text-zinc-300 text-sm">{t("pages.settings.updates.auto_check.label")}</span>
                </div>
                <div class="flex flex-row gap-2 items-center ml-6" class:opacity-50={!autoCheckUpdates}>
                    <ToggleSwitch bind:checked={autoCheckIntervalEnabled} disabled={!autoCheckUpdates} />
                    <span class="text-zinc-300 text-sm">{t("pages.settings.updates.interval.label_before")}</span>
                    <input
                        type="number"
                        min="1"
                        max="168"
                        class="hd2mm-input w-16"
                        bind:value={autoCheckIntervalHours}
                        disabled={!autoCheckUpdates || !autoCheckIntervalEnabled}
                        onchange={() => (autoCheckIntervalHours = clampInterval(autoCheckIntervalHours))}
                    />
                    <span class="text-zinc-300 text-sm">{t("pages.settings.updates.interval.label_after")}</span>
                </div>
                <p class="text-zinc-500 text-xs max-w-lg">{t("pages.settings.updates.auto_check.description")}</p>

                <h3 id="nexus-api-key" class="text-zinc-300 text-base mt-2">{t("pages.settings.updates.nexus.title")}</h3>
                <p class="text-zinc-400 text-sm max-w-lg">{t("pages.settings.updates.nexus.description")}</p>
                <div id="nexus-sign-in" class="flex flex-col gap-1 max-w-lg">
                    {#if signIn.SignedIn}
                        <p class="text-sm text-green-400">
                            {signIn.Username
                                ? t("pages.settings.updates.nexus.sign_in.signed_in", { name: signIn.Username })
                                : t("pages.settings.updates.nexus.sign_in.signed_in_unknown")}
                        </p>
                        <p class="text-zinc-400 text-xs">
                            {signIn.Storage === "File"
                                ? t("pages.settings.updates.nexus.sign_in.stored_file")
                                : t("pages.settings.updates.nexus.sign_in.stored_keychain")}
                        </p>
                        {#if nexusStatus.Present}
                            <p class="text-zinc-500 text-xs">{t("pages.settings.updates.nexus.sign_in.key_not_used")}</p>
                        {/if}
                        <div class="flex flex-row gap-1">
                            <button class="hd2mm-button" disabled={signInBusy} onclick={onNexusSignOut}>
                                {t("pages.settings.updates.nexus.sign_in.sign_out_button.text")}
                            </button>
                        </div>
                    {:else if !signIn.Available}
                        <div class="flex flex-row gap-2 items-center">
                            <button class="hd2mm-button" disabled title={t("pages.settings.updates.nexus.sign_in.coming_soon_tip")}>
                                {t("pages.settings.updates.nexus.sign_in.button.text")}
                            </button>
                            <span class="text-zinc-400 text-xs italic">{t("pages.settings.updates.nexus.sign_in.coming_soon")}</span>
                        </div>
                    {:else if signingIn}
                        <p class="text-zinc-300 text-sm">{t("pages.settings.updates.nexus.sign_in.waiting")}</p>
                        <div class="flex flex-row gap-1">
                            <button class="hd2mm-button" onclick={onNexusCancelSignIn}>
                                {t("pages.settings.updates.nexus.sign_in.cancel_button.text")}
                            </button>
                        </div>
                    {:else}
                        <div class="flex flex-row gap-1">
                            <button
                                class="hd2mm-button flex flex-row gap-1 items-center"
                                title={t("pages.settings.updates.nexus.sign_in.button.tip")}
                                onclick={onNexusSignIn}
                            >
                                <BoxArrowUpRight />
                                {t("pages.settings.updates.nexus.sign_in.button.text")}
                            </button>
                        </div>
                    {/if}
                    {#if signInError}
                        <p class="text-red-500 text-sm">{signInError}</p>
                    {/if}
                </div>
                <details class="max-w-lg" open={nexusStatus.Present && !signIn.SignedIn}>
                    <summary class="text-zinc-300 text-sm cursor-pointer select-none">{t("pages.settings.updates.nexus.manual.summary")}</summary>
                    <div class="flex flex-col gap-1 mt-1">
                        <p class="text-zinc-500 text-xs max-w-lg">{t("pages.settings.updates.nexus.where_to_get")}</p>
                        {#if nexusStatus.Present}
                            <p class="text-sm text-green-400">
                                {nexusStatus.Username
                                    ? t("pages.settings.updates.nexus.connected", { name: nexusStatus.Username })
                                    : t("pages.settings.updates.nexus.connected_unknown")}
                            </p>
                            <p class="text-zinc-400 text-xs max-w-lg">
                                {nexusStatus.Storage === "File"
                                    ? t("pages.settings.updates.nexus.stored_file")
                                    : t("pages.settings.updates.nexus.stored_keychain")}
                            </p>
                            <div class="flex flex-row gap-1">
                                <button class="hd2mm-button" disabled={nexusBusy} onclick={onRemoveNexusKey}>
                                    {t("pages.settings.updates.nexus.remove_button.text")}
                                </button>
                            </div>
                        {:else}
                            <div class="flex flex-row gap-1 max-w-lg">
                                <input
                                    type="password"
                                    class="hd2mm-input flex-1"
                                    placeholder={t("pages.settings.updates.nexus.placeholder")}
                                    autocomplete="off"
                                    autocorrect="off"
                                    autocapitalize="off"
                                    spellcheck="false"
                                    bind:value={nexusKeyInput}
                                    onkeydown={(e) => { if (e.key === "Enter") onSaveNexusKey(); }}
                                />
                                <button
                                    class="hd2mm-button"
                                    disabled={nexusBusy || nexusKeyInput.trim().length === 0}
                                    onclick={onSaveNexusKey}
                                >
                                    {nexusBusy ? t("pages.settings.updates.nexus.checking") : t("pages.settings.updates.nexus.save_button.text")}
                                </button>
                                <button
                                    class="hd2mm-button flex flex-row gap-1 items-center"
                                    title={t("pages.settings.updates.nexus.open_page_button.tip")}
                                    onclick={() => openUrl(NEXUS_API_KEYS_URL)}
                                >
                                    <BoxArrowUpRight />
                                    {t("pages.settings.updates.nexus.open_page_button.text")}
                                </button>
                            </div>
                        {/if}
                        {#if nexusError}
                            <p class="text-red-500 text-sm max-w-lg">{nexusError}</p>
                        {/if}
                    </div>
                </details>
                <p class="text-zinc-500 text-xs max-w-lg">{t("pages.settings.updates.nexus.privacy")}</p>
            </div>
            <div class="flex flex-col gap-1">
                <h2 class="text-zinc-300 text-xl">{t("pages.settings.browser_install.title")}</h2>
                <p class="text-zinc-400 text-sm">{t("pages.settings.browser_install.description")}</p>
                <div class="flex flex-row gap-2 items-center">
                    <span class="text-zinc-300 text-sm">{t("pages.settings.browser_install.after_install.label")}</span>
                    <Select
                        bind:selectedIndex={afterBrowserInstallIndex}
                        items={AFTER_BROWSER_INSTALL_OPTIONS}
                        class="w-40"
                    >
                        {#snippet renderItem(option)}
                            {#if option === "library"}
                                {t("pages.settings.browser_install.after_install.options.library")}
                            {:else if option === "profile"}
                                {t("pages.settings.browser_install.after_install.options.profile")}
                            {:else}
                                {t("pages.settings.browser_install.after_install.options.deploy")}
                            {/if}
                        {/snippet}
                    </Select>
                </div>
                <div class="flex flex-row gap-2 items-center">
                    <ToggleSwitch bind:checked={autoImportEnabled} />
                    <span class="text-zinc-300 text-sm">{t("pages.settings.browser_install.auto_import.label")}</span>
                </div>
                <p class="text-zinc-500 text-xs max-w-lg">{t("pages.settings.browser_install.auto_import.description")}</p>
                <h3 class="text-zinc-300 text-base mt-1">{t("pages.settings.browser_install.allowed_sites.title")}</h3>
                {#if bridgeAllowedSites.length === 0}
                    <p class="text-zinc-500 text-sm">{t("pages.settings.browser_install.allowed_sites.empty")}</p>
                {:else}
                    <ul class="w-80 border-2 border-zinc-500 max-h-40 overflow-y-scroll">
                        {#each bridgeAllowedSites as site (site)}
                            <li class="flex flex-row items-center justify-between px-1 py-0.5 text-zinc-300 text-sm">
                                <span class="truncate">{site}</span>
                                <button
                                    class="hd2mm-button shrink-0"
                                    title={t("pages.settings.browser_install.allowed_sites.revoke_button.tip")}
                                    onclick={() => onRevokeSite(site)}
                                >
                                    <Dash class="m-auto block" />
                                </button>
                            </li>
                        {/each}
                    </ul>
                {/if}
            </div>
            <div class="flex flex-col gap-1">
                <h2 class="text-zinc-300 text-xl">{t("pages.settings.browser_integration.title")}</h2>
                <p class="text-zinc-400 text-sm">{t("pages.settings.browser_integration.description")}</p>
                <button class="hd2mm-button self-start flex flex-row gap-1 items-center" onclick={onGetExtension}>
                    <BoxArrowUpRight />
                    {t("pages.settings.browser_integration.get_extension_button.text")}
                </button>
                <ul class="w-96 border-2 border-zinc-500 max-h-56 overflow-y-scroll">
                    {#each browserIntegration as browser (browser.browserId)}
                        <li class="flex flex-row items-center gap-2 px-1 py-0.5 text-sm">
                            <span
                                class="w-2.5 h-2.5 rounded-full shrink-0"
                                class:bg-green-500={browser.registered}
                                class:bg-zinc-600={!browser.registered}
                                title={browser.detail}
                            ></span>
                            <span class="flex-1 text-zinc-300 truncate" title={browser.detail}>{browser.displayName}</span>
                            <button
                                class="hd2mm-button"
                                disabled={browserIntegrationBusy}
                                title={t("pages.settings.browser_integration.repair_button.tip")}
                                onclick={() => onRepairBrowser(browser.browserId)}
                            >
                                <ArrowRepeat class="m-auto block" />
                            </button>
                            <button
                                class="hd2mm-button"
                                disabled={browserIntegrationBusy || !browser.registered}
                                title={t("pages.settings.browser_integration.remove_button.tip")}
                                onclick={() => onRemoveBrowser(browser.browserId)}
                            >
                                <Dash class="m-auto block" />
                            </button>
                        </li>
                    {/each}
                </ul>
                <div class="flex flex-row gap-1 justify-end">
                    <button class="hd2mm-button" disabled={browserIntegrationBusy} onclick={onRepairAllBrowsers}>
                        {t("pages.settings.browser_integration.repair_all_button.text")}
                    </button>
                    <button class="hd2mm-button" disabled={browserIntegrationBusy} onclick={onRemoveAllBrowsers}>
                        {t("pages.settings.browser_integration.remove_all_button.text")}
                    </button>
                </div>
            </div>
        </div>
    </div>
{:catch ex}
    <div class="w-full h-full flex justify-center items-center">
        <div class="p-4 bg-zinc-800 border-2 border-zinc-500 flex flex-col">
            <span class="text-red-500 text-xl self-center">
                {t("pages.settings.loading_failed.title")}
            </span>
            <p class="text-zinc-300 text-sm font-mono">{ex.toString()}</p>
        </div>
    </div>
{/await}