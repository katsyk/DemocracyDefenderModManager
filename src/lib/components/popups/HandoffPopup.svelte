<script lang="ts">
    import { onMount } from "svelte";
    import { openUrl } from "@tauri-apps/plugin-opener";
    import { open } from "@tauri-apps/plugin-dialog";
    import { listen } from "@tauri-apps/api/event";
    import PopupBase from "./PopupBase.svelte";
    import { HandoffPopup, type HandoffView } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";
    import { startHandoff, cancelHandoff, installHandoffFile, rawModToMod } from "$lib/utils/commands";
    import type { Manifest } from "$lib/models/manifest";
    import type { ResolvedSource } from "$lib/models/mod";

    const { t } = useLocalization();

    let { popup }: { popup: HandoffPopup } = $props();

    type HandoffEventPayload = {
        Status: HandoffView["status"] | "Done" | "Cancelled" | "TimedOut";
        Mod?: { Manifest: Manifest, Directory: string, Sources?: ResolvedSource[] };
        Warning?: string;
        Message?: string;
        /** A download left alone because it's another Nexus mod's file. */
        Ignored?: { FileName: string; ModId: string };
    };

    // What's shown lives on the popup object (`popup.view`), not in this
    // component: the component is unmounted while another popup covers it,
    // and must show the same thing (an error, an ignored file) when it's
    // shown again.
    let view = $state<HandoffView>({ status: "Waiting" });

    function update(change: Partial<HandoffView>) {
        Object.assign(popup.view, change);
        view = { ...popup.view };
    }

    onMount(() => {
        view = { ...popup.view };
        popup.onViewChange = () => (view = { ...popup.view });
        if (!popup.started) {
            popup.started = true;
            begin();
        }
        return () => {
            popup.onViewChange = undefined;
        };
    });

    /** Once per popup: listen for the handoff's progress for as long as the
     * popup is open (covered or not), open the page and start the handoff. */
    async function begin() {
        const unlisten = await listen<HandoffEventPayload>("handoff", (event) => {
            const payload = event.payload;
            switch (payload.Status) {
                case "Waiting":
                case "Installing":
                    update({ status: payload.Status, ...(payload.Ignored ? { ignored: payload.Ignored } : {}) });
                    break;
                case "Error":
                    update({ status: "Error", errorMessage: payload.Message });
                    break;
                case "Done":
                    if (payload.Mod) popup.close({ status: "Done", mod: rawModToMod(payload.Mod), warning: payload.Warning });
                    break;
                case "Cancelled":
                    // "Choose File" cancels the handoff itself; its own
                    // install decides how this popup closes.
                    if (!popup.installingChosenFile) popup.close({ status: "Cancelled" });
                    break;
                case "TimedOut":
                    if (!popup.installingChosenFile) popup.close({ status: "TimedOut" });
                    break;
            }
            popup.onViewChange?.();
        });
        // Runs at once if the popup was closed meanwhile.
        popup.promise.finally(unlisten);

        try {
            await openUrl(popup.pageUrl);
        } catch {
            // Non-fatal: the user can still open the link manually.
        }

        try {
            await startHandoff(popup.pageUrl, popup.existingGuid);
        } catch (ex: unknown) {
            update({ status: "Error", errorMessage: ex instanceof Error ? ex.message : String(ex) });
            popup.onViewChange?.();
        }
    }

    async function onCancel() {
        try {
            await cancelHandoff();
        } finally {
            popup.close({ status: "Cancelled" });
        }
    }

    async function onChooseFile() {
        const files = await open({
            multiple: false,
            directory: false,
            filters: [{ name: "Archives", extensions: ["zip", "7z", "rar"] }],
        });
        if (!files) return;

        popup.installingChosenFile = true;
        try {
            await cancelHandoff();
        } catch {
            // Ignore -- we're about to close regardless.
        }

        try {
            const { mod, warning } = await installHandoffFile(files, popup.pageUrl, popup.existingGuid);
            popup.close({ status: "Done", mod, warning });
        } catch (ex: unknown) {
            const message = ex instanceof Error ? ex.message : String(ex);
            popup.close({ status: "Error", message });
        }
    }
</script>

<PopupBase>
    <span class="text-yellow-300 text-xl font-blockletter self-center">{t("popup.handoff.title")}</span>
    {#if view.status === "Error"}
        <p class="min-w-60 max-w-80 text-sm text-red-500">{t("popup.handoff.error_message", { site: popup.siteName })}</p>
        {#if view.errorMessage}
            <pre class="min-w-40 px-2 py-1 bg-zinc-800 rounded text-sm self-start whitespace-pre-wrap font-mono">{view.errorMessage}</pre>
        {/if}
    {:else}
        <p class="min-w-60 max-w-80 text-sm">
            {t("popup.handoff.message", { site: popup.siteName, downloadsPath: popup.downloadsPath })}
        </p>
        {#if view.ignored}
            <p class="min-w-60 max-w-80 text-sm text-yellow-300">
                {t("popup.handoff.ignored_other_mod", { file: view.ignored.FileName, modId: view.ignored.ModId })}
            </p>
        {/if}
        <div class="flex flex-row gap-1 items-center self-center">
            <div class="w-4 h-4 rounded-full border-2 border-transparent border-b-yellow-300 animate-spin"></div>
            <span class="text-sm text-zinc-400">
                {view.status === "Installing" ? t("popup.handoff.status.installing") : t("popup.handoff.status.waiting")}
            </span>
        </div>
    {/if}
    <div class="flex flex-row gap-2 justify-between">
        <button class="hd2mm-button" onclick={onCancel}>{t("popup.handoff.cancel_button.text")}</button>
        <button class="hd2mm-button" onclick={onChooseFile}>{t("popup.handoff.choose_file_button.text")}</button>
    </div>
</PopupBase>
