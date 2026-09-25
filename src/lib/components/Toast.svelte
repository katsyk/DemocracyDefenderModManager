<!--
  @component
  Stacked, self-dismissing toasts -- click one to dismiss it early.
-->

<script lang="ts">
    import { useToast } from "$lib/state/toast.svelte";

    const toastState = useToast();
    const dismiss = toastState.dismiss;
</script>

<div class="absolute bottom-2 right-2 z-50 flex flex-col-reverse gap-1 items-end pointer-events-none">
    {#each toastState.toasts as toast (toast.id)}
        <button
            class="pointer-events-auto max-w-80 p-2 border-2 text-sm text-start shadow-xl/30"
            class:bg-blue-950={toast.kind === "info"}
            class:border-blue-500={toast.kind === "info"}
            class:text-blue-100={toast.kind === "info"}
            class:bg-yellow-950={toast.kind === "warning"}
            class:border-yellow-400={toast.kind === "warning"}
            class:text-yellow-100={toast.kind === "warning"}
            class:bg-red-950={toast.kind === "error"}
            class:border-red-500={toast.kind === "error"}
            class:text-red-100={toast.kind === "error"}
            onclick={() => dismiss(toast.id)}
        >
            {toast.message}
        </button>
    {/each}
</div>
