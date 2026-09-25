import { convertFileSrc, invoke } from "@tauri-apps/api/core";

/** Shown when a mod has no image, or its image can't be loaded. */
export const FALLBACK_MOD_IMAGE = "images/hd2_icon.png";

/**
 * Turn an image path from a mod's manifest (`IconPath`, an option's
 * `Image`) into something an `<img>` can load, or `null` if the file
 * doesn't exist. The backend resolves it with the same rules deploy uses
 * for manifest paths (`utils::fix_path_casing`: Windows `\` separators,
 * any casing, never outside the mod folder), so there's one mechanism.
 */
export async function modImageSrc(modDirectory: string, manifestPath: string): Promise<string | null> {
    const resolved = await invoke<string | null>("resolve_mod_image", { modDirectory, image: manifestPath });
    return resolved ? convertFileSrc(resolved) : null;
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
