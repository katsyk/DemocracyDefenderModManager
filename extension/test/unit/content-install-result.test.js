/**
 * @vitest-environment jsdom
 * @vitest-environment-options {"url": "https://www.nexusmods.com/helldivers2/mods/123"}
 */
import { beforeAll, describe, expect, it } from 'vitest';

// With auto-capture on, the background sends an install's result to every
// open tab of the site. Only the tab of the mod that was installed may show
// it: a second Nexus tab for another mod must not turn "Installed ✓".

const onMessage = [];
const sent = [];
let installed = false;

beforeAll(async () => {
  globalThis.chrome = {
    runtime: {
      sendMessage: async (msg) => {
        sent.push(msg.type);
        if (msg.type === 'ddmm:hello') return { ok: true };
        if (msg.type === 'ddmm:query') return { ok: true, installed };
        return { ok: true };
      },
      onMessage: { addListener: (fn) => onMessage.push(fn) },
    },
    storage: { local: {} },
  };
  for (const lib of ['browser-shim.js', 'sources.js', 'errors.js', 'button-state.js', 'capture.js']) {
    await import(`../../src/lib/${lib}`);
  }
  await import('../../src/content/common.js');
  globalThis.DDMM.content.runTestAdapter({ name: 'nexus', findInsertionPoint: () => null, findDirectDownloadUrl: () => null });
  await flush();
});

const flush = () => new Promise((r) => setTimeout(r, 20));
const label = () => document.querySelector('[data-ddmm-root]').shadowRoot.querySelector('.ddmm-label').textContent;
const deliver = (message) => onMessage.forEach((fn) => fn(message));

describe('install results sent to every tab of a site', () => {
  it("leave another mod's tab alone, re-checking it instead", async () => {
    expect(label()).toBe('Install with DDMM');
    const queries = sent.filter((t) => t === 'ddmm:query').length;
    deliver({
      type: 'ddmm:installResult',
      broadcast: true,
      pageUrl: 'https://www.nexusmods.com/helldivers2/mods/999',
      reply: { ok: true, type: 'installed', mod: { name: 'Other Mod' } },
    });
    await flush();
    expect(label()).toBe('Install with DDMM');
    expect(sent.filter((t) => t === 'ddmm:query').length).toBe(queries + 1);

    deliver({ type: 'ddmm:installResult', broadcast: true, pageUrl: 'https://www.nexusmods.com/helldivers2/mods/999', reply: { ok: false, error: { code: 'DECLINED' } } });
    await flush();
    expect(label()).toBe('Install with DDMM');
  });

  it("are shown on this mod's own tab", async () => {
    deliver({
      type: 'ddmm:installResult',
      broadcast: true,
      pageUrl: 'https://www.nexusmods.com/helldivers2/mods/123?tab=files',
      reply: { ok: true, type: 'installed', mod: { name: 'Cool Mod' } },
    });
    await flush();
    expect(label()).toBe('Installed ✓');
  });

  it('results sent to this tab alone are always shown', async () => {
    deliver({ type: 'ddmm:installResult', reply: { ok: false, error: { code: 'DECLINED' } } });
    await flush();
    expect(label()).toBe('You chose not to install this mod.');
  });
});
