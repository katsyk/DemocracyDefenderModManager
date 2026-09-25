/**
 * @file Popup UI logic. Classic script (see extension/README.md for why),
 * loaded after browser-shim.js and sources.js.
 */
/* global DDMM */
(function initPopup() {
  'use strict';

  const api = DDMM.browserApi;

  function send(message) {
    return api.runtime.sendMessage(message);
  }

  async function currentTabUrl() {
    const [tab] = await api.tabs.query({ active: true, currentWindow: true });
    return tab ? tab.url : null;
  }

  function setText(id, text) {
    document.getElementById(id).textContent = text;
  }

  async function renderConnection() {
    const statusEl = document.getElementById('connection-status');
    const helpEl = document.getElementById('connection-help');
    const helpLink = document.getElementById('connection-help-link');
    const gameEl = document.getElementById('game-status');

    const startEl = document.getElementById('connection-start');
    const startButton = document.getElementById('start-ddmm-button');

    const hello = await send({ type: 'ddmm:hello' });
    startEl.hidden = true;
    if (hello && hello.ok) {
      statusEl.dataset.ok = 'true';
      statusEl.textContent = `Connected to DDMM ${hello.appVersion}`;
      helpEl.hidden = true;

      const status = await send({ type: 'ddmm:status' });
      if (status && status.ok) {
        gameEl.hidden = false;
        gameEl.textContent = status.gameFound
          ? `Game found -- profile "${status.activeProfile}", ${status.modCount} mod(s)`
          : 'Helldivers 2 not found. Set the game path in DDMM.';
      }
    } else if (hello && hello.error && hello.error.code === 'NATIVE_HOST_MISSING') {
      // The native host itself is missing: DDMM isn't installed (or not
      // set up for this browser).
      statusEl.dataset.ok = 'false';
      statusEl.textContent = 'DDMM not found';
      helpEl.hidden = false;
      helpLink.href = DDMM_DOCS_INSTALL_URL;
      gameEl.hidden = true;
    } else {
      // Installed but not running. Checking never starts it; this button
      // (an explicit user action) does, via `open`.
      statusEl.dataset.ok = 'false';
      statusEl.textContent = "DDMM isn't running";
      helpEl.hidden = true;
      gameEl.hidden = true;
      startEl.hidden = false;
      startButton.disabled = false;
      startButton.textContent = 'Start DDMM';
      startButton.onclick = async () => {
        startButton.disabled = true;
        startButton.textContent = 'Starting DDMM…';
        const opened = await send({ type: 'ddmm:open' });
        if (opened && opened.ok) {
          renderConnection();
          return;
        }
        statusEl.textContent = "DDMM didn't start in time";
        startButton.disabled = false;
        startButton.textContent = 'Start DDMM';
      };
    }
  }

  async function renderAutoCapture() {
    const url = await currentTabUrl();
    const source = url ? DDMM.sources.sourceFromPageUrl(url) : null;
    const provider = source ? source.provider : DDMM.sources.providerFromUrl(url || '');
    const siteNameEl = document.getElementById('site-name');
    const toggle = document.getElementById('auto-capture-toggle');

    const KNOWN_SITE_LABELS = {
      ayakamods: 'AyakaMods',
      nexus: 'Nexus Mods',
      modworkshop: 'ModWorkshop',
      gamebanana: 'GameBanana',
      github: 'GitHub',
    };

    if (!provider || !KNOWN_SITE_LABELS[provider]) {
      siteNameEl.textContent = 'Not on a known mod site';
      toggle.disabled = true;
      toggle.checked = false;
      return;
    }

    siteNameEl.textContent = KNOWN_SITE_LABELS[provider];
    toggle.disabled = false;
    const result = await send({ type: 'ddmm:getAutoCapture', site: provider });
    toggle.checked = Boolean(result && result.enabled);
    toggle.onchange = () => {
      send({ type: 'ddmm:setAutoCapture', site: provider, enabled: toggle.checked });
    };
  }

  async function renderAfterInstall() {
    const select = document.getElementById('after-install-select');
    const result = await send({ type: 'ddmm:getAfterInstallOverride' });
    select.value = (result && result.value) || '';
    select.onchange = () => {
      send({ type: 'ddmm:setAfterInstallOverride', value: select.value || null });
    };
  }

  async function renderRecentInstalls() {
    const list = document.getElementById('recent-installs');
    const result = await send({ type: 'ddmm:getRecentInstalls' });
    const installs = (result && result.installs) || [];
    list.innerHTML = '';
    if (installs.length === 0) {
      const li = document.createElement('li');
      li.className = 'ddmm-empty';
      li.textContent = 'No installs yet.';
      list.appendChild(li);
      return;
    }
    for (const entry of installs) {
      const li = document.createElement('li');
      const name = document.createElement('span');
      name.className = 'ddmm-install-name';
      name.textContent = `${entry.updated ? 'Updated' : 'Installed'} ${entry.name || 'mod'}`;
      const meta = document.createElement('span');
      meta.className = 'ddmm-install-meta';
      meta.textContent = entry.provider || '';
      li.append(name, meta);
      list.appendChild(li);
    }
  }

  const DDMM_DOCS_INSTALL_URL = 'https://katsyk.github.io/DemocracyDefenderModManager/using/one-click-install/';

  renderConnection();
  renderAutoCapture();
  renderAfterInstall();
  renderRecentInstalls();
})();
