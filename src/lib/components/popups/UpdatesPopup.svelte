<script lang="ts">
    import PopupBase from "./PopupBase.svelte";
    import { UpdatesPopup } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";
    import type { UpdateStateKind, UpdateStatusEntry } from "$lib/utils/commands";

    const { t } = useLocalization();

    let { popup }: { popup: UpdatesPopup } = $props();

    const ORDER: UpdateStateKind[] = ["UpdateAvailable", "NeedsApiKey", "NeedsManualCheck", "Error", "Unknown", "Skipped", "UpToDate", "Unsupported"];

    let entries = $derived(
        [...popup.report.Results].sort((a, b) => ORDER.indexOf(a.Status.Kind) - ORDER.indexOf(b.Status.Kind))
    );
    let availableCount = $derived(new Set(
        popup.report.Results.filter(e => e.Status.Kind === "UpdateAvailable").map(e => e.Guid)
    ).size);
    let needsKey = $derived(popup.report.Results.some(e => e.Status.Kind === "NeedsApiKey"));

    function name(entry: UpdateStatusEntry): string {
        return popup.modNames.get(entry.Guid) ?? entry.Guid;
    }

    function versions(entry: UpdateStatusEntry): string {
        const from = entry.InstalledVersion ?? "?";
        const to = entry.LatestVersion ?? "?";
        return entry.Status.Kind === "UpToDate" ? from : `${from} → ${to}`;
    }
</script>

<PopupBase>
    <span class="text-xl text-yellow-300 font-blockletter self-center">{t("popup.updates.title")}</span>
    <p class="text-sm">
        {availableCount > 0
            ? t("popup.updates.summary_available", { count: availableCount })
            : t("popup.updates.summary_none")}
    </p>
    {#if entries.length === 0}
        <p class="text-sm text-zinc-400 max-w-md">{t("popup.updates.nothing_checkable")}</p>
    {:else}
        <ul class="min-w-110 max-w-3xl max-h-80 overflow-y-auto border border-zinc-600 divide-y divide-zinc-700">
            {#each entries as entry (entry.Guid + entry.Provider)}
                <li class="flex flex-row gap-2 items-center px-2 py-1 text-sm">
                    <div class="flex-1 min-w-0 flex flex-col">
                        <span class="truncate text-zinc-200">{name(entry)}</span>
                        <span class="truncate text-xs text-zinc-400">
                            {entry.DisplayName} · {versions(entry)}
                            {#if entry.LatestFileName && entry.Status.Kind === "UpdateAvailable"}
                                · {entry.LatestFileName}
                            {/if}
                        </span>
                    </div>
                    {#if entry.Status.Kind === "UpdateAvailable"}
                        <span class="text-yellow-300 text-xs shrink-0">{t("popup.updates.state.update_available")}</span>
                        <button class="hd2mm-success-button shrink-0" onclick={() => popup.close({ kind: "update", entry })}>
                            {t("popup.updates.update_button.text")}
                        </button>
                        <button
                            class="hd2mm-button shrink-0"
                            title={t("popup.updates.skip_button.tip", { version: entry.LatestVersion ?? "" })}
                            onclick={() => popup.close({ kind: "skip", entry })}
                        >
                            {t("popup.updates.skip_button.text")}
                        </button>
                    {:else if entry.Status.Kind === "Skipped"}
                        <span class="text-zinc-400 text-xs shrink-0">{t("popup.updates.state.skipped", { version: entry.LatestVersion ?? "" })}</span>
                        <button class="hd2mm-button shrink-0" onclick={() => popup.close({ kind: "unskip", entry })}>
                            {t("popup.updates.unskip_button.text")}
                        </button>
                    {:else if entry.Status.Kind === "NeedsApiKey"}
                        <span class="text-zinc-400 text-xs shrink-0 max-w-56 text-right">{t("popup.updates.state.needs_api_key")}</span>
                    {:else if entry.Status.Kind === "NeedsManualCheck"}
                        <span class="text-zinc-400 text-xs shrink-0 max-w-56 text-right">{t("popup.updates.state.needs_manual_check")}</span>
                    {:else if entry.Status.Kind === "Error"}
                        <span class="text-red-400 text-xs shrink-0 max-w-64 text-right truncate" title={entry.Status.message}>
                            {t("popup.updates.state.error", { message: entry.Status.message ?? "" })}
                        </span>
                    {:else if entry.Status.Kind === "Unknown"}
                        <span class="text-zinc-400 text-xs shrink-0" title={t("popup.updates.state.unknown_tip")}>{t("popup.updates.state.unknown")}</span>
                    {:else}
                        <span class="text-green-400 text-xs shrink-0">{t("popup.updates.state.up_to_date")}</span>
                    {/if}
                </li>
            {/each}
        </ul>
    {/if}
    {#if needsKey}
        <p class="text-xs text-zinc-400 max-w-lg">{t("popup.updates.needs_key_hint")}</p>
    {/if}
    {#if popup.report.NexusCheckedRecently}
        <p class="text-xs text-zinc-400 max-w-lg">{t("popup.updates.nexus_checked_recently")}</p>
    {/if}
    <div class="flex flex-row gap-2 justify-between">
        <div class="flex flex-row gap-2">
            {#if availableCount > 0}
                <button class="hd2mm-success-button" onclick={() => popup.close({ kind: "updateAll" })}>
                    {t("popup.updates.update_all_button.text", { count: availableCount })}
                </button>
            {/if}
            {#if needsKey}
                <button class="hd2mm-button" onclick={() => popup.close({ kind: "nexusSettings" })}>
                    {t("popup.updates.nexus_settings_button.text")}
                </button>
            {/if}
        </div>
        <button class="hd2mm-button" onclick={() => popup.close(null)}>{t("popup.updates.close_button.text")}</button>
    </div>
</PopupBase>
