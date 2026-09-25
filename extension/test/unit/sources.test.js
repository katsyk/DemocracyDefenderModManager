import { describe, expect, it } from 'vitest';
import '../../src/lib/sources.js';

const { sourceFromPageUrl, providerFromUrl } = globalThis.DDMM.sources;

// Mirrors src-tauri/src/sources.rs's `source_from_page_url` and
// `provider_from_url` test cases exactly, so the extension and the desktop
// app can never quietly drift apart on what a mod page URL means.
describe('sourceFromPageUrl', () => {
  it('parses an ayakamods slug + id URL', () => {
    const s = sourceFromPageUrl('https://ayakamods.com/mods/hd2-auto-reload.4084/');
    expect(s).toEqual({ provider: 'ayakamods', id: '4084' });
  });

  it('parses an ayakamods bare id with no trailing slash', () => {
    const s = sourceFromPageUrl('https://ayakamods.com/mods/4084');
    expect(s).toEqual({ provider: 'ayakamods', id: '4084' });
  });

  it('parses ayakamods with query and fragment, and www.', () => {
    const s = sourceFromPageUrl('https://www.ayakamods.com/mods/hd2-auto-reload.4084/?utm_source=x#reviews');
    expect(s).toEqual({ provider: 'ayakamods', id: '4084' });
  });

  it('rejects non-numeric ayakamods ids', () => {
    expect(sourceFromPageUrl('https://ayakamods.com/mods/hd2-auto-reload.abc/')).toBeNull();
    expect(sourceFromPageUrl('https://ayakamods.com/mods/not-a-number/')).toBeNull();
  });

  it('still resolves an ayakamods URL with a /download subpath', () => {
    const s = sourceFromPageUrl('https://ayakamods.com/mods/hd2-auto-reload.4084/download');
    expect(s.id).toBe('4084');
  });

  it('parses a nexus mod URL', () => {
    const s = sourceFromPageUrl('https://www.nexusmods.com/helldivers2/mods/123?tab=files');
    expect(s).toEqual({ provider: 'nexus', id: '123' });
  });

  it('rejects non-numeric nexus ids', () => {
    expect(sourceFromPageUrl('https://www.nexusmods.com/helldivers2/mods/abc')).toBeNull();
  });

  it('parses a modworkshop mod URL', () => {
    const s = sourceFromPageUrl('https://modworkshop.net/mod/12345/');
    expect(s).toEqual({ provider: 'modworkshop', id: '12345' });
  });

  it('parses a gamebanana mod URL', () => {
    const s = sourceFromPageUrl('https://gamebanana.com/mods/999999');
    expect(s).toEqual({ provider: 'gamebanana', id: '999999' });
  });

  it('rejects non-numeric gamebanana ids', () => {
    expect(sourceFromPageUrl('https://gamebanana.com/mods/abc')).toBeNull();
  });

  it('parses a github repo URL as owner/repo', () => {
    const s = sourceFromPageUrl('https://github.com/someone/example-mod');
    expect(s).toEqual({ provider: 'github', id: 'someone/example-mod' });
  });

  it('parses a github repo URL with extra path segments', () => {
    const s = sourceFromPageUrl('https://github.com/someone/example-mod/releases/latest');
    expect(s.id).toBe('someone/example-mod');
  });

  it('returns null for an unknown host', () => {
    expect(sourceFromPageUrl('https://example.com/downloads/mod.zip')).toBeNull();
  });

  it('rejects a non-http(s) scheme', () => {
    expect(sourceFromPageUrl('ftp://ayakamods.com/mods/4084/')).toBeNull();
  });

  it('rejects garbage input', () => {
    expect(sourceFromPageUrl('not a url at all')).toBeNull();
  });
});

describe('providerFromUrl', () => {
  it.each([
    ['https://www.nexusmods.com/helldivers2/mods/1', 'nexus'],
    ['https://modworkshop.net/mod/1', 'modworkshop'],
    ['https://github.com/owner/repo', 'github'],
    ['https://objects.githubusercontent.com/foo', 'github'],
    ['https://gamebanana.com/mods/1', 'gamebanana'],
    ['https://cdn.example.com/download.zip', 'url'],
    ['not a url', 'url'],
  ])('%s -> %s', (url, expected) => {
    expect(providerFromUrl(url)).toBe(expected);
  });
});

describe('Nexus file-download URLs', () => {
  const { nexusFileFromUrl, modFromDownloadUrl, attributeDownload } = globalThis.DDMM.sources;

  it.each([
    ['https://cf-files.nexusmods.com/cdn/6119/123/Cool Mod-123-1-0-1700000000.zip?md5=a&expires=1&user_id=2', '6119', '123'],
    ['https://supporter-files.nexus-cdn.com/6119/4567/X-4567-2-0.7z?key=a&expires=1', '6119', '4567'],
    ['https://premium-files.nexus-cdn.com/1704/9/y.rar', '1704', '9'],
  ])('reads game and mod id from %s', (url, gameId, modId) => {
    expect(nexusFileFromUrl(url)).toEqual({ gameId, modId });
  });

  it.each([
    'https://www.nexusmods.com/helldivers2/mods/123',
    'https://cf-files.nexusmods.com/cdn/abc/123/x.zip',
    'https://files.nexus-cdn.com/x.zip',
    'https://evil.example/6119/123/x.zip',
    'not a url',
  ])('rejects %s', (url) => {
    expect(nexusFileFromUrl(url)).toBeNull();
  });

  it('maps a Helldivers 2 file to its canonical mod page', () => {
    expect(modFromDownloadUrl('https://supporter-files.nexus-cdn.com/6119/123/x.zip')).toEqual({
      source: { provider: 'nexus', id: '123' },
      pageUrl: 'https://www.nexusmods.com/helldivers2/mods/123',
    });
  });

  it("never gives another game's file a page", () => {
    expect(modFromDownloadUrl('https://supporter-files.nexus-cdn.com/1704/123/x.zip').pageUrl).toBeNull();
  });

  it('attributes a same-mod CDN file to the page the user was on (so its version may be sent)', () => {
    expect(attributeDownload({
      contextPageUrl: 'https://www.nexusmods.com/helldivers2/mods/123?tab=files',
      downloadUrl: 'https://supporter-files.nexus-cdn.com/6119/123/x.zip',
      trustPage: true,
    })).toEqual({ pageUrl: 'https://www.nexusmods.com/helldivers2/mods/123?tab=files', sameModAsPage: true });
  });

  it("attributes another mod's CDN file to that mod, even when the page is trusted", () => {
    expect(attributeDownload({
      contextPageUrl: 'https://www.nexusmods.com/helldivers2/mods/123',
      downloadUrl: 'https://supporter-files.nexus-cdn.com/6119/999/x.zip',
      trustPage: true,
    })).toEqual({ pageUrl: 'https://www.nexusmods.com/helldivers2/mods/999', sameModAsPage: false });
  });
});
