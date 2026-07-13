import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  loadTavernHeroIncarnationProfile,
  syncTavernHeroStorageFromProfiles,
} from '../src/chrome/TavernModal';
import type { TavernHeroProfileDraft } from '../src/pty-client';

const PROFILE_STORAGE_KEY = 'kota-v2.tavern.hero-profiles';
const CUSTOM_HERO_STORAGE_KEY = 'kota-v2.tavern.custom-heroes';
let originalStorage: PropertyDescriptor | undefined;

beforeEach(() => {
  const storage = new Map<string, string>();
  originalStorage = Object.getOwnPropertyDescriptor(window, 'localStorage');
  Object.defineProperty(window, 'localStorage', {
    configurable: true,
    value: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, String(value)),
      removeItem: (key: string) => storage.delete(key),
    },
  });
  window.localStorage.removeItem(PROFILE_STORAGE_KEY);
  window.localStorage.removeItem(CUSTOM_HERO_STORAGE_KEY);
});

afterEach(() => {
  if (originalStorage) Object.defineProperty(window, 'localStorage', originalStorage);
});

describe('Tavern CLI-default models', () => {
  it('omits model flags for fresh Claude and Codex factory heroes', () => {
    const claude = loadTavernHeroIncarnationProfile('hero-cc');
    const codex = loadTavernHeroIncarnationProfile('hero-dex');

    expect(claude).toMatchObject({ model: 'default', effort: 'max' });
    expect(claude?.args).toEqual(['--effort', 'max', '--dangerously-skip-permissions']);
    expect(claude?.shell).toContain('model: default');
    expect(claude?.shell).not.toContain('claude-opus-4-8[1m]');

    expect(codex).toMatchObject({ model: 'default', effort: 'xhigh' });
    expect(codex?.args).toEqual([
      '--config',
      'model_reasoning_effort="xhigh"',
      '--dangerously-bypass-approvals-and-sandbox',
    ]);
    expect(codex?.shell).toContain('model: default');
    expect(codex?.shell).not.toContain('gpt-5.5');
  });

  it('rebuilds legacy factory shells while preserving explicitly selected models', () => {
    syncTavernHeroStorageFromProfiles([
      legacyFactoryProfile('hero-cc', 'claude', 'claude-opus-4-8[1m]'),
      legacyFactoryProfile('hero-dex', 'codex', 'gpt-5.5'),
    ]);

    const claude = loadTavernHeroIncarnationProfile('hero-cc');
    const codex = loadTavernHeroIncarnationProfile('hero-dex');
    expect(claude?.model).toBe('default');
    expect(claude?.args).not.toContain('--model');
    expect(claude?.shell).not.toContain('claude-opus-4-8[1m]');
    expect(codex?.model).toBe('default');
    expect(codex?.args).not.toContain('--model');
    expect(codex?.shell).not.toContain('gpt-5.5');

    window.localStorage.setItem(PROFILE_STORAGE_KEY, JSON.stringify({
      'hero-dex': {
        provider: 'codex',
        model: 'gpt-5.6-sol',
        shell: 'model: gpt-5.6-sol\nargs:\n  - "--model"\n  - "gpt-5.6-sol"',
      },
    }));
    const pinned = loadTavernHeroIncarnationProfile('hero-dex');
    expect(pinned?.model).toBe('gpt-5.6-sol');
    expect(pinned?.args).toContain('gpt-5.6-sol');
  });
});

function legacyFactoryProfile(
  heroId: string,
  provider: string,
  model: string,
): TavernHeroProfileDraft {
  return {
    heroId,
    name: heroId === 'hero-cc' ? 'CC' : 'Dex',
    provider,
    model,
    effort: provider === 'claude' ? 'max' : 'xhigh',
    skills: ['frontend-design'],
    ghost: 'Factory ghost',
    shell: [
      `provider: ${provider}`,
      `model: ${model}`,
      'args:',
      '  - "--model"',
      `  - "${model}"`,
    ].join('\n'),
  };
}
