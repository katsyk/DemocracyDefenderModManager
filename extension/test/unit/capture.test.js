import { describe, expect, it } from 'vitest';
import '../../src/lib/capture.js';

const { CaptureRegistry, hostMatchesSite, isArchiveDownload, isFromSite, hasArchiveExtension } = globalThis.DDMM.capture;

describe('hostMatchesSite', () => {
  it('matches the site\'s own domain', () => {
    expect(hostMatchesSite('ayakamods.com', 'ayakamods')).toBe(true);
    expect(hostMatchesSite('www.ayakamods.com', 'ayakamods')).toBe(true);
  });

  it('matches a known cross-domain CDN host', () => {
    expect(hostMatchesSite('files.nexus-cdn.com', 'nexus')).toBe(true);
    expect(hostMatchesSite('supporter-files.nexus-cdn.com', 'nexus')).toBe(true);
    expect(hostMatchesSite('objects.githubusercontent.com', 'github')).toBe(true);
  });

  it('matches a same-domain CDN subdomain', () => {
    expect(hostMatchesSite('storage.modworkshop.net', 'modworkshop')).toBe(true);
  });

  it('rejects an unrelated host', () => {
    expect(hostMatchesSite('evil.com', 'ayakamods')).toBe(false);
    expect(hostMatchesSite('notnexusmods.com', 'nexus')).toBe(false);
  });

  it('rejects an unknown site key', () => {
    expect(hostMatchesSite('ayakamods.com', 'not-a-site')).toBe(false);
  });
});

describe('hasArchiveExtension / isArchiveDownload', () => {
  it.each(['mod.zip', 'mod.7z', 'mod.rar', 'MOD.ZIP'])('accepts %s', (name) => {
    expect(hasArchiveExtension(name)).toBe(true);
  });

  it.each(['mod.exe', 'mod.txt', 'mod', 'readme.md'])('rejects %s', (name) => {
    expect(hasArchiveExtension(name)).toBe(false);
  });

  it('strips query/fragment before checking the extension', () => {
    expect(hasArchiveExtension('https://cdn.example.com/mod.zip?x=1#y')).toBe(true);
  });

  it('accepts a matching MIME type even without an archive-looking filename', () => {
    expect(isArchiveDownload({ filename: 'download', mime: 'application/x-7z-compressed' })).toBe(true);
  });

  it('rejects a non-archive download', () => {
    expect(isArchiveDownload({ filename: 'installer.exe', mime: 'application/octet-stream', url: 'https://x/installer.exe' })).toBe(false);
  });
});

describe('isFromSite', () => {
  it('matches via referrer', () => {
    expect(isFromSite({ referrer: 'https://ayakamods.com/mods/foo.1/' }, 'ayakamods')).toBe(true);
  });

  it('matches via finalUrl on a different (CDN) host', () => {
    expect(isFromSite({ finalUrl: 'https://files.nexus-cdn.com/abc.zip' }, 'nexus')).toBe(true);
  });

  it('does not match an unrelated host', () => {
    expect(isFromSite({ referrer: 'https://evil.com/', finalUrl: 'https://evil.com/x.zip' }, 'ayakamods')).toBe(false);
  });
});

describe('CaptureRegistry', () => {
  it('matches an armed site within the time window', () => {
    let now = 1000;
    const reg = new CaptureRegistry({ now: () => now, windowMs: 10 * 60 * 1000 });
    reg.arm('ayakamods', { tabId: 7, pageUrl: 'https://ayakamods.com/mods/foo.1/', pageVersion: '2.0' });
    expect(reg.isArmed('ayakamods')).toBe(true);

    now += 60 * 1000; // 1 minute later, still within window
    const result = reg.match({ referrer: 'https://ayakamods.com/mods/foo.1/', filename: 'C:/dl/foo.zip' });
    expect(result).toEqual({ site: 'ayakamods', pageUrl: 'https://ayakamods.com/mods/foo.1/', pageVersion: '2.0', tabId: 7 });
  });

  it('is one-shot: a match disarms the site', () => {
    const reg = new CaptureRegistry({ now: () => 0 });
    reg.arm('gamebanana');
    reg.match({ referrer: 'https://gamebanana.com/mods/1', filename: 'x.zip' });
    expect(reg.isArmed('gamebanana')).toBe(false);
  });

  it('expires after the arm window', () => {
    let now = 0;
    const reg = new CaptureRegistry({ now: () => now, windowMs: 10 * 60 * 1000 });
    reg.arm('nexus');
    now = 10 * 60 * 1000 + 1;
    expect(reg.isArmed('nexus')).toBe(false);
    expect(reg.match({ referrer: 'https://nexusmods.com/x', filename: 'x.zip' })).toBeNull();
  });

  it('ignores a non-archive download even from an armed site', () => {
    const reg = new CaptureRegistry({ now: () => 0 });
    reg.arm('modworkshop');
    const result = reg.match({ referrer: 'https://modworkshop.net/mod/1', filename: 'readme.txt' });
    expect(result).toBeNull();
    expect(reg.isArmed('modworkshop')).toBe(true); // non-match doesn't consume the arm
  });

  it('ignores a download from a site that was never armed', () => {
    const reg = new CaptureRegistry({ now: () => 0 });
    reg.arm('ayakamods');
    const result = reg.match({ referrer: 'https://gamebanana.com/mods/1', filename: 'x.zip' });
    expect(result).toBeNull();
  });

  it('disarm removes an active arm', () => {
    const reg = new CaptureRegistry({ now: () => 0 });
    reg.arm('github');
    reg.disarm('github');
    expect(reg.isArmed('github')).toBe(false);
  });

  it('re-arming the same site refreshes the window instead of stacking', () => {
    let now = 0;
    const reg = new CaptureRegistry({ now: () => now, windowMs: 1000 });
    reg.arm('github');
    now = 900;
    reg.arm('github'); // refresh
    now = 1100; // would have expired from the first arm, not the refreshed one
    expect(reg.isArmed('github')).toBe(true);
  });
});
