<script lang="ts">
    import { onMount } from "svelte";
    import { open } from "@tauri-apps/plugin-dialog";
    import PopupBase from "./PopupBase.svelte";
    import { ImportPopup } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";
    import { blocked } from "$lib/state/importWizard.svelte";
    import { formatBytes } from "$lib/utils/bytes";
    import type { ImportItem, ImportSource } from "$lib/utils/commands";
    import { FolderSymlink, FolderPlus, Search } from "svelte-bootstrap-icons";

    const { t } = useLocalization();

    let { popup }: { popup: ImportPopup } = $props();
    let w = $derived(popup.wizard);

    let filter = $state("");


    onMount(() => {
        if (popup.started) return;
        popup.started = true;
        if (popup.paths && popup.paths.length > 0) {
            w.scanPaths(popup.paths);
        } else {
            w.detect();
        }
    });

    function sourceLabel(source: ImportSource): string {
        switch (source.Kind) {
            case "ManagerMods": return t("popup.import.source.manager_mods");
            case "ManagerDownloads": return t("popup.import.source.manager_downloads");
            case "Downloads": return t("popup.import.source.downloads");
        }
    }

    async function chooseFolder() {
        const folder = await open({ directory: true, multiple: false });
        if (typeof folder === "string") await w.scanFolder(folder);
    }

    function statusText(item: ImportItem): string {
        const s = item.Status;
        switch (s.Kind) {
            case "New": return t("popup.import.status.new");
            case "Installed": return t("popup.import.status.installed", { name: s.Name });
            case "InstalledAddSource": return t("popup.import.status.installed_add_source", { name: s.Name });
            case "InstalledOtherVersion": return t("popup.import.status.installed_other_version", { name: s.Name });
            case "Duplicate": return t("popup.import.status.duplicate", { name: s.Of });
            case "OlderVersion": return t("popup.import.status.older_version", { name: s.Of });
            case "NotAMod": return t("popup.import.status.not_a_mod");
            case "Unreadable": return t("popup.import.status.unreadable", { reason: s.Reason });
        }
    }

    function statusClass(item: ImportItem): string {
        switch (item.Status.Kind) {
            case "New": return "text-green-400";
            case "InstalledAddSource": return "text-sky-300";
            case "Unreadable": return "text-red-400";
            default: return "text-zinc-400";
        }
    }

    function fileName(path: string): string {
        return path.split(/[\\/]/).pop() ?? path;
    }

    function details(item: ImportItem): string {
        const parts = [fileName(item.Path), formatBytes(item.Size)];
        if (item.Nexus) {
            parts.push(item.Nexus.Version
                ? t("popup.import.nexus_version", { id: item.Nexus.ModId, version: item.Nexus.Version })
                : t("popup.import.nexus", { id: item.Nexus.ModId }));
        }
        if (item.Profile) {
            parts.push(item.Profile.Enabled ? t("popup.import.was_enabled") : t("popup.import.was_disabled"));
        }
        return parts.join(" · ");
    }

    /** New first, then what needs a look, then what's already here or
     * can't be imported; by name within each group. */
    const STATUS_ORDER = ["New", "InstalledAddSource", "InstalledOtherVersion", "OlderVersion", "NotAMod", "Installed", "Duplicate", "Unreadable"];
    let items = $derived(
        [...(w.scan?.Items ?? [])].sort((a, b) =>
            STATUS_ORDER.indexOf(a.Status.Kind) - STATUS_ORDER.indexOf(b.Status.Kind) || a.Name.localeCompare(b.Name, undefined, { numeric: true }))
    );
    let visible = $derived(
        filter.trim().length === 0
            ? items
            : items.filter(i => (i.Name + " " + i.Path).toLowerCase().includes(filter.trim().toLowerCase()))
    );
    let counts = $derived({
        new: items.filter(i => i.Status.Kind === "New").length,
        installed: items.filter(i => ["Installed", "InstalledOtherVersion", "InstalledAddSource"].includes(i.Status.Kind)).length,
        linkable: items.filter(i => i.Status.Kind === "InstalledAddSource").length,
        other: items.filter(i => !["New", "Installed", "InstalledOtherVersion", "InstalledAddSource"].includes(i.Status.Kind)).length,
    });
    let selectedCount = $derived(w.selected.size);
    let tooBig = $derived(w.scan?.FreeBytes != null && w.selectedBytes + 256 * 1024 * 1024 > w.scan.FreeBytes);
    let importPercent = $derived(
        w.progress && w.progress.BytesTotal > 0
            ? Math.min(100, Math.round((w.progress.BytesDone / w.progress.BytesTotal) * 100))
            : 0
    );
    let warningCount = $derived(w.report?.Imported.filter(m => m.Warning).length ?? 0);
    let scanPercent = $derived(w.scanTotal > 0 ? Math.round((w.scanDone / w.scanTotal) * 100) : 0);
</script>

<PopupBase>
    <div class="flex flex-col gap-2 w-[min(46rem,85vw)] min-h-0 max-h-[calc(83vh-1rem)]" data-testid="import-popup">
        <span class="text-xl text-yellow-300 font-blockletter self-center">{t("popup.import.title")}</span>

        {#if w.error}
            <p class="text-sm text-red-400 bg-red-950/40 px-2 py-1 whitespace-pre-wrap" data-testid="import-error">{w.error}</p>
        {/if}

        {#if w.step === "source"}
            <p class="text-sm">{t("popup.import.intro")}</p>
            {#if w.sources === null}
                <p class="text-sm text-zinc-400">{t("popup.import.looking")}</p>
            {:else if w.sources.length === 0}
                <p class="text-sm text-zinc-400">{t("popup.import.none_found")}</p>
            {:else}
                <ul class="flex flex-col border border-zinc-600 divide-y divide-zinc-700" data-testid="import-sources">
                    {#each w.sources as source (source.Path)}
                        <li class="flex flex-row items-center gap-2 px-2 py-1.5">
                            <FolderSymlink class="shrink-0 text-yellow-300" />
                            <div class="flex-1 min-w-0 flex flex-col">
                                <span class="text-sm text-zinc-200">{sourceLabel(source)}</span>
                                <span class="text-xs text-zinc-400 break-all">{source.Count === 1
                                    ? t("popup.import.found_one", { path: source.Path })
                                    : t("popup.import.found", { count: source.Count, path: source.Path })}</span>
                            </div>
                            <button class="hd2mm-button shrink-0" onclick={() => w.scanFolder(source.Path)}>{t("popup.import.scan_button")}</button>
                        </li>
                    {/each}
                </ul>
            {/if}
            <p class="text-xs text-zinc-400">{t("popup.import.folder_hint")}</p>
            <div class="flex flex-row gap-2 justify-between">
                <button class="hd2mm-button flex flex-row gap-1 items-center" onclick={chooseFolder} data-testid="import-choose-folder">
                    <FolderPlus />{t("popup.import.choose_folder")}
                </button>
                <button class="hd2mm-button" onclick={() => popup.close(null)}>{t("popup.import.close_button")}</button>
            </div>
        {:else if w.step === "scanning"}
            <p class="text-sm">
                {w.scanTotal > 0 ? t("popup.import.scanning_progress", { done: w.scanDone, total: w.scanTotal }) : t("popup.import.scanning")}
            </p>
            <div class="w-full h-2 bg-zinc-700 rounded overflow-hidden">
                <div class="h-full bg-yellow-300 transition-all" class:animate-pulse={w.scanTotal === 0} style="width: {w.scanTotal === 0 ? 100 : scanPercent}%"></div>
            </div>
            <button class="hd2mm-button self-end" disabled={w.cancelling} onclick={() => w.cancel()}>{t("popup.import.cancel_button")}</button>
        {:else if w.step === "preview" && w.scan}
            {#if w.scan.Root}
                <p class="text-xs text-zinc-400 break-all">{w.scan.Root}</p>
            {/if}
            {#if items.length === 0}
                <p class="text-sm">{t("popup.import.nothing_found")}</p>
            {:else}
                <p class="text-sm" data-testid="import-summary">
                    {t("popup.import.summary", { total: items.length, new: counts.new, installed: counts.installed, other: counts.other })}
                </p>
                {#if counts.linkable > 0}
                    <p class="text-xs text-sky-300" data-testid="import-linkable">
                        {counts.linkable === 1
                            ? t("popup.import.linkable_one")
                            : t("popup.import.linkable", { count: counts.linkable })}
                    </p>
                {/if}
                {#if w.scan.Truncated}
                    <p class="text-xs text-yellow-300">{t("popup.import.truncated")}</p>
                {/if}
                <div class="flex flex-row flex-wrap gap-1 items-center">
                    <button class="hd2mm-button text-xs" onclick={() => w.selectAll()}>{t("popup.import.select_all")}</button>
                    <button class="hd2mm-button text-xs" onclick={() => w.selectNone()}>{t("popup.import.select_none")}</button>
                    <button class="hd2mm-button text-xs" onclick={() => w.selectNew()}>{t("popup.import.select_new")}</button>
                    <div class="flex-1"></div>
                    <div class="flex flex-row items-center gap-1 bg-zinc-800 border border-zinc-600 px-1">
                        <Search class="text-zinc-400" width="12" height="12" />
                        <input class="bg-transparent text-sm outline-none w-40" placeholder={t("popup.import.filter_placeholder")} bind:value={filter} />
                    </div>
                </div>
                <ul class="min-h-24 shrink overflow-y-auto border border-zinc-600 divide-y divide-zinc-700" data-testid="import-items">
                    {#each visible as item (item.Id)}
                        <li class="flex flex-row gap-2 items-start px-2 py-1 text-sm" class:opacity-60={blocked(item)}>
                            <input
                                type="checkbox"
                                class="mt-1 accent-yellow-300"
                                checked={w.selected.has(item.Id)}
                                disabled={blocked(item)}
                                onchange={(e) => w.toggle(item, (e.currentTarget as HTMLInputElement).checked)}
                                aria-label={item.Name}
                            />
                            <div class="flex-1 min-w-0 flex flex-col">
                                <div class="flex flex-row gap-2 items-baseline min-w-0">
                                    <span class="truncate text-zinc-200">{item.Name}</span>
                                    <span class="text-xs shrink-0 {statusClass(item)} truncate max-w-72" title={statusText(item)}>{statusText(item)}</span>
                                </div>
                                <span class="truncate text-xs text-zinc-500" title={item.Path}>{details(item)}</span>
                            </div>
                        </li>
                    {/each}
                </ul>
                <p class="text-xs" class:text-red-400={tooBig} class:text-zinc-400={!tooBig} data-testid="import-selection">
                    {t("popup.import.selection", { count: selectedCount, size: formatBytes(w.selectedBytes) })}
                    {#if w.scan.FreeBytes != null}
                        · {t("popup.import.free", { size: formatBytes(w.scan.FreeBytes) })}
                    {/if}
                </p>
                {#if popup.profileName}
                    <label class="flex flex-row gap-2 items-start text-sm">
                        <input type="checkbox" class="mt-1 accent-yellow-300" bind:checked={w.addToProfile} />
                        <span>
                            {t("popup.import.add_to_profile", { profile: popup.profileName })}
                            {#if w.scan.ProfileName}
                                <span class="text-xs text-zinc-400 block">{t("popup.import.keeps_order", { profile: w.scan.ProfileName })}</span>
                            {:else if items.some(i => i.Profile)}
                                <span class="text-xs text-zinc-400 block">{t("popup.import.keeps_order_unnamed")}</span>
                            {/if}
                        </span>
                    </label>
                {/if}
                <p class="text-xs text-zinc-500">{t("popup.import.read_only_note")}</p>
            {/if}
            <div class="flex flex-row gap-2 justify-between">
                {#if popup.paths}
                    <button class="hd2mm-button" onclick={() => popup.close(null)}>{t("popup.import.close_button")}</button>
                {:else}
                    <button class="hd2mm-button" onclick={() => w.back()}>{t("popup.import.back_button")}</button>
                {/if}
                <button class="hd2mm-success-button" disabled={selectedCount === 0} onclick={() => w.start()} data-testid="import-start">
                    {selectedCount === 1 ? t("popup.import.import_button_one") : t("popup.import.import_button", { count: selectedCount })}
                </button>
            </div>
        {:else if w.step === "importing"}
            <p class="text-sm" data-testid="import-progress">
                {#if w.cancelling}
                    {t("popup.import.stopping")}
                {:else if w.progress}
                    {t("popup.import.importing_progress", { done: w.progress.Done + 1 > w.progress.Total ? w.progress.Total : w.progress.Done + 1, total: w.progress.Total })}
                {:else}
                    {t("popup.import.importing")}
                {/if}
            </p>
            {#if w.progress?.Current}
                <p class="text-xs text-zinc-400 truncate">{w.progress.Current}</p>
            {/if}
            <div class="w-full h-2 bg-zinc-700 rounded overflow-hidden">
                <div class="h-full bg-yellow-300 transition-all" style="width: {importPercent}%"></div>
            </div>
            {#if w.progress}
                <span class="text-xs text-zinc-400 self-center">
                    {formatBytes(w.progress.BytesDone)} / {formatBytes(w.progress.BytesTotal)} ({importPercent}%)
                </span>
            {/if}
            <button class="hd2mm-button self-end" disabled={w.cancelling} onclick={() => w.cancel()} data-testid="import-cancel">{t("popup.import.cancel_button")}</button>
        {:else if w.step === "done" && w.report}
            <p class="text-sm" data-testid="import-done">
                {w.report.Imported.length === 1
                    ? t("popup.import.done_imported_one")
                    : t("popup.import.done_imported", { count: w.report.Imported.length })}
                {#if w.report.Linked.length > 0}
                    {w.report.Linked.length === 1
                        ? t("popup.import.done_linked_one")
                        : t("popup.import.done_linked", { count: w.report.Linked.length })}
                {/if}
                {#if w.report.Failed.length > 0}
                    {w.report.Failed.length === 1
                        ? t("popup.import.done_failed_one")
                        : t("popup.import.done_failed", { count: w.report.Failed.length })}
                {/if}
            </p>
            {#if w.report.Cancelled}
                <p class="text-sm text-yellow-300">
                    {w.report.NotStarted === 1
                        ? t("popup.import.done_cancelled_one")
                        : t("popup.import.done_cancelled", { count: w.report.NotStarted })}
                    {#if w.report.RolledBack}
                        {t("popup.import.done_rolled_back", { name: w.report.RolledBack })}
                    {/if}
                </p>
            {/if}
            {#if w.report.Failed.length > 0}
                <ul class="min-h-16 shrink overflow-y-auto border border-zinc-600 divide-y divide-zinc-700" data-testid="import-failures">
                    {#each w.report.Failed as failure (failure.Id)}
                        <li class="flex flex-col px-2 py-1 text-sm">
                            <span class="text-zinc-200">{failure.Name}</span>
                            <span class="text-xs text-red-400 whitespace-pre-wrap break-words">{failure.Reason}</span>
                        </li>
                    {/each}
                </ul>
            {/if}
            {#if warningCount > 0}
                <p class="text-xs text-yellow-300">{warningCount === 1
                    ? t("popup.import.done_warnings_one")
                    : t("popup.import.done_warnings", { count: warningCount })}</p>
            {/if}
            <button
                class="hd2mm-success-button self-end"
                onclick={() => popup.close({ report: w.report!, addToProfile: !!popup.profileName && w.addToProfile })}
                data-testid="import-finish"
            >
                {t("popup.import.finish_button")}
            </button>
        {/if}
    </div>
</PopupBase>
