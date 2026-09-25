<script lang="ts">
    import { onMount } from "svelte";
    import { listen, type UnlistenFn } from "@tauri-apps/api/event";
    import PopupBase from "./PopupBase.svelte";
    import { UpdateDownloadPopup } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";
    import { updateModDirect } from "$lib/utils/commands";

    const { t } = useLocalization();

    let { popup }: { popup: UpdateDownloadPopup } = $props();

    let downloaded = $state(0);
    let total = $state<number | undefined>(undefined);
    let installing = $derived(total !== undefined && downloaded >= total);
    let percent = $derived(total ? Math.min(100, Math.round((downloaded / total) * 100)) : undefined);

    function mb(bytes: number): string {
        return (bytes / (1024 * 1024)).toFixed(1);
    }

    onMount(() => {
        let unlisten: UnlistenFn | undefined;
        (async () => {
            unlisten = await listen<{ Guid: string; Downloaded: number; Total?: number | null }>(
                "updates://progress",
                (e) => {
                    if (e.payload.Guid !== popup.entry.Guid) return;
                    downloaded = e.payload.Downloaded;
                    total = e.payload.Total ?? undefined;
                },
            );
            // The popup can be re-mounted (another popup shown on top and
            // closed again); only ever start the update once per popup, and
            // let the first mount's call close it whenever it finishes.
            if (popup.started) return;
            popup.started = true;
            try {
                const { mod, warning } = await updateModDirect(
                    popup.entry.Guid,
                    popup.entry.Provider,
                    popup.file,
                    popup.entry.LatestVersion ?? null,
                );
                popup.close({ ok: true, mod, warning });
            } catch (ex: unknown) {
                const message = ex instanceof Error ? ex.message : String(ex);
                popup.close({ ok: false, message });
            }
        })();
        return () => {
            unlisten?.();
        };
    });
</script>

<PopupBase>
    <span class="text-xl text-yellow-300 font-blockletter self-center">
        {popup.position
            ? t("popup.update_download.title_bulk", { index: popup.position.index, total: popup.position.total })
            : t("popup.update_download.title")}
    </span>
    <p class="text-sm min-w-80 max-w-lg">
        {t("popup.update_download.message", { name: popup.modName, site: popup.entry.DisplayName, version: popup.entry.LatestVersion ?? "?" })}
    </p>
    <p class="text-xs text-zinc-400 truncate max-w-lg">{popup.file.Label ?? popup.file.Name}</p>
    <div class="w-full h-2 bg-zinc-700 rounded overflow-hidden">
        <div
            class="h-full bg-yellow-300 transition-all"
            class:animate-pulse={percent === undefined}
            style="width: {percent ?? 100}%"
        ></div>
    </div>
    <span class="text-xs text-zinc-400 self-center">
        {#if installing}
            {t("popup.update_download.installing")}
        {:else if total}
            {t("popup.update_download.progress", { done: mb(downloaded), total: mb(total), percent: percent ?? 0 })}
        {:else}
            {t("popup.update_download.progress_unknown", { done: mb(downloaded) })}
        {/if}
    </span>
</PopupBase>
