<script lang="ts">
    import { onMount } from "svelte";
    import { SvelteMap } from "svelte/reactivity";
    import { FALLBACK_MOD_IMAGE, modImageSrc, useFallbackImage } from "$lib/utils/modImages";
    import type { ModConfigPopup } from "$lib/types/popup";
    import type { v1, v2 } from "$lib/models/manifest";
    import { useLocalization } from "$lib/state/localization.svelte";
    import type { ConfigV1, ConfigV2 } from "$lib/models/profile";
    import type { UUID } from "$lib/types/uuid";
    import PopupBase from "./PopupBase.svelte";
    import Select from "../Select.svelte";

    const { t } = useLocalization();

    let { popup }: { popup: ModConfigPopup } = $props();
    let initPromise = $state<Promise<void>>();
    let toggled = $state<boolean[]>([]);
    let selected = $state<number[]>([]);
    let imagePaths = new SvelteMap<string, string>();

    type AnyOption = v1.Option | v2.Option;
    type Entry = { option: AnyOption; index: number };
    type Group = { category: v2.Category | null; entries: Entry[] };

    onMount(() => initPromise = init());

    async function init() {
        const { config, mod } = popup;

        if (config.Guid !== mod.Manifest.Guid) {
            throw new Error("GUIDs of config and mod do not match!");
        }

        if (!("Version" in mod.Manifest) || (mod.Manifest.Version !== 1 && mod.Manifest.Version !== 2)) {
            throw new Error("Config popup can only handle V1/V2 mods!");
        }
        if ((mod.Manifest.Version === 1 && config.For !== "V1") || (mod.Manifest.Version === 2 && config.For !== "V2")) {
            throw new Error("Config version does not match manifest version!");
        }

        const manifest = mod.Manifest as v1.Manifest | v2.Manifest;
        const typedConfig = config as ConfigV1 | ConfigV2;

        if (!manifest.Options) {
            throw new Error("Manifest has no options!");
        }

        toggled = [...typedConfig.Toggled];
        selected = [...typedConfig.Selected];

        for (const opt of manifest.Options) {
            if (opt.Image) {
                imagePaths.set(opt.Image, await modImageSrc(mod.Directory, opt.Image));
            }
            if (opt.SubOptions) {
                for (const sub of opt.SubOptions) {
                    if (sub.Image) {
                        imagePaths.set(sub.Image, await modImageSrc(mod.Directory, sub.Image));
                    }
                }
            }
        }
    }

    function onClose() {
        const config = (popup.config as ConfigV1 | ConfigV2);
        config.Toggled = toggled;
        config.Selected = selected;
        popup.close(config);
    }

    /**
     * V1 mods (and V2 mods with no declared Categories) render as a single,
     * flat group -- identical to the old V1-only layout. V2 mods with
     * Categories group their options under a heading per category (in
     * declaration order), with anything left over (no CategoryRef, or one
     * that doesn't resolve) falling into a trailing uncategorized group.
     * `index` always refers to the option's position in the manifest's own
     * Options array, since that's what `toggled`/`selected` are keyed by --
     * grouping only changes display order, never that binding.
     */
    function computeGroups(): Group[] {
        const manifest = popup.mod.Manifest as v1.Manifest | v2.Manifest;
        const options = manifest.Options ?? [];
        const entries: Entry[] = options.map((option, index) => ({ option, index }));

        if (manifest.Version !== 2 || !manifest.Categories || manifest.Categories.length === 0) {
            return [{ category: null, entries }];
        }

        const categories = manifest.Categories;
        const byCategory = new Map<UUID, Entry[]>();
        const uncategorized: Entry[] = [];

        for (const entry of entries) {
            const ref = (entry.option as v2.Option).CategoryRef;
            const category = ref ? categories.find(c => c.Guid === ref) : undefined;
            if (!category) {
                uncategorized.push(entry);
                continue;
            }
            const list = byCategory.get(category.Guid) ?? [];
            list.push(entry);
            byCategory.set(category.Guid, list);
        }

        const result: Group[] = categories
            .filter(c => byCategory.has(c.Guid))
            .map(c => ({ category: c, entries: byCategory.get(c.Guid)! }));

        if (uncategorized.length > 0) {
            result.push({ category: null, entries: uncategorized });
        }

        return result;
    }

    let groups = $derived<Group[]>(computeGroups());
</script>

<PopupBase>
    <h1 class="self-center text-4xl">{popup.mod.Manifest.Name}</h1>
    {#await initPromise}
        <div
            class="w-6 h-6 rounded-full border-4 border-transparent border-b-yellow-300 animate-spin self-center"
        ></div>
    {:then _}
        <div class="pr-5 flex flex-col gap-2 overflow-x-hidden overflow-y-auto">
            {#each groups as group}
                <div class="flex flex-col gap-1">
                    {#if group.category}
                        <h2 class="text-lg text-yellow-300 font-blockletter" title={group.category.Description}>{group.category.Name}</h2>
                    {/if}
                    {#each group.entries as { option, index }, entryI}
                        {@const image = option.Image ? imagePaths.get(option.Image) : undefined}
                        <div class="flex flex-row gap-1">
                            <img
                                class="object-contain self-center shrink-0"
                                src={image ?? FALLBACK_MOD_IMAGE}
                                onerror={useFallbackImage}
                                alt="Option icon"
                                width="64"
                                height="64"
                            />
                            <div class="flex flex-col flex-1 self-stretch min-w-0">
                                <h2 class="text-xl">{option.Name}</h2>
                                <p class="flex-1 text-base truncate" title={option.Description}>{option.Description}</p>
                                {#if option.SubOptions}
                                    <Select
                                        bind:selectedIndex={selected[index]}
                                        items={option.SubOptions}
                                    >
                                        {#snippet renderItem(sub)}
                                            {@const subImage = sub.Image ? imagePaths.get(sub.Image) : undefined}
                                            <div
                                                class="flex flex-row gap-1"
                                                title={sub.Description}
                                            >
                                                <img
                                                    class="object-contain shrink-0"
                                                    src={subImage ?? FALLBACK_MOD_IMAGE}
                                                    onerror={useFallbackImage}
                                                    alt="Sub-Option icon"
                                                    width="48"
                                                    height="48"
                                                />
                                                <div class="flex-1 flex flex-col overflow-y-hidden min-w-0">
                                                    <h3 class="text-sm">{sub.Name}</h3>
                                                    <p class="text-xs truncate">{sub.Description}</p>
                                                </div>
                                            </div>
                                        {/snippet}
                                    </Select>
                                {/if}
                            </div>
                            <input
                                bind:checked={toggled[index]}
                                class="w-5 h-5 mt-1 self-start bg-zinc-800 accent-yellow-300 border-zinc-500 border-2"
                                type="checkbox"
                            />
                        </div>
                        {#if entryI < group.entries.length - 1}
                            <hr class="border-t border-zinc-500" />
                        {/if}
                    {/each}
                </div>
            {/each}
        </div>
    {/await}
    <button class="hd2mm-button self-end" onclick={onClose}>{t("popup.config.ok_button.text")}</button>
</PopupBase>
