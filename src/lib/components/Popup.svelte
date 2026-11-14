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

    /** The popup shown with its render error instead, and that error:
     * there, Escape is its Close button. */
    let renderFailed: { popup: unknown; message: string } | null = null;

    /** A popup that throws while rendering would otherwise leave its
     * caller waiting for good (e.g. "Checking for updates..."): show the
     * error with a Close button instead, and log it. */
    function onRenderError(ex: unknown) {
        renderFailed = { popup: popup.currentPopup, message: message(ex) };
        log.error(`A popup couldn't be shown (${popup.currentPopup?.constructor?.name ?? "?"}): ${describe(ex)}`).catch(() => {});
    }

    /** Off from an Escape keydown that was acted on until that key is
     * released: holding Escape (or a platform that doesn't flag the
     * repeats) never closes more than the top popup. */
    let escapeArmed = true;

    /** Escape inside an open `<select>` dropdown closes the dropdown. */
    function inOpenDropdown(target: EventTarget | null): boolean {
        const select = target instanceof Element ? target.closest("select") : null;
        if (!select) return false;
        try {
            return select.matches(":open");
        } catch {
            // No `:open` here: such webviews show the dropdown natively,
            // and it takes its own Escape before the page sees it.
            return false;
        }
    }

    /** Escape: the top popup's safe choice (Cancel, No, Close, OK), as
     * if clicked; nothing on one with no safe way out (see
     * `Popup.dismiss`). Only the top popup, once per key press, and never
     * the page behind it. */
    function onKeydown(e: KeyboardEvent) {
        if (e.key !== "Escape" || !popup.isShown) return;
        if (e.defaultPrevented || e.isComposing || inOpenDropdown(e.target)) return;
        e.preventDefault();
        e.stopPropagation();
        if (e.repeat || !escapeArmed) return;
        escapeArmed = false;
        const top = popup.currentPopup;
        if (renderFailed?.popup === top) top.closeAfterError(renderFailed.message);
        else top.dismiss();
    }

    function onKeyup(e: KeyboardEvent) {
        if (e.key === "Escape") escapeArmed = true;
    }
</script>

<svelte:window onkeydown={onKeydown} onkeyup={onKeyup} onblur={() => (escapeArmed = true)} />
<!-- Hidden (minimized) with Escape down: its keyup may never come. -->
<svelte:document onvisibilitychange={() => (escapeArmed = true)} />

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
