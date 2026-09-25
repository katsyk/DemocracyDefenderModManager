/**
 * Lightweight, non-blocking toasts -- unlike `Popup`, these never stop the
 * user from doing anything else. Used for background events the user
 * didn't just click a button to trigger, like a browser-triggered install
 * finishing while DDMM was in the background.
 */

export type ToastKind = "info" | "warning" | "error";
export type Toast = { id: number; kind: ToastKind; message: string };

let toasts = $state<Toast[]>([]);
let nextId = 0;

const DEFAULT_DURATION_MS = 6000;

function dismiss(id: number) {
    // Mutate in place rather than reassigning: consumers hold on to this
    // exact array (Toast.svelte reads it once via useToast()), so a new
    // array would leave them rendering a stale list -- the first toast
    // never went away and every later one was never shown.
    const i = toasts.findIndex((t) => t.id === id);
    if (i !== -1) toasts.splice(i, 1);
}

function show(kind: ToastKind, message: string, durationMs: number = DEFAULT_DURATION_MS): number {
    const id = nextId++;
    toasts.push({ id, kind, message });
    setTimeout(() => dismiss(id), durationMs);
    return id;
}

// Plain functions (not object-literal methods) so callers can safely
// destructure, e.g. `const { show: showToast } = useToast();`, matching
// this codebase's usePopup()/useLocalization() convention.
export function useToast() {
    return {
        get toasts(): Toast[] {
            return toasts;
        },
        show,
        dismiss
    };
}
