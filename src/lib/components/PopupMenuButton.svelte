<script lang="ts">
    import { tick, type Snippet } from "svelte";
    import { ThreeDotsVertical } from "svelte-bootstrap-icons";
    import { clickOutside, scrollOutside, windowResize, portal } from "$lib/actions";
    import { usePopup } from "$lib/state/popup.svelte";

    let isOpen = $state<boolean>(false);
    let x = $state<number>(0);
    let y = $state<number>(0);
    let button = $state<HTMLElement | null>(null);
    let popup = $state<HTMLElement | null>(null);

    /** The items (`children`) are `role="menuitem" tabindex="-1"`
     * buttons: the arrow keys move between them, not Tab. */
    let { children, insertTarget, label }: {
        children: Snippet,
        insertTarget?: string | HTMLElement,
        /** What the button and its menu are called for screen readers. */
        label?: string,
    } = $props();

    const popups = usePopup();

    $effect(() => {
        if (!popup) return;

        const popupRect = popup.getBoundingClientRect();
        const maxX = window.innerWidth - popupRect.width - 8;
        x = Math.min(x, maxX);
        const maxY = window.innerHeight - popupRect.height - 8;
        y = Math.min(y, maxY);
    });

    // A popup shown while the menu is open makes the page (the menu with
    // it) inert: close the menu rather than leave it open out of reach.
    $effect(() => {
        if (popups.isShown) isOpen = false;
    });

    /** The menu's items that can be chosen now, in order. */
    function items(): HTMLElement[] {
        if (!popup) return [];
        return [...popup.querySelectorAll<HTMLElement>("[role='menuitem']")]
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

    /** Focus left the menu: close it. That includes focus dropped to
     * nowhere (a popup made the page inert), but not the window itself
     * losing focus (another app): the menu is still there on return. */
    function onFocusout(e: FocusEvent) {
        const to = e.relatedTarget as Node | null;
        if (to === button || (to && popup?.contains(to))) return;
        if (to) {
            close(false);
            return;
        }
        setTimeout(() => {
            if (!isOpen || !document.hasFocus()) return;
            const active = document.activeElement;
            if (!active || active === document.body || !popup?.contains(active)) close(false);
        });
    }

    /** The three-dot buttons of all menus like this one in the page, in
     * order (one per row). */
    function openers(): HTMLElement[] {
        const scope = button?.closest("main") ?? document;
        return [...scope.querySelectorAll<HTMLElement>("button[aria-haspopup='menu']")];
    }

    /** An item chosen: close, and when the keyboard chose it, put the
     * focus back on the three-dot button rather than nowhere. Once the
     * item has done its job, again: moving the row moves the button,
     * which drops its focus; removing the row removes it, and then the
     * focus goes to the next row's button (the previous row's when it
     * was the last; the page itself when none is left). Not when the item
     * showed a popup (the page is inert then) or put the focus elsewhere. */
    function onItemChosen() {
        const refocus = popup?.contains(document.activeElement) ?? false;
        const before = openers();
        const at = button ? before.indexOf(button) : -1;
        close(refocus);
        if (!refocus) return;
        // A removed row can linger for a moment (the list animates it
        // out): keep watching for a few frames until it's gone.
        let frames = 30;
        const settle = () => {
            const active = document.activeElement;
            if (active && active !== document.body && active !== button) return;
            if (!button?.isConnected) {
                const after = openers().filter(b => !b.closest("[inert]"));
                const target = after[Math.min(at, after.length - 1)] ?? document.querySelector<HTMLElement>("main");
                if (target && !target.closest("[inert]")) {
                    if (target.tagName === "MAIN" && !target.hasAttribute("tabindex")) target.tabIndex = -1;
                    target.focus();
                }
                return;
            }
            if (active !== button && !button.closest("[inert]")) button.focus();
            if (--frames > 0) requestAnimationFrame(settle);
        };
        setTimeout(settle);
    }
</script>

<button
    bind:this={button}
    class="hd2mm-button-nop p-2"
    class:border-yellow-300={isOpen}
    aria-haspopup="menu"
    aria-expanded={isOpen}
    aria-label={label}
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
        aria-label={label}
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
