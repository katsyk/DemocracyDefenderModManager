<script lang="ts">
    import PopupBase from "./PopupBase.svelte";
    import { UpdateFilePickPopup } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";
    import type { UpdateFile } from "$lib/utils/commands";

    const { t } = useLocalization();

    let { popup }: { popup: UpdateFilePickPopup } = $props();

    let files = $derived(popup.entry.Files ?? []);
    let selected = $state<string | undefined>(undefined);
    $effect.pre(() => {
        if (selected === undefined) selected = popup.entry.PreselectedFile;
    });

    function size(file: UpdateFile): string {
        if (file.Size === undefined) return "";
        const mb = file.Size / (1024 * 1024);
        return mb >= 1 ? `${mb.toFixed(1)} MB` : `${Math.max(1, Math.round(file.Size / 1024))} KB`;
    }

    function confirm() {
        const file = files.find(f => f.Id === selected);
        if (file) popup.close(file);
    }
</script>

<PopupBase>
    <span class="text-xl text-yellow-300 font-blockletter self-center">{t("popup.update_file_pick.title")}</span>
    <p class="text-sm max-w-lg">
        {t("popup.update_file_pick.message", { name: popup.modName, site: popup.entry.DisplayName, version: popup.entry.LatestVersion ?? "?" })}
    </p>
    <ul class="min-w-96 max-w-2xl max-h-72 overflow-y-auto border border-zinc-600">
        {#each files as file (file.Id)}
            <li>
                <label class="flex flex-row gap-2 items-center px-2 py-1 text-sm cursor-pointer hover:bg-zinc-800">
                    <input type="radio" name="update-file" value={file.Id} bind:group={selected} />
                    <span class="flex-1 min-w-0 flex flex-col">
                        <span class="truncate text-zinc-200">{file.Label ?? file.Name}</span>
                        {#if file.Label}
                            <span class="truncate text-xs text-zinc-400">{file.Name}</span>
                        {/if}
                    </span>
                    <span class="text-xs text-zinc-400 shrink-0">{size(file)}</span>
                </label>
            </li>
        {/each}
    </ul>
    <div class="flex flex-row gap-2 justify-between">
        <button class="hd2mm-button" onclick={() => popup.close(null)}>{t("popup.update_file_pick.cancel_button.text")}</button>
        <button class="hd2mm-success-button" disabled={!selected} onclick={confirm}>
            {t("popup.update_file_pick.confirm_button.text")}
        </button>
    </div>
</PopupBase>
