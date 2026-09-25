/**
 * @file The "Install with DDMM" button's state machine, shared by every
 * content-script site adapter and unit-tested independently of the DOM.
 *
 * States and their labels (see the top-level task spec):
 *   - `checking`     -> "Checking DDMM…"
 *   - `unreachable`  -> "Get DDMM" (links to the docs install page) -- only
 *                       when the native host itself is missing, never just
 *                       because DDMM isn't running
 *   - `install`      -> "Install with DDMM" (also when DDMM isn't running:
 *                       then with the hint "DDMM will start", since
 *                       clicking starts it)
 *   - `starting`     -> "Starting DDMM…" (clicked while DDMM wasn't running;
 *                       the install launches it)
 *   - `installed`    -> "Installed ✓" (click -> reinstall/open)
 *   - `update`       -> "Update with DDMM"
 *   - `waiting`      -> "Click Download on this page…" (capture armed:
 *                       whatever archive this site downloads next -- via
 *                       the site's own button or any other tool the user
 *                       runs -- is installed; still clickable to re-arm)
 *   - `installing`   -> "Installing…"
 *   - `error`        -> the error's friendly message
 */
(function initButtonState(root) {
  'use strict';

  const DDMM = (root.DDMM = root.DDMM || {});

  /** @type {Record<string, string>} */
  const LABELS = {
    checking: 'Checking DDMM…',
    unreachable: 'Get DDMM',
    install: 'Install with DDMM',
    installed: 'Installed ✓',
    update: 'Update with DDMM',
    installing: 'Installing…',
    starting: 'Starting DDMM…',
    waiting: 'Click Download on this page…',
  };

  /** Shown under "Install with DDMM" when DDMM isn't running. */
  const NOT_RUNNING_HINT = 'DDMM will start';

  /** Shown under "Click Download on this page…" while capture is armed. */
  const WAITING_HINT = 'DDMM installs the next mod file this site downloads.';

  /** Finite state machine for one button instance. Framework-agnostic. */
  class ButtonState {
    constructor() {
      /** @type {string} */
      this.state = 'checking';
      /** @type {string|null} */
      this.errorMessage = null;
      /** @type {boolean} */
      this.updateAvailable = false;
      /**
       * Whether DDMM answered the last check. When it didn't, installed /
       * update status is unknown and never guessed.
       * @type {boolean}
       */
      this.appRunning = true;
    }

    /** @returns {string|null} A hint to show under the button, if any. */
    get hint() {
      if (this.state === 'waiting') return WAITING_HINT;
      return this.state === 'install' && !this.appRunning ? NOT_RUNNING_HINT : null;
    }

    /** @returns {string} The current label to render. */
    get label() {
      if (this.state === 'error') return this.errorMessage || 'DDMM error';
      return LABELS[this.state] || LABELS.install;
    }

    /** @returns {boolean} Whether the button should currently accept clicks. */
    get clickable() {
      return this.state !== 'installing' && this.state !== 'checking' && this.state !== 'starting';
    }

    /** DDMM isn't reachable -> "Get DDMM". */
    setUnreachable() {
      this.state = 'unreachable';
      this.errorMessage = null;
    }

    /**
     * The native host answered but DDMM isn't running (APP_NOT_RUNNING, or
     * it didn't answer in time). Still installable -- the install starts
     * DDMM -- but whether this mod is installed is unknown, so don't guess.
     */
    setAppNotRunning() {
      this.state = 'install';
      this.errorMessage = null;
      this.updateAvailable = false;
      this.appRunning = false;
    }

    /**
     * Apply a `queryResult` reply.
     * @param {{installed: boolean, updateAvailable: boolean|null}} result
     */
    setQueryResult(result) {
      this.errorMessage = null;
      this.appRunning = true;
      if (result && result.installed) {
        this.updateAvailable = Boolean(result.updateAvailable);
        this.state = this.updateAvailable ? 'update' : 'installed';
      } else {
        this.updateAvailable = false;
        this.state = 'install';
      }
    }

    /**
     * Transition to the mid-flight state: "Installing…", or "Starting DDMM…"
     * when DDMM wasn't running and this install is what launches it.
     */
    startInstalling() {
      this.state = this.appRunning ? 'installing' : 'starting';
      this.errorMessage = null;
    }

    /**
     * Capture is armed: waiting for the site's download, which the user
     * starts (on the page, or with whatever tool they use). Shown right
     * away -- never gated on the site's own countdown.
     */
    waitForDownload() {
      this.state = 'waiting';
      this.errorMessage = null;
    }

    /**
     * @returns {boolean} Whether an install is in flight (or armed and
     *   waiting for its download), so a late DDMM check mustn't relabel it.
     */
    get busy() {
      return this.state === 'installing' || this.state === 'starting' || this.state === 'waiting';
    }

    /** Install (or reinstall/update) finished successfully. */
    setInstalled() {
      this.appRunning = true;
      this.state = 'installed';
      this.errorMessage = null;
      this.updateAvailable = false;
    }

    /**
     * Install failed; show the message, then the caller re-queries (or the
     * user retries) to move on from `error`.
     * @param {string} message
     */
    setError(message) {
      this.state = 'error';
      this.errorMessage = message;
    }

    /** Back to the initial "checking" state (e.g. on navigation to a new mod page). */
    reset() {
      this.state = 'checking';
      this.errorMessage = null;
      this.updateAvailable = false;
      this.appRunning = true;
    }
  }

  DDMM.ButtonState = ButtonState;
  DDMM.buttonLabels = LABELS;
  DDMM.buttonHints = { NOT_RUNNING_HINT, WAITING_HINT };
})(typeof globalThis !== 'undefined' ? globalThis : this);
