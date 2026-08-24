import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  isTauri: vi.fn(),
}));

import { invoke, isTauri } from '@tauri-apps/api/core';
import type { UserHeroAvatar } from '../src/lib/hero-avatars';

const STORAGE_KEY = 'kota-v2.user-hero-avatars';
let originalStorage: PropertyDescriptor | undefined;

function avatar(id: string, data = 'data:image/png;base64,YQ=='): UserHeroAvatar {
  return {
    id,
    label: id,
    dataUrl: data,
    createdAt: '2026-08-15T00:00:00.000Z',
    mime: 'image/png',
    sizeBytes: 1,
  };
}

describe('user hero avatar storage', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    const storage = new Map<string, string>();
    originalStorage = Object.getOwnPropertyDescriptor(window, 'localStorage');
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      value: {
        getItem: vi.fn((key: string) => storage.get(key) ?? null),
        setItem: vi.fn((key: string, value: string) => storage.set(key, String(value))),
        removeItem: vi.fn((key: string) => storage.delete(key)),
      },
    });
    vi.mocked(isTauri).mockReturnValue(true);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    if (originalStorage) Object.defineProperty(window, 'localStorage', originalStorage);
  });

  it('uses disk as the Tauri source of truth and removes an aligned legacy mirror', async () => {
    const stored = avatar('user:stored');
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify([stored]));
    const setItem = vi.mocked(window.localStorage.setItem);
    setItem.mockClear();
    vi.mocked(invoke).mockResolvedValue([stored]);

    const heroAvatars = await import('../src/lib/hero-avatars');
    expect(heroAvatars.loadUserHeroAvatars()).toEqual([]);
    expect(heroAvatars.normalizeHeroAvatarId(stored.id, 'codex')).toBe(stored.id);
    await expect(heroAvatars.refreshUserHeroAvatars()).resolves.toEqual([stored]);

    expect(invoke).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith('hero_avatar_list');
    expect(setItem).not.toHaveBeenCalled();
    expect(window.localStorage.getItem(STORAGE_KEY)).toBeNull();
    expect(heroAvatars.loadUserHeroAvatars()).toEqual([stored]);
  });

  it('falls back from a missing uploaded avatar only after disk hydration completes', async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    const heroAvatars = await import('../src/lib/hero-avatars');

    expect(heroAvatars.normalizeHeroAvatarId('user:missing', 'codex')).toBe('user:missing');
    await heroAvatars.refreshUserHeroAvatars();
    expect(heroAvatars.normalizeHeroAvatarId('user:missing', 'codex')).toBe('codex');
  });

  it('resumes only the missing legacy avatars after a partial migration', async () => {
    const first = avatar('user:first');
    const second = avatar('user:second', 'data:image/png;base64,Yg==');
    const disk = new Map<string, UserHeroAvatar>();
    let secondFailed = false;
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify([first, second]));
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === 'hero_avatar_list') return [...disk.values()];
      if (command === 'hero_avatar_save') {
        const request = (args as { request: UserHeroAvatar }).request;
        if (request.id === second.id && !secondFailed) {
          secondFailed = true;
          throw new Error('migration interrupted');
        }
        const saved = request.id === first.id ? first : second;
        disk.set(saved.id, saved);
        return saved;
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    const { refreshUserHeroAvatars } = await import('../src/lib/hero-avatars');
    await expect(refreshUserHeroAvatars()).resolves.toEqual([first]);
    expect(window.localStorage.getItem(STORAGE_KEY)).not.toBeNull();

    await expect(refreshUserHeroAvatars()).resolves.toEqual([first, second]);
    expect(window.localStorage.getItem(STORAGE_KEY)).toBeNull();
    expect(vi.mocked(invoke).mock.calls
      .filter(([command]) => command === 'hero_avatar_save')
      .map(([, args]) => (args as { request: UserHeroAvatar }).request.id))
      .toEqual([first.id, second.id, second.id]);
    expect(warning).toHaveBeenCalledOnce();
  });

  it('never writes avatar data to localStorage after Tauri save or delete', async () => {
    const stored = avatar('user:sha256-content');
    const setItem = vi.mocked(window.localStorage.setItem).mockImplementation(() => {
      throw new DOMException('The quota has been exceeded.');
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'hero_avatar_save') return stored;
      if (command === 'hero_avatar_delete') return undefined;
      throw new Error(`Unexpected command: ${command}`);
    });

    const { addUserHeroAvatar, deleteUserHeroAvatar } = await import('../src/lib/hero-avatars');
    await expect(addUserHeroAvatar('portrait.png', stored.dataUrl)).resolves.toEqual(stored);
    await expect(deleteUserHeroAvatar(stored.id)).resolves.toBeUndefined();

    expect(setItem).not.toHaveBeenCalled();
    expect(invoke).toHaveBeenNthCalledWith(1, 'hero_avatar_save', {
      request: { label: 'portrait', dataUrl: stored.dataUrl },
    });
    expect(invoke).toHaveBeenNthCalledWith(2, 'hero_avatar_delete', {
      request: { avatarId: stored.id },
    });
  });

  it('keeps localStorage persistence for the browser fallback', async () => {
    vi.mocked(isTauri).mockReturnValue(false);
    const { addUserHeroAvatar, deleteUserHeroAvatar } = await import('../src/lib/hero-avatars');

    const stored = await addUserHeroAvatar('portrait.png', 'data:image/png;base64,YQ==');
    expect(JSON.parse(window.localStorage.getItem(STORAGE_KEY) || '[]')).toEqual([stored]);
    expect(invoke).not.toHaveBeenCalled();

    await deleteUserHeroAvatar(stored.id);
    expect(JSON.parse(window.localStorage.getItem(STORAGE_KEY) || '[]')).toEqual([]);
  });
});
