<!--
  @component
  A statusbar.
-->

<script lang="ts">
    import { useLocalization } from "$lib/state/localization.svelte";
    import { openUrl } from "@tauri-apps/plugin-opener";
    import { Github } from "svelte-bootstrap-icons";

    const { t } = useLocalization();
    
    function clickHandler(e: MouseEvent) {
        const target = e.target as Element | null;
        if (!target) return;

        const anchor = target.closest("a");
        if (!anchor) return;

        const url = anchor.getAttribute("href");
        if (!url) return;

        e.preventDefault();
        openUrl(url);
    }
</script>

<div class="h-8 flex items-center p-1 bg-zinc-800 gap-2">
    <span
        class="text-zinc-100 bg-zinc-900 rounded-xs px-1.5"
    >
        {t("app.version", [__APP_VERSION__])}
    </span>
    <span class="flex-1"></span>
    <a
        class="flex flex-row gap-1 text-zinc-100 bg-zinc-900 rounded-xs px-1.5 items-center hover:text-blue-300"
        href="https://github.com/katsyk/DemocracyDefenderModManager"
        title="https://github.com/katsyk/DemocracyDefenderModManager"
        onclick={clickHandler}
    >
        <Github class="text-zinc-300" />
        <span class="underline">GitHub</span>
    </a>
</div>
