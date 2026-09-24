<script lang="ts">
    import { onMount, onDestroy } from "svelte";
    import { openUrl } from "@tauri-apps/plugin-opener";
    import { open } from "@tauri-apps/plugin-dialog";
    import { listen, type UnlistenFn } from "@tauri-apps/api/event";
    import PopupBase from "./PopupBase.svelte";
    import { HandoffPopup } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";
    import { startHandoff, cancelHandoff, installHandoffFile, rawModToMod } from "$lib/utils/commands";
    import type { Manifest } from "$lib/models/manifest";
    import type { ResolvedSource } from "$lib/models/mod";

    const { t } = useLocalization();

    let { popup }: { popup: HandoffPopup } = $props();

    type Status = "Waiting" | "Installing" | "Done" | "Cancelled" | "TimedOut" | "Error";
    type HandoffEventPayload = {
        Status: Status;
        Mod?: { Manifest: Manifest, Directory: string, Sources?: ResolvedSource[] };
        Warning?: string;
        Message?: string;
    };

    let status = $state<Status>("Waiting");
    let errorMessage = $state<string | undefined>();
    let unlisten: UnlistenFn | undefined;
    let closed = false;

    function closeOnce(result: Parameters<typeof popup.close>[0]) {
        if (closed) return;
        closed = true;
        popup.close(result);
    }

    onMount(() => {
        (async () => {
            unlisten = await listen<HandoffEventPayload>("handoff", (event) => {
                const payload = event.payload;
                status = payload.Status;

                switch (payload.Status) {
                    case "Done":
                        if (payload.Mod) {
                            closeOnce({ status: "Done", mod: rawModToMod(payload.Mod), warning: payload.Warning });
                        }
                        break;
                    case "Cancelled":
                        closeOnce({ status: "Cancelled" });
                        break;
                    case "TimedOut":
                        closeOnce({ status: "TimedOut" });
                        break;
                    case "Error":
                        errorMessage = payload.Message;
                        break;
                }
            });

            try {
                await openUrl(popup.pageUrl);
            } catch {
                // Non-fatal: the user can still open the link manually.
            }

            try {
                await startHandoff(popup.pageUrl, popup.existingGuid);
            } catch (ex: unknown) {
                status = "Error";
                errorMessage = ex instanceof Error ? ex.message : String(ex);
            }
        })();

        return () => {
            unlisten?.();
        };
    });

    onDestroy(() => {
        unlisten?.();
    });

    async function onCancel() {
        try {
            await cancelHandoff();
        } finally {
            closeOnce({ status: "Cancelled" });
        }
    }

    async function onChooseFile() {
        const files = await open({
            multiple: false,
            directory: false,
            filters: [{ name: "Archives", extensions: ["zip", "7z", "rar"] }],
        });
        if (!files) return;

        try {
            await cancelHandoff();
        } catch {
            // Ignore -- we're about to close regardless.
        }

        try {
            const { mod, warning } = await installHandoffFile(files, popup.pageUrl, popup.existingGuid);
            closeOnce({ status: "Done", mod, warning });
        } catch (ex: unknown) {
            const message = ex instanceof Error ? ex.message : String(ex);
            closeOnce({ status: "Error", message });
        }
    }
</script>

<PopupBase>
    <span class="text-yellow-300 text-xl font-blockletter self-center">{t("popup.handoff.title")}</span>
    {#if status === "Error"}
        <p class="min-w-60 max-w-80 text-sm text-red-500">{t("popup.handoff.error_message", { site: popup.siteName })}</p>
        {#if errorMessage}
            <pre class="min-w-40 px-2 py-1 bg-zinc-800 rounded text-sm self-start whitespace-pre-wrap font-mono">{errorMessage}</pre>
        {/if}
    {:else}
        <p class="min-w-60 max-w-80 text-sm">
            {t("popup.handoff.message", { site: popup.siteName, downloadsPath: popup.downloadsPath })}
        </p>
        <div class="flex flex-row gap-1 items-center self-center">
            <div class="w-4 h-4 rounded-full border-2 border-transparent border-b-yellow-300 animate-spin"></div>
            <span class="text-sm text-zinc-400">
                {status === "Installing" ? t("popup.handoff.status.installing") : t("popup.handoff.status.waiting")}
            </span>
        </div>
    {/if}
    <div class="flex flex-row gap-2 justify-between">
        <button class="hd2mm-button" onclick={onCancel}>{t("popup.handoff.cancel_button.text")}</button>
        <button class="hd2mm-button" onclick={onChooseFile}>{t("popup.handoff.choose_file_button.text")}</button>
    </div>
</PopupBase>
