<script lang="ts">
    import PopupBase from "./PopupBase.svelte";
    import { StartupNoticesPopup } from "$lib/types/popup";
    import { useLocalization } from "$lib/state/localization.svelte";
    import { openStartupNoticeFolder, type StartupNotice } from "$lib/utils/commands";
    import { ExclamationTriangleFill, InfoCircle } from "svelte-bootstrap-icons";

    const { t } = useLocalization();

    let { popup }: { popup: StartupNoticesPopup } = $props();

    /** Why "Open folder" didn't work, per notice. */
    let openErrors = $state<Record<number, string>>({});

    /** Kinds that only say what DDMM did; the rest ask for something. The
     * backend lists these first (`NoticeKind::is_info`). */
    const INFO_KINDS = ["updateFinished", "updateUndone"];
    /** Kinds whose folder is worth showing: one to fix, check or clean up. */
    const OPENABLE_KINDS = ["backupRestored", "updatePending", "adoptLeft", "backupKept", "stagingKept", "deleteKept", "modUnreadable"];

    /** What it says about the mod (or folder), without the mod's name. */
    function body(n: StartupNotice, params: Record<string, string>): string {
        switch (n.kind) {
            case "updateFinished": return t("popup.startup_notices.update_finished", params);
            case "updateUndone": return t("popup.startup_notices.update_undone", params);
            case "backupRestored": return t("popup.startup_notices.backup_restored", params);
            case "profilesRetry": return t("popup.startup_notices.profiles_retry", params);
            case "updatePending": return t("popup.startup_notices.update_pending", params);
            case "adoptLeft": return t("popup.startup_notices.adopt_left", params);
            case "backupKept": return t("popup.startup_notices.backup_kept", params);
            case "stagingKept": return t("popup.startup_notices.staging_kept", params);
            case "deleteKept": return t("popup.startup_notices.delete_kept", params);
            case "modUnreadable": return t("popup.startup_notices.mod_unreadable", params);
            default: return t("popup.startup_notices.unknown", params);
        }
    }

    function text(n: StartupNotice): string {
        const params = {
            name: n.modName ?? "",
            folder: n.folder ?? "",
            profiles: (n.profiles ?? []).join(", "),
        };
        // Its own sentence around the name, which it always has.
        if (n.kind === "optionsReset") return t("popup.startup_notices.options_reset", params);
        const text = body(n, params);
        return n.modName ? t("popup.startup_notices.named", { name: n.modName, text }) : text;
    }

    async function openFolder(i: number, folder: string) {
        try {
            await openStartupNoticeFolder(folder);
            delete openErrors[i];
        } catch (ex: unknown) {
            openErrors[i] = t("popup.startup_notices.open_folder_failed", { error: ex instanceof Error ? ex.message : String(ex) });
        }
    }
</script>

<PopupBase>
    <span class="text-yellow-300 text-xl font-blockletter self-center">{t("popup.startup_notices.title")}</span>
    <p class="text-sm self-start max-w-160">{t("popup.startup_notices.intro")}</p>
    <ul class="min-w-60 max-w-160 max-h-[60vh] flex flex-col items-stretch gap-2 overflow-y-auto overflow-x-hidden" data-testid="startup-notices">
        {#each popup.notices as notice, i}
            <li class="flex flex-row items-start gap-2" data-testid="startup-notice">
                {#if INFO_KINDS.includes(notice.kind)}
                    <InfoCircle class="text-blue-400 shrink-0 mt-0.5" />
                {:else}
                    <ExclamationTriangleFill class="text-yellow-300 shrink-0 mt-0.5" />
                {/if}
                <div class="flex flex-col gap-1 min-w-0">
                    <p class="text-sm whitespace-pre-wrap break-words">{text(notice)}</p>
                    {#if notice.folder && OPENABLE_KINDS.includes(notice.kind)}
                        <button class="hd2mm-button self-start text-sm" onclick={() => openFolder(i, notice.folder!)}>
                            {t("popup.startup_notices.open_folder_button.text")}
                        </button>
                    {/if}
                    {#if openErrors[i]}
                        <p class="text-sm text-red-400 break-words">{openErrors[i]}</p>
                    {/if}
                </div>
            </li>
        {/each}
    </ul>
    <button class="hd2mm-button self-end" onclick={() => popup.close()}>{t("popup.notification.ok_button.text")}</button>
</PopupBase>
