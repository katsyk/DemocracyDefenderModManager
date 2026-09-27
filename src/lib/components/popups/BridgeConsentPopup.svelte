<script lang="ts">
    import PopupBase from "./PopupBase.svelte";
    import { BridgeConsentPopup } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";
    const { t } = useLocalization();

    let { popup }: { popup: BridgeConsentPopup } = $props();
</script>

<PopupBase>
    <span class="text-xl text-yellow-300 font-blockletter self-center">{t("popup.bridge_consent.title")}</span>
    {#if popup.site}
        <p class="min-w-40 text-sm">{t("popup.bridge_consent.question", { site: popup.site })}</p>
    {:else}
        <!-- No website to remember: asked every time, no "Always allow". -->
        <p class="min-w-40 text-sm">{t("popup.bridge_consent.question_no_site", { file: popup.fileName })}</p>
    {/if}
    <div class="flex flex-col gap-1">
        {#if popup.site}
            <button class="hd2mm-button" onclick={() => popup.close("AlwaysAllow")}>
                {t("popup.bridge_consent.always_allow_button.text")}
            </button>
        {/if}
        <button class="hd2mm-button" onclick={() => popup.close("JustOnce")}>
            {t("popup.bridge_consent.just_once_button.text")}
        </button>
        <button class="hd2mm-button" onclick={() => popup.close("Deny")}>
            {t("popup.bridge_consent.deny_button.text")}
        </button>
    </div>
</PopupBase>
