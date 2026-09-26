<script lang="ts">
    import { open } from "@tauri-apps/plugin-dialog";
    import { ArrowRepeat, Folder2Open, ArrowCounterclockwise, XLg } from "svelte-bootstrap-icons";
    import { useLocalization } from "$lib/state/localization.svelte";
    import { usePopup } from "$lib/state/popup.svelte";
    import { ConfirmPopup } from "$lib/types/popup";
    import {
        retryDataFolder, locateDataFolder, resetDataFolderLocation, forceExit, type DataFolderInfo
    } from "$lib/utils/commands";

    const { t } = useLocalization();
    const { show: showPopup } = usePopup();

    let { info }: { info: DataFolderInfo } = $props();

    let busy = $state(false);
    let error = $state<string | null>(null);
    let restarting = $state(false);

    async function run(action: () => Promise<void>) {
        busy = true;
        error = null;
        try {
            await action();
            restarting = true;
        } catch (ex: unknown) {
            error = ex instanceof Error ? ex.message : String(ex);
        } finally {
            busy = false;
        }
    }

    async function onRetry() {
        await run(retryDataFolder);
    }

    async function onLocate() {
        const folder = await open({ directory: true, multiple: false });
        if (!folder) return;
        await run(() => locateDataFolder(folder));
    }

    async function onReset() {
        const confirmed = await showPopup(new ConfirmPopup(
            t("recovery.data_folder.reset_confirm.title"),
            t("recovery.data_folder.reset_confirm.question", { default: info.DefaultPath }),
        ));
        if (!confirmed) return;
        await run(resetDataFolderLocation);
    }
</script>

<div class="h-full flex justify-center items-center p-4">
    <div class="max-w-2xl flex flex-col gap-3 p-4 border-2 border-zinc-500 bg-zinc-900 text-zinc-300">
        <h1 class="text-2xl text-yellow-300 font-blockletter">{t("recovery.data_folder.title")}</h1>
        {#if info.Problem === "missing"}
            <p>{t("recovery.data_folder.missing_message")}</p>
            <p class="font-mono text-sm break-all bg-zinc-800 p-2">{info.Path}</p>
            <p class="text-sm text-zinc-400">{t("recovery.data_folder.missing_hint")}</p>
        {:else}
            <p>{t("recovery.data_folder.bad_pointer_message")}</p>
            <p class="font-mono text-sm break-all bg-zinc-800 p-2">{info.ProblemDetail ?? info.PointerFile}</p>
        {/if}
        <p class="text-sm text-zinc-400">{t("recovery.data_folder.nothing_changed")}</p>

        {#if restarting}
            <p class="text-yellow-300">{t("recovery.data_folder.restarting")}</p>
        {:else}
            <div class="flex flex-row flex-wrap gap-2">
                <button class="hd2mm-button flex flex-row gap-1 items-center" onclick={onRetry} disabled={busy}>
                    <ArrowRepeat />
                    {t("recovery.data_folder.retry_button")}
                </button>
                <button class="hd2mm-button flex flex-row gap-1 items-center" onclick={onLocate} disabled={busy}>
                    <Folder2Open />
                    {t("recovery.data_folder.locate_button")}
                </button>
                <button class="hd2mm-button flex flex-row gap-1 items-center" onclick={onReset} disabled={busy}>
                    <ArrowCounterclockwise />
                    {t("recovery.data_folder.reset_button")}
                </button>
                <button class="hd2mm-button flex flex-row gap-1 items-center" onclick={() => forceExit()} disabled={busy}>
                    <XLg />
                    {t("recovery.data_folder.quit_button")}
                </button>
            </div>
            <p class="text-xs text-zinc-500">{t("recovery.data_folder.reset_hint", { default: info.DefaultPath })}</p>
        {/if}
        {#if error}
            <p class="text-red-500 text-sm whitespace-pre-wrap">{error}</p>
        {/if}
    </div>
</div>
