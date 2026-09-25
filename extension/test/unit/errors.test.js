import { describe, expect, it } from 'vitest';
import '../../src/lib/errors.js';

const { describeError, ERROR_MESSAGES } = globalThis.DDMM.errors;

describe('describeError', () => {
  it('returns the table entry for a known code', () => {
    expect(describeError('BUSY')).toBe(ERROR_MESSAGES.BUSY);
    expect(describeError('DECLINED')).toBe('You chose not to install this mod.');
  });

  it('covers every documented protocol error code', () => {
    const documented = [
      'APP_NOT_RUNNING',
      'BAD_REQUEST',
      'UNSUPPORTED',
      'FORBIDDEN_ORIGIN',
      'DECLINED',
      'NOT_ARCHIVE',
      'UNSAFE_ARCHIVE',
      'FILE_NOT_FOUND',
      'GAME_NOT_FOUND',
      'DEPLOY_FAILED',
      'BUSY',
      'INTERNAL',
    ];
    for (const code of documented) {
      expect(ERROR_MESSAGES[code], `missing message for ${code}`).toBeTypeOf('string');
    }
  });

  it('falls back to the reply message for an unknown code', () => {
    expect(describeError('SOMETHING_NEW', 'a specific reason')).toBe('a specific reason');
  });

  it('falls back to a generic message when there is no code and no message', () => {
    expect(describeError(undefined, undefined)).toBe(ERROR_MESSAGES.INTERNAL);
    expect(describeError(null, null)).toBe(ERROR_MESSAGES.INTERNAL);
  });
});
