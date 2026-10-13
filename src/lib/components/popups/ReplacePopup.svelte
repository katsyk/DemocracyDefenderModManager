<script lang="ts">
    import PopupBase from "./PopupBase.svelte";
    import { ReplacePopup } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";
    const { t } = useLocalization();

    let { popup }: { popup: ReplacePopup } = $props();

    const offer = $derived(popup.offer);
    const names = $derived({
        installed: offer.installedName,
        installedVersion: offer.installedVersion ? t("popup.replace.version", { version: offer.installedVersion }) : "",
        new: offer.newName,
        newVersion: offer.newVersion ? t("popup.replace.version", { version: offer.newVersion }) : "",
    });

    /** Cancel is the default: Enter (or a stray click on the focused
     * button) never replaces anything. */
    function focusOnShow(node: HTMLElement) {
        node.focus();
    }
</script>

<PopupBase>
    <span class="text-xl text-yellow-300 font-blockletter self-center">
        {offer.sameName ? t("popup.replace.title") : t("popup.replace.title_different")}
    </span>
    <p class="min-w-60 max-w-120 text-sm" data-testid="replace-question">
        {offer.sameName ? t("popup.replace.question", names) : t("popup.replace.question_different", names)}
    </p>
    {#if !offer.sameName}
        <p class="min-w-60 max-w-120 text-sm text-yellow-300" data-testid="replace-warning">{t("popup.replace.warning_different", names)}</p>
    {/if}
    <p class="min-w-60 max-w-120 text-sm text-zinc-400">{t("popup.replace.keeps")}</p>
    <p class="min-w-60 max-w-120 text-xs text-zinc-500 font-mono break-all">{t("popup.replace.file", { file: offer.fileName })}</p>
    <div class="flex flex-row gap-2 justify-between">
        <button class="hd2mm-button" onclick={() => popup.close(true)}>{t("popup.replace.replace_button.text")}</button>
        <button class="hd2mm-button" use:focusOnShow onclick={() => popup.close(false)}>{t("popup.replace.cancel_button.text")}</button>
    </div>
</PopupBase>
