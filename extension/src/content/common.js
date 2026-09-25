/**
 * @file Shared "Install with DDMM" button: shadow-DOM widget, state
 * rendering, and the per-page driver that wires a site adapter (see
 * content/sites/*.js) up to the background script.
 *
 * DDMM brand: #FFC61A (yellow) on #0B0B0C (near-black), compact, obviously
 * not part of the host page. Isolated in a shadow root so no mod site's CSS
 * can bleed in (or be bled on).
 */
/* global DDMM */
(function initContentCommon(root) {
  'use strict';

  const DDMM_NS = (root.DDMM = root.DDMM || {});

  const DOCS_INSTALL_URL = 'https://katsyk.github.io/DemocracyDefenderModManager/using/one-click-install/';

  const SVG_NS = 'http://www.w3.org/2000/svg';

  /**
   * Builds a small inline version of the shield-and-chevrons emblem, DDMM
   * yellow on transparent, as real SVG DOM nodes (not `innerHTML`, so
   * there's nothing here for a store reviewer -- or `web-ext lint` -- to
   * flag as an unsanitized dynamic-markup assignment, even though every
   * value involved is a static constant).
   * @returns {SVGSVGElement}
   */
  function buildEmblem() {
    const svg = document.createElementNS(SVG_NS, 'svg');
    svg.setAttribute('viewBox', '0 0 512 512');
    svg.setAttribute('width', '14');
    svg.setAttribute('height', '14');
    svg.setAttribute('aria-hidden', 'true');
    svg.setAttribute('focusable', 'false');

    const shield = document.createElementNS(SVG_NS, 'path');
    shield.setAttribute('d', 'M256 40 L440 100 V250 C440 362 358 436 256 478 C154 436 72 362 72 250 V100 Z');
    shield.setAttribute('fill', 'none');
    shield.setAttribute('stroke', '#FFC61A');
    shield.setAttribute('stroke-width', '28');
    svg.appendChild(shield);

    const chevrons = document.createElementNS(SVG_NS, 'g');
    chevrons.setAttribute('fill', 'none');
    chevrons.setAttribute('stroke', '#FFC61A');
    chevrons.setAttribute('stroke-width', '30');
    chevrons.setAttribute('stroke-linejoin', 'miter');
    chevrons.setAttribute('stroke-linecap', 'butt');
    for (const points of ['164,282 256,226 348,282', '164,334 256,278 348,334']) {
      const chevron = document.createElementNS(SVG_NS, 'polyline');
      chevron.setAttribute('points', points);
      chevrons.appendChild(chevron);
    }
    svg.appendChild(chevrons);

    return svg;
  }

  const STYLE = `
    :host { all: initial; }
    .ddmm-btn {
      all: initial;
      display: inline-flex;
      align-items: center;
      gap: 6px;
      font: 600 13px/1.2 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      background: #FFC61A;
      color: #0B0B0C;
      border: 2px solid #0B0B0C;
      border-radius: 6px;
      padding: 7px 12px;
      cursor: pointer;
      white-space: nowrap;
      user-select: none;
      box-shadow: 0 1px 0 rgba(0,0,0,0.4);
    }
    .ddmm-btn:hover { background: #ffd24d; }
    .ddmm-btn:active { background: #e6b117; }
    .ddmm-btn[data-state="installing"],
    .ddmm-btn[data-state="checking"] { cursor: progress; opacity: 0.85; }
    .ddmm-btn[data-state="installed"] { background: #2ecc71; color: #0B0B0C; }
    .ddmm-btn[data-state="update"] { background: #3ba7ff; color: #0B0B0C; }
    .ddmm-btn[data-state="error"] { background: #ff5555; color: #1a0000; }
    .ddmm-btn[data-state="unreachable"] { background: #0B0B0C; color: #FFC61A; border-color: #FFC61A; }
    .ddmm-hint {
      all: initial;
      display: block;
      margin-top: 4px;
      font: 500 11px/1.3 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      color: #cfcfcf;
      background: #0B0B0C;
      border: 1px solid #FFC61A;
      border-radius: 4px;
      padding: 4px 6px;
      max-width: 240px;
    }
    /* "all: initial; display: block" above would otherwise override the
       hidden attribute, leaving an empty hint bar under every button. */
    .ddmm-hint[hidden] { display: none; }
    .ddmm-floating {
      position: fixed;
      right: 16px;
      bottom: 16px;
      z-index: 2147483647;
      display: flex;
      flex-direction: column;
      align-items: flex-end;
    }
  `;

  /**
   * @typedef {object} ButtonWidget
   * @property {HTMLElement} host - The element inserted into the page (a shadow host).
   * @property {(state: import('../lib/button-state.js').ButtonState) => void} render
   * @property {(hint: string|null) => void} setHint
   * @property {(handler: (event: MouseEvent) => void) => void} onClick
   */

  /**
   * Builds the shadow-DOM button widget. Does not insert it into the page.
   * @param {{floating: boolean}} [opts]
   * @returns {ButtonWidget}
   */
  function createButtonWidget(opts = {}) {
    const host = document.createElement('div');
    host.setAttribute('data-ddmm-root', '');
    if (opts.floating) host.className = 'ddmm-floating-host';

    // 'open' (not 'closed'): the shadow root still fully isolates the host
    // page's CSS from ours (and ours from it), which is the actual
    // isolation goal here. What 'closed' would additionally hide is JS
    // access to `.shadowRoot` itself -- but the real defense against a page
    // script driving our button is the `event.isTrusted` check in
    // onClick() below, which stops a synthetic/dispatched click regardless
    // of shadow mode. 'open' is also what lets devtools and test tooling
    // (Playwright's locators, which cannot pierce a closed root) inspect
    // the button normally.
    const shadow = host.attachShadow({ mode: 'open' });
    const style = document.createElement('style');
    style.textContent = STYLE;
    shadow.appendChild(style);

    const wrapper = document.createElement('div');
    if (opts.floating) wrapper.className = 'ddmm-floating';
    shadow.appendChild(wrapper);

    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'ddmm-btn';
    button.appendChild(buildEmblem());
    const label = document.createElement('span');
    label.className = 'ddmm-label';
    label.textContent = 'Install with DDMM';
    button.appendChild(label);
    wrapper.appendChild(button);

    const hint = document.createElement('div');
    hint.className = 'ddmm-hint';
    hint.hidden = true;
    wrapper.appendChild(hint);

    return {
      host,
      render(state) {
        button.dataset.state = state.state;
        label.textContent = state.label;
        button.disabled = !state.clickable;
      },
      setHint(text) {
        if (text) {
          hint.textContent = text;
          hint.hidden = false;
        } else {
          hint.hidden = true;
        }
      },
      onClick(handler) {
        button.addEventListener('click', (event) => {
          // Security rule (bridge-protocol.md): only synthetic-click-proof,
          // user-generated clicks may drive an install. A page script
          // dispatching a fake click on our button (it can't reach inside
          // the shadow root anyway, but defense in depth) must be ignored.
          if (!event.isTrusted) return;
          handler(event);
        });
      },
    };
  }

  /**
   * @typedef {object} SiteAdapter
   * @property {string} name - Key into DDMM.capture.SITE_HOSTS.
   * @property {(doc: Document) => Element|null} findInsertionPoint - Where
   *   to place the button inline; `null` means "use the floating fallback".
   * @property {(doc: Document) => string|null} [findDirectDownloadUrl] - A
   *   direct file URL already present on the page, if any.
   * @property {(doc: Document) => string|null} [scrapeVersion]
   */

  /**
   * Drives one mod page: detects it, builds the button, queries DDMM, and
   * wires up clicks. No-ops if the current URL isn't a mod page for this
   * adapter.
   * @param {SiteAdapter} adapter
   */
  function runSiteAdapter(adapter) {
    const source = DDMM_NS.sources.sourceFromPageUrl(location.href);
    if (!source || source.provider !== adapter.name) return;
    driveAdapter(adapter);
  }

  /**
   * Same driver as {@link runSiteAdapter}, without the
   * `sourceFromPageUrl` gate -- used only by the localhost
   * test-fixture adapter (never shipped in a release build; see
   * `includeLocalhost` in scripts/manifest.mjs) so the Playwright smoke
   * test can exercise the real button/query/install pipeline against a
   * page that isn't one of the five real mod-site hosts.
   * @param {SiteAdapter} adapter
   */
  function runTestAdapter(adapter) {
    driveAdapter(adapter);
  }

  /** @param {SiteAdapter} adapter */
  function driveAdapter(adapter) {
    const insertionPoint = adapter.findInsertionPoint ? adapter.findInsertionPoint(document) : null;
    const widget = createButtonWidget({ floating: !insertionPoint });
    if (insertionPoint) {
      insertionPoint.appendChild(widget.host);
    } else {
      document.documentElement.appendChild(widget.host);
    }

    const state = new DDMM_NS.ButtonState();
    /** Render the state, including its own hint ("DDMM will start"). */
    function render() {
      widget.render(state);
      widget.setHint(state.hint);
    }
    render();

    const pageVersion = adapter.scrapeVersion ? adapter.scrapeVersion(document) : null;

    /** @param {object} message @returns {Promise<object>} */
    function send(message) {
      return DDMM_NS.browserApi.runtime.sendMessage(message);
    }

    /**
     * Label the button from DDMM's current state. Only ever *checks* --
     * hello/query never start DDMM (the host launches it only for
     * install/open), so merely browsing a mod page can't pop DDMM open.
     */
    async function refresh() {
      const hello = await send({ type: 'ddmm:hello' });
      // An install the user already clicked owns the button until its
      // result arrives; a late check must not overwrite that.
      if (state.busy) return;
      if (!hello || !hello.ok) {
        const code = hello && hello.error && hello.error.code;
        if (code === 'NATIVE_HOST_MISSING') {
          state.setUnreachable();
        } else {
          state.setAppNotRunning();
        }
        render();
        return;
      }
      const result = await send({ type: 'ddmm:query', pageUrl: location.href, pageVersion });
      if (state.busy) return;
      if (result && result.ok) {
        state.setQueryResult(result);
      } else {
        state.setAppNotRunning();
      }
      render();
    }

    widget.onClick(async () => {
      if (state.state === 'unreachable') {
        window.open(DOCS_INSTALL_URL, '_blank', 'noopener');
        return;
      }
      if (!state.clickable) return;

      state.startInstalling();
      render();

      const directUrl = adapter.findDirectDownloadUrl ? adapter.findDirectDownloadUrl(document) : null;
      if (directUrl) {
        await send({
          type: 'ddmm:installDirect',
          url: directUrl,
          pageUrl: location.href,
          pageVersion,
        });
        // The result arrives asynchronously via the 'ddmm:installResult'
        // runtime message once the browser download finishes.
        return;
      }

      await send({ type: 'ddmm:armCapture', site: adapter.name, pageUrl: location.href });
      widget.setHint('Click Download on this page. DDMM will take it from there.');
    });

    DDMM_NS.browserApi.runtime.onMessage.addListener((message) => {
      if (!message) return;
      if (message.type === 'ddmm:installResult') {
        if (message.reply && message.reply.ok) {
          state.setInstalled();
        } else {
          const err = message.reply && message.reply.error;
          state.setError(DDMM_NS.errors.describeError(err && err.code, err && err.message));
        }
        render();
      } else if (message.type === 'ddmm:captureExpired' && message.site === adapter.name) {
        // The armed install never happened; release the button and re-check.
        state.reset();
        render();
        refresh();
      }
    });

    refresh();
  }

  DDMM_NS.content = { createButtonWidget, runSiteAdapter, runTestAdapter, DOCS_INSTALL_URL };
})(typeof globalThis !== 'undefined' ? globalThis : this);
