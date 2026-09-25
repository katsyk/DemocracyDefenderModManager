/** A byte count as a short human-readable size ("1.4 GB"). */
export function formatBytes(bytes: number): string {
    const units = ["bytes", "KB", "MB", "GB", "TB"];
    let value = bytes;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
        value /= 1024;
        unit++;
    }
    return unit === 0 ? `${bytes} bytes` : `${value.toFixed(1)} ${units[unit]}`;
}
