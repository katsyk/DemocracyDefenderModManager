<script lang="ts">
    import { onMount } from "svelte";
    import { listen, type UnlistenFn } from "@tauri-apps/api/event";
    import { openUrl } from "@tauri-apps/plugin-opener";
    import PopupBase from "./PopupBase.svelte";
    import { BrowserUpdatePopup } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";

    const { t } = useLocalization();

    let { popup }: { popup: BrowserUpdatePopup } = $props();

    let opened = $state<boolean>(false);

    onMount(() => {
        let unlisten: UnlistenFn | undefined;
        if (popup.extensionActive) {
            (async () => {
                // The extension's "Update with DDMM" lands as a normal bridge
                // install that updates this mod in place.
                unlisten = await listen<{ mod: { guid: string }; updated?: boolean }>("bridge://mod-installed", (e) => {
                    if (e.payload.mod.guid === popup.entry.Guid && e.payload.updated !== false) popup.close("Done");
                });
            })();
        }
        return () => unlisten?.();
    });

    async function onOpenPage() {
        if (!popup.entry.PageUrl) return;
        if (!popup.extensionActive) {
            popup.close("Handoff");
            return;
        }
        try {
            await openUrl(popup.entry.PageUrl);
        } catch {
            // The user can still open it themselves.
        }
        opened = true;
    }
</script>

<PopupBase>
    <span class="text-xl text-yellow-300 font-blockletter self-center">
        {popup.position
            ? t("popup.browser_update.title_bulk", { index: popup.position.index, total: popup.position.total })
            : t("popup.browser_update.title")}
    </span>
    <p class="text-sm min-w-80 max-w-lg">
        {t("popup.browser_update.message", { name: popup.modName, site: popup.entry.DisplayName, version: popup.entry.LatestVersion ?? "?" })}
    </p>
    {#if popup.extensionActive}
        <p class="text-sm max-w-lg text-zinc-300">{t("popup.browser_update.extension_hint")}</p>
        {#if opened}
            <div class="flex flex-row gap-1 items-center self-center">
                <div class="w-4 h-4 rounded-full border-2 border-transparent border-b-yellow-300 animate-spin"></div>
                <span class="text-sm text-zinc-400">{t("popup.browser_update.waiting")}</span>
            </div>
        {/if}
    {:else}
        <p class="text-sm max-w-lg text-zinc-300">{t("popup.browser_update.handoff_hint")}</p>
    {/if}
    <div class="flex flex-row gap-2 justify-between flex-wrap">
        <div class="flex flex-row gap-2">
            <button class="hd2mm-success-button" disabled={!popup.entry.PageUrl} onclick={onOpenPage}>
                {popup.extensionActive
                    ? t("popup.browser_update.open_button.text", { site: popup.entry.DisplayName })
                    : t("popup.browser_update.open_and_watch_button.text", { site: popup.entry.DisplayName })}
            </button>
            {#if popup.extensionActive}
                <button class="hd2mm-button" onclick={() => popup.close("Handoff")}>
                    {t("popup.browser_update.use_handoff_button.text")}
                </button>
            {/if}
        </div>
        <div class="flex flex-row gap-2">
            {#if popup.position && popup.position.index < popup.position.total}
                <button class="hd2mm-button" onclick={() => popup.close("Skip")}>{t("popup.browser_update.next_button.text")}</button>
            {/if}
            <button class="hd2mm-button" onclick={() => popup.close("Stop")}>
                {popup.position ? t("popup.browser_update.stop_button.text") : t("popup.browser_update.close_button.text")}
            </button>
        </div>
    </div>
</PopupBase>
