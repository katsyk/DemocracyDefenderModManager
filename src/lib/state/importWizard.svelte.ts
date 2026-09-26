import { listen } from "@tauri-apps/api/event";
import * as log from "@tauri-apps/plugin-log";
import {
    cancelImport,
    detectImportSources,
    runImport,
    scanImportFolder,
    scanImportPaths,
    type ImportItem,
    type ImportProgress,
    type ImportReport,
    type ImportScan,
    type ImportSource,
} from "$lib/utils/commands";

export type ImportStep = "source" | "scanning" | "preview" | "importing" | "done";

/** Statuses that are ticked by default / can't be ticked at all (mirrors
 * `ItemStatus::selected_by_default` / `blocked` on the Rust side). */
export function selectedByDefault(item: ImportItem): boolean {
    return item.Status.Kind === "New";
}

export function blocked(item: ImportItem): boolean {
    return item.Status.Kind === "Unreadable" || item.Status.Kind === "Duplicate";
}

function errorText(ex: unknown): string {
    if (ex instanceof Error) return ex.message;
    if (typeof ex === "string") return ex;
    return JSON.stringify(ex);
}

/**
 * Everything the "Import mods" wizard knows, kept outside the popup
 * component: a popup is unmounted while another one is shown on top of it,
 * and a scan or import must survive that.
 */
export class ImportWizard {
    step = $state<ImportStep>("source");
    sources = $state<ImportSource[] | null>(null);
    scan = $state<ImportScan | null>(null);
    selected = $state<Set<number>>(new Set());
    scanDone = $state(0);
    scanTotal = $state(0);
    progress = $state<ImportProgress | null>(null);
    report = $state<ImportReport | null>(null);
    error = $state<string | null>(null);
    cancelling = $state(false);
    addToProfile = $state(true);
    /** Whether the scan came from explicitly picked files. */
    fromPicked = $state(false);

    get selectedItems(): ImportItem[] {
        return this.scan?.Items.filter(i => this.selected.has(i.Id)) ?? [];
    }

    get selectedBytes(): number {
        return this.selectedItems.reduce((sum, i) => sum + i.Size, 0);
    }

    async detect() {
        try {
            this.sources = await detectImportSources();
        } catch (ex: unknown) {
            log.warn(`Couldn't look for other mod managers' folders: ${errorText(ex)}`);
            this.sources = [];
        }
    }

    async scanFolder(folder: string) {
        this.fromPicked = false;
        await this.runScan(() => scanImportFolder(folder));
    }

    async scanPaths(paths: string[]) {
        this.fromPicked = true;
        await this.runScan(() => scanImportPaths(paths));
    }

    private async runScan(run: () => Promise<ImportScan>) {
        this.error = null;
        this.cancelling = false;
        this.scanDone = 0;
        this.scanTotal = 0;
        this.step = "scanning";
        const unlisten = await listen<{ Done: number; Total: number }>("import://scan-progress", e => {
            this.scanDone = e.payload.Done;
            this.scanTotal = e.payload.Total;
        });
        try {
            const scan = await run();
            if (scan.Cancelled) {
                this.step = "source";
                return;
            }
            this.scan = scan;
            this.selected = new Set(scan.Items.filter(selectedByDefault).map(i => i.Id));
            this.step = "preview";
        } catch (ex: unknown) {
            this.error = errorText(ex);
            this.step = "source";
        } finally {
            unlisten();
        }
    }

    toggle(item: ImportItem, on: boolean) {
        if (blocked(item)) return;
        const next = new Set(this.selected);
        if (on) next.add(item.Id);
        else next.delete(item.Id);
        this.selected = next;
    }

    selectAll() {
        this.selected = new Set(this.scan?.Items.filter(i => !blocked(i)).map(i => i.Id) ?? []);
    }

    selectNone() {
        this.selected = new Set();
    }

    selectNew() {
        this.selected = new Set(this.scan?.Items.filter(selectedByDefault).map(i => i.Id) ?? []);
    }

    async start() {
        const ids = [...this.selected];
        if (ids.length === 0) return;
        this.error = null;
        this.cancelling = false;
        this.progress = null;
        this.step = "importing";
        const unlisten = await listen<ImportProgress>("import://progress", e => {
            this.progress = e.payload;
        });
        try {
            this.report = await runImport(ids);
            this.step = "done";
        } catch (ex: unknown) {
            // Refused before anything was imported (not enough space, a
            // data folder move running, ...): back to the list.
            this.error = errorText(ex);
            this.step = "preview";
        } finally {
            unlisten();
        }
    }

    async cancel() {
        this.cancelling = true;
        try {
            await cancelImport();
        } catch (ex: unknown) {
            log.warn(`Couldn't cancel the import: ${errorText(ex)}`);
        }
    }

    back() {
        this.error = null;
        this.scan = null;
        this.step = "source";
    }
}
