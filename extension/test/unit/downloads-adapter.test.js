import { describe, expect, it } from 'vitest';
import '../../src/lib/downloads-adapter.js';

const { normalizeDownloadItem, normalizeChangedDelta } = globalThis.DDMM.downloadsAdapter;

describe('normalizeDownloadItem', () => {
  it('uses finalUrl when present (Chromium)', () => {
    const n = normalizeDownloadItem({
      id: 1,
      state: 'complete',
      filename: 'C:\\Users\\me\\Downloads\\mod.zip',
      url: 'https://ayakamods.com/mods/foo.1/download',
      finalUrl: 'https://ayakamods.com/data/files/mod.zip',
      referrer: 'https://ayakamods.com/mods/foo.1/',
      mime: 'application/zip',
    });
    expect(n.finalUrl).toBe('https://ayakamods.com/data/files/mod.zip');
    expect(n.filename).toBe('C:\\Users\\me\\Downloads\\mod.zip');
  });

  it('falls back to url when finalUrl is absent (Firefox)', () => {
    const n = normalizeDownloadItem({
      id: 2,
      state: 'complete',
      filename: '/home/me/Downloads/mod.zip',
      url: 'https://modworkshop.net/mods/files/mod.zip',
      referrer: 'https://modworkshop.net/mod/1',
    });
    expect(n.finalUrl).toBe('https://modworkshop.net/mods/files/mod.zip');
  });

  it('defaults missing optional fields to null', () => {
    const n = normalizeDownloadItem({ id: 3, state: 'in_progress' });
    expect(n.filename).toBeNull();
    expect(n.url).toBeNull();
    expect(n.finalUrl).toBeNull();
    expect(n.referrer).toBeNull();
    expect(n.mime).toBeNull();
  });
});

describe('normalizeChangedDelta', () => {
  it('reads the current state out of the delta wrapper', () => {
    const n = normalizeChangedDelta({ id: 5, state: { current: 'complete', previous: 'in_progress' } });
    expect(n).toEqual({ id: 5, state: 'complete', filenameChanged: false });
  });

  it('handles a delta with no state change (e.g. just filename determined)', () => {
    const n = normalizeChangedDelta({ id: 6, filename: { current: '/x/mod.zip', previous: '' } });
    expect(n.state).toBeNull();
    expect(n.filenameChanged).toBe(true);
  });
});
