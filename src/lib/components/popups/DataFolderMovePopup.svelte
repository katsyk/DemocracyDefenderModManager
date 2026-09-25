<script lang="ts">
    import PopupBase from "./PopupBase.svelte";
    import { DataFolderMovePopup } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";
    import { formatBytes } from "$lib/utils/bytes";

    const { t } = useLocalization();

    let { popup }: { popup: DataFolderMovePopup } = $props();
    const plan = $derived(popup.plan);
</script>

<PopupBase>
    <span class="text-xl text-yellow-300 font-blockletter self-center">
        {plan.ExistingData
            ? t("popup.data_folder_move.adopt_title")
            : plan.IsReset
                ? t("popup.data_folder_move.reset_title")
                : t("popup.data_folder_move.title")}
    </span>
    <div class="flex flex-col gap-2 text-sm max-w-xl min-w-80">
        {#if plan.ExistingData}
            <p>{t("popup.data_folder_move.adopt_message", { target: plan.Target })}</p>
            <p class="text-zinc-400">{t("popup.data_folder_move.adopt_detail", { source: plan.Source })}</p>
        {:else}
            <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
                <dt class="text-zinc-400">{t("popup.data_folder_move.from")}</dt>
                <dd class="break-all">{plan.Source}</dd>
                <dt class="text-zinc-400">{t("popup.data_folder_move.to")}</dt>
                <dd class="break-all">{plan.Target}</dd>
                <dt class="text-zinc-400">{t("popup.data_folder_move.size")}</dt>
                <dd>{t("popup.data_folder_move.size_value", { size: formatBytes(plan.TotalBytes), files: plan.TotalFiles })}</dd>
                <dt class="text-zinc-400">{t("popup.data_folder_move.free")}</dt>
                <dd>{plan.FreeBytes === null ? t("popup.data_folder_move.free_unknown") : formatBytes(plan.FreeBytes)}</dd>
            </dl>
            {#if plan.UsedSubfolder}
                <p class="text-yellow-300">{t("popup.data_folder_move.subfolder_note", { picked: plan.Picked })}</p>
            {/if}
            <p>{t("popup.data_folder_move.what_happens")}</p>
            <ol class="list-decimal ml-5 text-zinc-300">
                <li>{t("popup.data_folder_move.step_copy")}</li>
                <li>{t("popup.data_folder_move.step_verify")}</li>
                <li>{t("popup.data_folder_move.step_switch")}</li>
                <li>{t("popup.data_folder_move.step_delete")}</li>
                <li>{t("popup.data_folder_move.step_restart")}</li>
            </ol>
            <p class="text-zinc-400">{t("popup.data_folder_move.failure_note")}</p>
        {/if}
    </div>
    <div class="flex flex-row gap-2 justify-between">
        {#if plan.ExistingData}
            <button class="hd2mm-button" onclick={() => popup.close("adopt")}>{t("popup.data_folder_move.adopt_button")}</button>
        {:else}
            <button class="hd2mm-button" onclick={() => popup.close("move")}>{t("popup.data_folder_move.move_button")}</button>
        {/if}
        <button class="hd2mm-button" onclick={() => popup.close(null)}>{t("popup.data_folder_move.cancel_button")}</button>
    </div>
</PopupBase>
