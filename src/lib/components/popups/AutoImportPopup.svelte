<script lang="ts">
    import PopupBase from "./PopupBase.svelte";
    import { AutoImportPopup } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";
    const { t } = useLocalization();

    let { popup }: { popup: AutoImportPopup } = $props();

    let fileName = $derived(popup.file.split(/[/\\]/).pop() ?? popup.file);
</script>

<PopupBase>
    <span class="text-xl text-yellow-300 font-blockletter self-center">{t("popup.auto_import.title")}</span>
    <p class="min-w-40 text-sm">{t("popup.auto_import.question", { file: fileName })}</p>
    <div class="flex flex-col gap-1">
        <button class="hd2mm-button" onclick={() => popup.close("InstallAndDeploy")}>
            {t("popup.auto_import.install_and_deploy_button.text")}
        </button>
        <button class="hd2mm-button" onclick={() => popup.close("Install")}>
            {t("popup.auto_import.install_button.text")}
        </button>
        <button class="hd2mm-button" onclick={() => popup.close("Ignore")}>
            {t("popup.auto_import.ignore_button.text")}
        </button>
    </div>
</PopupBase>
