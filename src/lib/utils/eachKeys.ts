// Keys for Svelte's keyed `{#each}` blocks. No imports but types, so the
// unit tests in tests/unit can run them with plain Node.
import type { UpdateStatusEntry } from "./commands";

/** Each item with a key for a keyed `{#each}`: `key(item)`, made unique by
 * a `#n` suffix on repeats. A keyed `{#each}` throws on a repeated key
 * (`each_key_duplicate`), and the list -- or the popup or page around it --
 * is then never shown (issue #71). Lists built from backend data or files
 * can repeat what was assumed to be unique. */
export function withUniqueKeys<T>(items: readonly T[], key: (item: T) => string): [T, string][] {
    const seen = new Map<string, number>();
    return items.map(item => {
        const k = key(item);
        const n = seen.get(k) ?? 0;
        seen.set(k, n + 1);
        return [item, n === 0 ? k : `${k}#${n}`];
    });
}

/** An update check result is per mod, site and the mod's ID on that site
 * (a mod can name two pages on the same site). */
export function updateEntryKey(entry: Pick<UpdateStatusEntry, "Guid" | "Provider" | "SourceId">): string {
    return `${entry.Guid}|${entry.Provider}|${entry.SourceId ?? ""}`;
}
