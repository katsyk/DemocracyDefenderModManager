import { beforeAll, describe, expect, it } from 'vitest';

// The emblem used to be hard-coded DDMM yellow, the same color as the
// button behind it, so it was invisible. It must follow the label color.
beforeAll(async () => {
  globalThis.chrome = { runtime: {}, storage: { local: {} } };
  await import('../../src/lib/browser-shim.js');
  await import('../../src/lib/sources.js');
  await import('../../src/lib/button-state.js');
  await import('../../src/content/common.js');
});

describe('Install with DDMM button emblem', () => {
  it.each([false, true])('is drawn in currentColor, never the button yellow (floating: %s)', (floating) => {
    const widget = globalThis.DDMM.content.createButtonWidget({ floating });
    const svg = widget.host.shadowRoot.querySelector('.ddmm-btn svg');
    expect(svg).not.toBeNull();
    const strokes = [...svg.querySelectorAll('[stroke]')].map((el) => el.getAttribute('stroke'));
    expect(strokes.length).toBeGreaterThan(0);
    for (const stroke of strokes) expect(stroke).toBe('currentColor');
    expect(svg.outerHTML).not.toMatch(/#FFC61A/i);
  });
});
