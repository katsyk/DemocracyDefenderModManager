import { convertFileSrc } from "@tauri-apps/api/core";
import { join } from "@tauri-apps/api/path";

/** Shown when a mod has no image, or its image can't be loaded. */
export const FALLBACK_MOD_IMAGE = "images/hd2_icon.png";

/**
 * Turn an image path from a mod's manifest (`IconPath`, an option's
 * `Image`) into something an `<img>` can load. Manifests are often written
 * on Windows with backslashes (`images\icon.png`), which on Linux/macOS
 * would be one file name containing a backslash, so both separators are
 * accepted and the path is rebuilt with the platform's own.
 */
export async function modImageSrc(modDirectory: string, manifestPath: string): Promise<string> {
    const parts = manifestPath.split(/[\\/]+/).filter((p) => p.length > 0 && p !== ".");
    return convertFileSrc(await join(modDirectory, ...parts));
}

/**
 * `onerror` handler for mod images: a missing or unreadable file falls back
 * to the default icon instead of a broken-image box.
 */
export function useFallbackImage(event: Event): void {
    const img = event.currentTarget as HTMLImageElement | null;
    if (!img || img.dataset.fallback === "1") return;
    img.dataset.fallback = "1";
    img.src = FALLBACK_MOD_IMAGE;
}
