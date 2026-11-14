<script lang="ts">
    import { tick, type Snippet } from "svelte";
    import { ThreeDotsVertical } from "svelte-bootstrap-icons";
    import { clickOutside, scrollOutside, windowResize, portal } from "$lib/actions";

    let isOpen = $state<boolean>(false);
    let x = $state<number>(0);
    let y = $state<number>(0);
    let button = $state<HTMLElement | null>(null);
    let popup = $state<HTMLElement | null>(null);

    let { children, insertTarget }: { children: Snippet, insertTarget?: string | HTMLElement } = $props();

    $effect(() => {
        if (!popup) return;

        const popupRect = popup.getBoundingClientRect();
        const maxX = window.innerWidth - popupRect.width - 8;
        x = Math.min(x, maxX);
        const maxY = window.innerHeight - popupRect.height - 8;
        y = Math.min(y, maxY);
    });

    /** The menu's items that can be chosen now, in order. */
    function items(): HTMLElement[] {
        if (!popup) return [];
        return [...popup.querySelectorAll<HTMLElement>("button, a[href]")]
            .filter(item => !(item as HTMLButtonElement).disabled);
    }

    async function onClick() {
        isOpen = true;

        const rect = button!.getBoundingClientRect();
        x = rect.left;
        y = rect.bottom + 4;

        // Into the menu, on its first item: the keyboard doesn't have to
        // Tab through the rest of the page to reach it.
        await tick();
        for (const item of popup?.querySelectorAll<HTMLElement>("button, a[href]") ?? []) {
            item.setAttribute("role", "menuitem");
            item.tabIndex = -1;
        }
        items()[0]?.focus();
    }

    /** Close it; with `refocus`, back on the three-dot button. */
    function close(refocus: boolean) {
        isOpen = false;
        if (refocus) button?.focus();
    }

    function onKeydown(e: KeyboardEvent) {
        const list = items();
        const at = list.indexOf(document.activeElement as HTMLElement);
        let next: HTMLElement | undefined;
        switch (e.key) {
            case "ArrowDown":
                next = list[(at + 1) % list.length];
                break;
            case "ArrowUp":
                next = list[(at <= 0 ? list.length : at) - 1];
                break;
            case "Home":
                next = list[0];
                break;
            case "End":
                next = list[list.length - 1];
                break;
            case "Escape":
                // Only the menu: not a popup or anything else listening.
                e.preventDefault();
                e.stopPropagation();
                close(true);
                return;
            case "Tab":
                // From the three-dot button, so Tab goes on from where the
                // menu was opened (the menu itself is elsewhere in the page).
                close(true);
                return;
            default:
                return;
        }
        e.preventDefault();
        next?.focus();
    }

    /** Focus moved somewhere else (not just a click on the menu's own
     * blank space or a separator): close. */
    function onFocusout(e: FocusEvent) {
        const to = e.relatedTarget as Node | null;
        if (to && !popup?.contains(to) && to !== button) close(false);
    }

    /** An item chosen: close, and when the keyboard chose it, put the
     * focus back on the three-dot button rather than nowhere. */
    function onItemChosen() {
        const refocus = popup?.contains(document.activeElement) ?? false;
        close(refocus);
        // Again once the item has done its job: moving the entry moves
        // this button in the page, which drops its focus. Not when the
        // item showed a popup (the page is inert then) or went elsewhere.
        if (refocus) {
            setTimeout(() => {
                const active = document.activeElement;
                if (button?.isConnected && !button.closest("[inert]") && (!active || active === document.body)) button.focus();
            });
        }
    }
</script>

<button
    bind:this={button}
    class="hd2mm-button-nop p-2"
    class:border-yellow-300={isOpen}
    aria-haspopup="menu"
    aria-expanded={isOpen}
    onclick={onClick}
>
    <ThreeDotsVertical class="block mx-auto" />
</button>
{#if isOpen}
    <div
        bind:this={popup}
        use:clickOutside={() => isOpen = false}
        use:scrollOutside={() => isOpen = false}
        use:windowResize={() => isOpen = false}
        use:portal={insertTarget ?? "body"}
        class="fixed border-2 border-zinc-500 bg-zinc-800 drop-shadow-xl/50 transform-none m-0 z-40"
        style:top="{y}px"
        style:left="{x}px"
        role="menu"
        tabindex="-1"
        data-testid="entry-menu"
        onclickcapture={onItemChosen}
        onkeydown={onKeydown}
        onfocusout={onFocusout}
    >
        <div class="flex flex-col items-stretch popup-content">
            {@render children()}
        </div>
    </div>
{/if}
