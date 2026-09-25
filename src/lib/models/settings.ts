export type FixedLengthString<N extends number> = string & {
    readonly __length: N;
};

export function toFixedLengthString<N extends number>(value: string, length: N): FixedLengthString<N> {
    if (value.length != length) {
        throw new Error(`Expected string of length ${length}, got ${value.length}`);
    }
    return value as FixedLengthString<N>;
}

export type SkipEntry = FixedLengthString<16>;

export function toSkipEntry(value: string): SkipEntry {
    return toFixedLengthString(value, 16);
}

export function tryToSkipEntry(value: string): SkipEntry | null {
    if (value.length !== 16) return null;
    return value as SkipEntry;
}

/** What a one-click browser install does once the mod is downloaded. */
export type AfterBrowserInstall = "library" | "profile" | "deploy";

export type SettingsV1 = {
    Version: "V1",
    GamePath: string,
    SkipList: SkipEntry[],
    DownloadsPath: string,
    AfterBrowserInstall: AfterBrowserInstall,
    BridgeAllowedSites: string[],
    AutoImportEnabled: boolean,
    /** Opt-in: check for mod updates when DDMM starts (off by default). */
    AutoCheckUpdates: boolean,
    /** With AutoCheckUpdates: re-check every this many hours while open;
     * 0 = only at startup. */
    AutoCheckIntervalHours: number,
    /** Nexus account whose optional API key is stored (backend-owned; the
     * key itself is never part of settings). */
    NexusUsername?: string
};

export type Settings = SettingsV1 /* | SettingsV2 */;