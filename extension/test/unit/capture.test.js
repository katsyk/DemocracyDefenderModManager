import { describe, expect, it } from 'vitest';
import '../../src/lib/sources.js';
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

describe('CaptureRegistry recent downloads (finished before the click)', () => {
  const PAGE = 'https://www.nexusmods.com/helldivers2/mods/123?tab=files&file_id=456';
  const CDN = 'https://supporter-files.nexus-cdn.com/6119/123/Cool-123-1-0-1700000000.zip?md5=a&expires=1';

  it('claims a finished CDN download (no referrer) of the same mod', () => {
    const reg = new CaptureRegistry({ now: () => 0 });
    reg.remember({ id: 1, url: CDN, finalUrl: CDN, referrer: null, filename: '/d/a.zip' });
    expect(reg.claimRecent('nexus', PAGE)).toMatchObject({ id: 1 });
    expect(reg.claimRecent('nexus', PAGE)).toBeNull(); // one-shot
  });

  it('claims a download whose URL names no mod when its referrer is that mod page', () => {
    const reg = new CaptureRegistry({ now: () => 0 });
    const bare = 'https://files.nexus-cdn.com/download/abc.zip';
    reg.remember({ id: 2, url: bare, finalUrl: bare, referrer: PAGE, filename: '/d/b.zip' });
    expect(reg.claimRecent('nexus', 'https://www.nexusmods.com/helldivers2/mods/123')).toMatchObject({ id: 2 });
  });

  it('never claims a download of another mod, or one it cannot tie to the page', () => {
    const reg = new CaptureRegistry({ now: () => 0 });
    const other = 'https://supporter-files.nexus-cdn.com/6119/999/Other-999-1-0-1700000000.zip';
    reg.remember({ id: 3, url: other, finalUrl: other, filename: '/d/c.zip' });
    reg.remember({ id: 4, url: 'https://files.nexus-cdn.com/x/abc.zip', filename: '/d/d.zip' });
    expect(reg.claimRecent('nexus', PAGE)).toBeNull();
  });

  it('forgets downloads older than the recent window', () => {
    let now = 0;
    const reg = new CaptureRegistry({ now: () => now });
    reg.remember({ id: 5, url: CDN, finalUrl: CDN, filename: '/d/e.zip' });
    now = globalThis.DDMM.capture.RECENT_WINDOW_MS + 1;
    expect(reg.claimRecent('nexus', PAGE)).toBeNull();
  });

  it('ignores non-archives and unknown sites', () => {
    const reg = new CaptureRegistry({ now: () => 0 });
    reg.remember({ id: 6, url: 'https://supporter-files.nexus-cdn.com/6119/123/readme.txt', filename: '/d/r.txt' });
    reg.remember({ id: 7, url: 'https://evil.example/6119/123/x.zip', filename: '/d/x.zip' });
    expect(reg.serialize().recent).toHaveLength(0);
  });

  it('survives a serialize/restore round trip (MV3 service worker restart)', () => {
    const a = new CaptureRegistry({ now: () => 100 });
    a.arm('nexus', { tabId: 3, pageUrl: PAGE, pageVersion: '1.0' });
    a.remember({ id: 8, url: CDN, finalUrl: CDN, filename: '/d/f.zip' });
    const snapshot = JSON.parse(JSON.stringify(a.serialize()));
    const b = new CaptureRegistry({ now: () => 200 });
    b.restore(snapshot);
    expect(b.isArmed('nexus')).toBe(true);
    expect(b.match({ url: CDN, finalUrl: CDN, filename: '/d/g.zip' })).toMatchObject({ site: 'nexus', tabId: 3, pageVersion: '1.0' });
    expect(b.claimRecent('nexus', PAGE)).toMatchObject({ id: 8 });
  });
});
