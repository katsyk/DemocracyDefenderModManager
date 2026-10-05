<script lang="ts">
    import { useLocalization } from "$lib/state/localization.svelte";
    import { openLogFolder } from "$lib/utils/commands";
    import { ArrowRepeat, FolderSymlink } from "svelte-bootstrap-icons";

    const { t } = useLocalization();

    /** Shown instead of the Mods page when it couldn't load or render
     * (issue #71: it used to stay on "Loading..." for good). The error
     * itself is logged by whoever caught it. */
    let { message, beforeReload }: {
        message: string;
        /** Best effort before reloading (e.g. saving the profiles); its
         * errors are ignored, and it may not hold up the reload. */
        beforeReload?: () => Promise<void>;
    } = $props();

    let openError = $state<string | null>(null);
    let reloading = $state(false);

    async function onReload() {
        if (reloading) return;
        reloading = true;
        try {
            await beforeReload?.();
        } catch {
            // Reload anyway: that's the way out of this screen.
        }
        location.reload();
    }

    async function onOpenLogFolder() {
        openError = null;
        try {
            await openLogFolder();
        } catch (ex: unknown) {
            openError = t("pages.mods.loading_failed.open_log_folder_failed", {
                error: typeof ex === "string" ? ex : ex instanceof Error ? ex.message : String(ex),
            });
        }
    }
</script>

<div class="w-full h-full flex justify-center items-center" data-testid="mods-load-error">
    <div class="p-4 bg-zinc-800 border-2 border-zinc-500 flex flex-col gap-2 max-w-160">
        <span class="text-red-500 text-xl self-center">
            {t("pages.mods.loading_failed.title")}
        </span>
        <p class="text-zinc-300 text-sm">{t("pages.mods.loading_failed.message")}</p>
        <pre class="px-2 py-1 bg-zinc-900 rounded text-sm text-zinc-300 whitespace-pre-wrap break-all font-mono max-h-60 overflow-auto">{message}</pre>
        {#if openError}
            <p class="text-red-400 text-sm">{openError}</p>
        {/if}
        <div class="flex flex-row gap-1 justify-end">
            <button class="hd2mm-button flex flex-row gap-1 items-center" onclick={onOpenLogFolder} data-testid="mods-open-log-folder">
                <FolderSymlink />
                {t("pages.mods.loading_failed.open_log_folder_button.text")}
            </button>
            <button class="hd2mm-button flex flex-row gap-1 items-center" onclick={onReload} disabled={reloading} data-testid="mods-reload">
                <ArrowRepeat />
                {t("pages.mods.loading_failed.reload_button.text")}
            </button>
        </div>
    </div>
</div>
