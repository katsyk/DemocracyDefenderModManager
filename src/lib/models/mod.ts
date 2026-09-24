import type { UUID } from "$lib/types/uuid";
import { join } from "@tauri-apps/api/path";
import type { Manifest } from "./manifest";
import { convertFileSrc } from "@tauri-apps/api/core";

/** Where a {@link ResolvedSource} came from. */
export type SourceOrigin = "Manifest" | "Install";

/** A {@link import("./manifest").Source} resolved into something directly renderable. */
export type ResolvedSource = {
    readonly Provider: string;
    readonly DisplayName: string;
    readonly PageUrl?: string;
    readonly Version?: string;
    readonly Origin: SourceOrigin;
};

export class Mod {
    constructor(
        public readonly Manifest: Manifest,
        public readonly Directory: string,
        public readonly Sources: ResolvedSource[] = []
    ) {}

    get guid(): UUID {
        return this.Manifest.Guid;
    }

    get name(): string {
        return this.Manifest.Name;
    }

    get description(): string {
        return this.Manifest.Description;
    }

    async iconPath(): Promise<string | null> {
        if (!this.Manifest.IconPath) return null;
        return convertFileSrc(await join(this.Directory, this.Manifest.IconPath));
    }
}