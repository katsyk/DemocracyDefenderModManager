<script lang="ts">
    import { onMount } from "svelte";
    import { listen, type UnlistenFn } from "@tauri-apps/api/event";
    import PopupBase from "./PopupBase.svelte";
    import { DataFolderProgressPopup } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";
    import { moveDataFolder, type DataFolderMoveProgress } from "$lib/utils/commands";
    import { formatBytes } from "$lib/utils/bytes";

    const { t } = useLocalization();

    let { popup }: { popup: DataFolderProgressPopup } = $props();

    let progress = $state<DataFolderMoveProgress | null>(null);
    let restarting = $state(false);
    let leftovers = $state<string[]>([]);
    let percent = $derived(
        progress && progress.TotalBytes > 0
            ? Math.min(100, Math.round((progress.DoneBytes / progress.TotalBytes) * 100))
            : undefined,
    );

    onMount(() => {
        let unlisten: UnlistenFn | undefined;
        (async () => {
            unlisten = await listen<DataFolderMoveProgress>("data-folder://progress", (e) => {
                progress = e.payload;
            });
            if (popup.started) return;
            popup.started = true;
            try {
                const result = await moveDataFolder(popup.destination, popup.reset);
                // The app restarts by itself in a moment; keep this up
                // until it does.
                leftovers = result.Leftovers;
                restarting = true;
            } catch (ex: unknown) {
                const message = ex instanceof Error ? ex.message : String(ex);
                popup.close({ ok: false, message });
            }
        })();
        return () => unlisten?.();
    });
</script>

<PopupBase>
    <span class="text-xl text-yellow-300 font-blockletter self-center">{t("popup.data_folder_progress.title")}</span>
    <p class="text-sm min-w-80 max-w-lg">
        {#if restarting}
            {t("popup.data_folder_progress.restarting")}
        {:else if progress?.Phase === "verifying"}
            {t("popup.data_folder_progress.verifying")}
        {:else if progress?.Phase === "finishing"}
            {t("popup.data_folder_progress.finishing")}
        {:else}
            {t("popup.data_folder_progress.copying")}
        {/if}
    </p>
    <div class="w-full h-2 bg-zinc-700 rounded overflow-hidden">
        <div
            class="h-full bg-yellow-300 transition-all"
            class:animate-pulse={percent === undefined || restarting}
            style="width: {restarting ? 100 : (percent ?? 100)}%"
        ></div>
    </div>
    {#if progress && !restarting}
        <span class="text-xs text-zinc-400 self-center">
            {t("popup.data_folder_progress.progress", {
                done: formatBytes(progress.DoneBytes),
                total: formatBytes(progress.TotalBytes),
                files: progress.DoneFiles,
                total_files: progress.TotalFiles,
                percent: percent ?? 0,
            })}
        </span>
    {/if}
    {#if leftovers.length > 0}
        <p class="text-xs text-zinc-400 max-w-lg">{t("popup.data_folder_progress.leftovers", { count: leftovers.length })}</p>
    {/if}
    <p class="text-xs text-zinc-500 max-w-lg">{t("popup.data_folder_progress.dont_close")}</p>
</PopupBase>
