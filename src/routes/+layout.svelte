<script lang="ts">
    import "../app.css";
    import Titlebar from "$lib/components/Titlebar.svelte";
    import Sidebar from "$lib/components/Sidebar.svelte";
    import Statusbar from "$lib/components/Statusbar.svelte";
    import Popup from "$lib/components/Popup.svelte";
    import Toast from "$lib/components/Toast.svelte";
    import DataFolderRecovery from "$lib/components/DataFolderRecovery.svelte";
    import { onMount } from "svelte";
    import { initLocalization } from "$lib/state/localization.svelte";
    import { checkSettings, getDataFolderInfo, type DataFolderInfo } from "$lib/utils/commands";
    import { goto } from "$app/navigation";
    import type { Snippet } from "svelte";
    import { usePopup } from "$lib/state/popup.svelte";

    let { children }: { children?: Snippet } = $props();

    // Set when the chosen data folder is missing (e.g. an unplugged USB
    // drive): the whole app is replaced by the recovery screen until the
    // user picks what to do. See `commands::data_folder` on the Rust side.
    let recovery = $state<DataFolderInfo | null>(null);
    let ready = $state(false);

    // While a popup is shown, the page under it is inert: the button that
    // opened it keeps the focus otherwise, and Enter or Space would press it
    // again behind the popup (a second deploy, a second "Add URL" popup).
    const popups = usePopup();

    onMount(async () => {
        try {
            await initLocalization("en");
        } catch {
            // Keys show untranslated; still better than a blank window.
        }

        try {
            const info = await getDataFolderInfo();
            if (info.Problem) {
                recovery = info;
                ready = true;
                return;
            }
        } catch {
            // Never block startup on this.
        }
        ready = true;

        if (!await checkSettings()) {
            goto("/settings");
        }
    });
</script>

<div
    class="flex flex-col h-dvh overflow-hidden bg-zinc-900 border-zinc-500 border-4"
>
    <Titlebar />
    <div class="relative flex-1 flex flex-row min-h-0">
        {#if recovery}
            <main class="flex-1 p-2 overflow-auto min-w-0 min-h-0 h-full" inert={popups.isShown}>
                <DataFolderRecovery info={recovery} />
            </main>
        {:else if ready}
            <div class="contents" inert={popups.isShown}>
                <Sidebar />
            </div>
            <main class="flex-1 p-2 overflow-hidden min-w-0 min-h-0 h-full" inert={popups.isShown}>
                {@render children?.()}
            </main>
        {/if}
        <Popup />
        <Toast />
    </div>
    <Statusbar />
</div>
