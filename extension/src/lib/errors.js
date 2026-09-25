/**
 * @file Protocol error code -> friendly English message table.
 *
 * Codes are defined in docs/development/bridge-protocol.md ("Errors"
 * section). Keep this table in sync with that document.
 */
(function initErrors(root) {
  'use strict';

  const DDMM = (root.DDMM = root.DDMM || {});

  /** @type {Record<string, string>} */
  const ERROR_MESSAGES = {
    APP_NOT_RUNNING: "DDMM didn't start in time. Open DDMM, then try again.",
    NATIVE_HOST_MISSING: "DDMM isn't installed (or isn't set up for this browser). Get DDMM, then try again.",
    BAD_REQUEST: 'DDMM rejected that request as malformed. This is a DDMM extension bug -- please report it.',
    UNSUPPORTED: 'This version of the DDMM extension is too old (or too new) for your DDMM app. Try updating one of them.',
    FORBIDDEN_ORIGIN: "DDMM didn't recognize this browser extension. Try reinstalling the DDMM extension.",
    DECLINED: 'You chose not to install this mod.',
    NOT_ARCHIVE: "That file isn't a mod archive DDMM recognizes (zip, 7z or rar).",
    UNSAFE_ARCHIVE: 'That archive failed a safety check and was not installed.',
    FILE_NOT_FOUND: "DDMM couldn't find the downloaded file. Try downloading it again.",
    GAME_NOT_FOUND: "The mod was installed, but DDMM couldn't find your Helldivers 2 install to deploy it. Set the game path in DDMM.",
    DEPLOY_FAILED: 'The mod was installed, but deploying it failed. Check DDMM for details.',
    BUSY: 'DDMM is busy with another install right now. Try again in a moment.',
    INTERNAL: 'Something went wrong in DDMM.',
    TIMEOUT: 'DDMM took too long to respond.',
    DISCONNECTED: 'Lost the connection to DDMM.',
  };

  /**
   * @param {string|undefined|null} code
   * @param {string} [fallbackMessage] - The `message` field from the error
   *   reply, used when `code` isn't in the table.
   * @returns {string} A friendly, English message safe to show the user.
   */
  function describeError(code, fallbackMessage) {
    if (code && ERROR_MESSAGES[code]) {
      return ERROR_MESSAGES[code];
    }
    return fallbackMessage || ERROR_MESSAGES.INTERNAL;
  }

  DDMM.errors = { ERROR_MESSAGES, describeError };
})(typeof globalThis !== 'undefined' ? globalThis : this);
