/**
 * @file The "Install with DDMM" button's state machine, shared by every
 * content-script site adapter and unit-tested independently of the DOM.
 *
 * States and their labels (see the top-level task spec):
 *   - `checking`     -> "Checking DDMM…"
 *   - `unreachable`  -> "Get DDMM" (links to the docs install page)
 *   - `install`      -> "Install with DDMM"
 *   - `installed`    -> "Installed ✓" (click -> reinstall/open)
 *   - `update`       -> "Update with DDMM"
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
  };

  /** Finite state machine for one button instance. Framework-agnostic. */
  class ButtonState {
    constructor() {
      /** @type {string} */
      this.state = 'checking';
      /** @type {string|null} */
      this.errorMessage = null;
      /** @type {boolean} */
      this.updateAvailable = false;
    }

    /** @returns {string} The current label to render. */
    get label() {
      if (this.state === 'error') return this.errorMessage || 'DDMM error';
      return LABELS[this.state] || LABELS.install;
    }

    /** @returns {boolean} Whether the button should currently accept clicks. */
    get clickable() {
      return this.state !== 'installing' && this.state !== 'checking';
    }

    /** DDMM isn't reachable -> "Get DDMM". */
    setUnreachable() {
      this.state = 'unreachable';
      this.errorMessage = null;
    }

    /**
     * Apply a `queryResult` reply.
     * @param {{installed: boolean, updateAvailable: boolean|null}} result
     */
    setQueryResult(result) {
      this.errorMessage = null;
      if (result && result.installed) {
        this.updateAvailable = Boolean(result.updateAvailable);
        this.state = this.updateAvailable ? 'update' : 'installed';
      } else {
        this.updateAvailable = false;
        this.state = 'install';
      }
    }

    /** Transition to the mid-flight "Installing…" state. */
    startInstalling() {
      this.state = 'installing';
      this.errorMessage = null;
    }

    /** Install (or reinstall/update) finished successfully. */
    setInstalled() {
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
    }
  }

  DDMM.ButtonState = ButtonState;
  DDMM.buttonLabels = LABELS;
})(typeof globalThis !== 'undefined' ? globalThis : this);
