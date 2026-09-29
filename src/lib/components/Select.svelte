<script lang="ts" module>
    /** Whether this webview keeps markup inside `<option>` (customizable
     * select). Without it, an option shows only its plain text. */
    function detectRichOptions(): boolean {
        if (typeof document === 'undefined') return false;
        const probe = document.createElement('select');
        probe.innerHTML = '<option><span>t</span></option>';
        return probe.firstElementChild?.firstElementChild != null;
    }

    const richOptions = detectRichOptions();
</script>

<script lang="ts" generics="T">
    import type { Snippet } from 'svelte';
    import type { ClassValue } from 'svelte/elements';

    let {
        items,
        renderItem,
        itemLabel,
        selectedIndex = $bindable<number>(),
        class: extraClasses
    }: {
        items: T[];
        renderItem?: Snippet<[T]>;
        /** Plain-text label for an item. Webviews without rich `<option>`
         * support show only an option's text, so give one whenever
         * `renderItem` renders more than a line of text. */
        itemLabel?: (item: T) => string;
        selectedIndex?: number;
        class?: ClassValue;
    } = $props();
</script>

<!--
    The `<option>`s must be direct children of the `<select>`. Webviews
    without customizable-select support (older WebKitGTK/WebView2, which
    DDMM runs in on many machines) only list an option that is a child of
    the select (or of an optgroup), so wrapping them in a `<div>` left the
    dropdown empty there (#55). The `<button>` is ignored by those webviews
    and shows the rich selected item where customizable select works.
-->
<select
    bind:value={selectedIndex}
    class="hd2mm-select px-2 py-1 overflow-y-hidden min-w-0 {extraClasses}"
>
    <button>
        <div>
            <selectedcontent></selectedcontent>
        </div>
    </button>
    {#each items as item, i}
        <option
            value={i}
            label={richOptions ? undefined : itemLabel?.(item)}
            selected={i === selectedIndex}
        >
            {#if renderItem}
                {@render renderItem(item)}
            {:else}
                {String(item)}
            {/if}
        </option>
    {/each}
</select>

<style>
    select, select::picker(select) {
        appearance: base-select;
    }

    select::picker(select) {
        border: 2px solid var(--color-zinc-500);
    }
</style>
