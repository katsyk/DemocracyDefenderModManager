import { describe, expect, it } from 'vitest';
import '../../src/lib/button-state.js';

const { ButtonState } = globalThis.DDMM;

describe('ButtonState', () => {
  it('starts in checking', () => {
    const s = new ButtonState();
    expect(s.state).toBe('checking');
    expect(s.label).toBe('Checking DDMM…');
    expect(s.clickable).toBe(false);
  });

  it('goes to unreachable -> "Get DDMM"', () => {
    const s = new ButtonState();
    s.setUnreachable();
    expect(s.label).toBe('Get DDMM');
    expect(s.clickable).toBe(true);
  });

  it('not installed -> "Install with DDMM"', () => {
    const s = new ButtonState();
    s.setQueryResult({ installed: false, updateAvailable: null });
    expect(s.label).toBe('Install with DDMM');
  });

  it('installed, no update -> "Installed ✓"', () => {
    const s = new ButtonState();
    s.setQueryResult({ installed: true, updateAvailable: false });
    expect(s.label).toBe('Installed ✓');
  });

  it('installed with update available -> "Update with DDMM"', () => {
    const s = new ButtonState();
    s.setQueryResult({ installed: true, updateAvailable: true });
    expect(s.label).toBe('Update with DDMM');
  });

  it('installed with unknown update status is not treated as an update', () => {
    const s = new ButtonState();
    s.setQueryResult({ installed: true, updateAvailable: null });
    expect(s.label).toBe('Installed ✓');
  });

  it('installing -> "Installing…" and not clickable', () => {
    const s = new ButtonState();
    s.startInstalling();
    expect(s.label).toBe('Installing…');
    expect(s.clickable).toBe(false);
  });

  it('install success -> "Installed ✓"', () => {
    const s = new ButtonState();
    s.startInstalling();
    s.setInstalled();
    expect(s.label).toBe('Installed ✓');
    expect(s.clickable).toBe(true);
  });

  it('install error shows the friendly message', () => {
    const s = new ButtonState();
    s.startInstalling();
    s.setError("DDMM couldn't be reached.");
    expect(s.state).toBe('error');
    expect(s.label).toBe("DDMM couldn't be reached.");
    expect(s.clickable).toBe(true);
  });

  it('reset returns to checking', () => {
    const s = new ButtonState();
    s.setQueryResult({ installed: true, updateAvailable: false });
    s.reset();
    expect(s.state).toBe('checking');
  });
});
