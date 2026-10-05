<script lang="ts">
    import { usePopup } from "$lib/state/popup.svelte";
    import { useLocalization } from "$lib/state/localization.svelte";
    import * as log from "@tauri-apps/plugin-log";
    import PopupBase from "./popups/PopupBase.svelte";

    const popup = usePopup();
    const { t } = useLocalization();

    function describe(ex: unknown): string {
        if (ex instanceof Error) return ex.stack && !ex.stack.includes(ex.message) ? `${ex.message}\n${ex.stack}` : (ex.stack ?? ex.message);
        return typeof ex === "string" ? ex : String(ex);
    }

    function message(ex: unknown): string {
        return ex instanceof Error ? ex.message : String(ex);
    }

    /** A popup that throws while rendering would otherwise leave its
     * caller waiting for good (e.g. "Checking for updates..."): show the
     * error with a Close button instead, and log it. */
    function onRenderError(ex: unknown) {
        log.error(`A popup couldn't be shown (${popup.currentPopup?.constructor?.name ?? "?"}): ${describe(ex)}`).catch(() => {});
    }
</script>

{#if popup.isShown}
    {#key popup.currentPopup}
        <div class="absolute inset-0 z-50 bg-black/20 flex justify-center items-center">
            <svelte:boundary onerror={onRenderError}>
                <svelte:component this={popup.currentPopup.component} popup={popup.currentPopup} />
                {#snippet failed(error)}
                    <PopupBase>
                        <span class="text-red-500 text-xl font-blockletter self-center">{t("popup.render_failed.title")}</span>
                        <p class="min-w-40 max-w-120 text-sm self-start">{t("popup.render_failed.message")}</p>
                        <pre class="min-w-40 max-w-120 max-h-40 overflow-auto px-2 py-1 bg-zinc-800 rounded text-sm self-start whitespace-pre-wrap break-all font-mono" data-testid="popup-render-error">{message(error)}</pre>
                        <button class="hd2mm-button self-end" onclick={() => popup.currentPopup.closeAfterError(message(error))}>{t("popup.render_failed.close_button.text")}</button>
                    </PopupBase>
                {/snippet}
            </svelte:boundary>
        </div>
    {/key}
{/if}
