import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: vi.fn() }));
import { invoke, isTauri } from '@tauri-apps/api/core';
import { bbsSyncAvatarRead, createBbsAvatarImages, parseBbsSyncAvatar, bbsBuiltinAvatarClass } from '../src/bbs-sync-avatars';

const image = 'data:image/png;base64,YQ==';
const ref = (index = 1) => ({ sha256: index.toString(16).padStart(64, '0'), ext: 'png' });
const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
beforeEach(() => { vi.clearAllMocks(); vi.mocked(isTauri).mockReturnValue(true); });

describe('BBS origin avatar client', () => {
  it('is inert on import and only absence preserves local lookup', async () => {
    vi.resetModules(); await import('../src/bbs-sync-avatars');
    expect(invoke).not.toHaveBeenCalled();
    expect(parseBbsSyncAvatar(undefined)).toBeUndefined();
    for (const value of [null, [], 'codex', {}, { kind: 'future' }, { kind: 'builtin', id: 'user:same-local-id' },
      { kind: 'builtin', id: '__proto__' }, { kind: 'image', ...ref(), available: 'true', localPath: '/tmp/file' },
      { kind: 'image', ...ref(), ext: '../../png', available: true, localPath: '/tmp/file' },
    ]) expect(parseBbsSyncAvatar(value)).toEqual({ kind: 'none' });
    expect(parseBbsSyncAvatar({ kind: 'builtin', id: 'violet', secret: 'not-forwarded' })).toEqual({ kind: 'builtin', id: 'violet' });
    expect(bbsBuiltinAvatarClass('violet')).toBe('system-violet');
    expect(bbsBuiltinAvatarClass('user:local')).toBeUndefined();
  });

  it('accepts the agreed sidecar without using localPath as read authority', async () => {
    const origin = parseBbsSyncAvatar({ kind: 'image', ...ref(), available: true, localPath: '/private/origin.png' });
    vi.mocked(invoke).mockResolvedValue(image);
    expect(await bbsSyncAvatarRead(origin as never)).toBe(image);
    expect(invoke).toHaveBeenCalledExactlyOnceWith('bbs_sync_avatar_read', { request: ref() });
  });

  it('rejects browser use and invalid hash/extension before IPC', async () => {
    for (const request of [{ ...ref(), sha256: 'a'.repeat(63) }, { ...ref(), sha256: 'A'.repeat(64) },
      { ...ref(), ext: 'svg' }, { ...ref(), ext: '../png' }, { ...ref(), ext: 'constructor' }]) {
      await expect(bbsSyncAvatarRead(request)).rejects.toThrow('BBS avatar is unavailable.');
    }
    vi.mocked(isTauri).mockReturnValue(false);
    await expect(bbsSyncAvatarRead(ref())).rejects.toThrow('BBS avatar is unavailable.');
    expect(invoke).not.toHaveBeenCalled();
  });

  it('only accepts bounded raster base64 of the requested format', async () => {
    for (const value of [null, {}, 'https://remote/avatar.png', 'data:image/svg+xml;base64,YQ==',
      'data:image/jpeg;base64,YQ==', 'data:image/png;base64,', 'data:image/png;base64,YQ=',
      'data:image/png;base64,YQ=\n', 'data:image/png;base64,=YQ=', 'data:image/png;base64,YQ===',
      `data:image/png;base64,${btoa('x'.repeat(600_001))}`,
    ]) {
      vi.mocked(invoke).mockResolvedValueOnce(value);
      await expect(bbsSyncAvatarRead(ref())).rejects.toThrow('BBS avatar is unavailable.');
    }
    const atLimit = `data:image/png;base64,${btoa('x'.repeat(600_000))}`;
    vi.mocked(invoke).mockResolvedValueOnce(atLimit);
    expect(await bbsSyncAvatarRead(ref())).toBe(atLimit);
    for (const [ext, mime] of [['jpg', 'image/jpeg'], ['webp', 'image/webp']]) {
      const data = `data:${mime};base64,YQ==`;
      vi.mocked(invoke).mockResolvedValueOnce(data);
      expect(await bbsSyncAvatarRead({ ...ref(), ext })).toBe(data);
    }
  });

  it('redacts all read failures without retry or persistence', async () => {
    for (const failure of [new Error('/private/avatar token=SECRET'), { privateKey: 'SECRET' }, 'SECRET']) {
      vi.mocked(invoke).mockRejectedValueOnce(failure);
      const error = await bbsSyncAvatarRead(ref()).catch((caught: unknown) => caught);
      expect(error).toBeInstanceOf(Error);
      expect(error).toMatchObject({ message: 'BBS avatar is unavailable.' });
      expect(error).not.toHaveProperty('cause');
    }
    expect(invoke).toHaveBeenCalledTimes(3);
  });
});

describe('BBS bounded avatar image queue', () => {
  it('deduplicates list/detail reads and releases unsubscribed listeners', async () => {
    let finish!: (src: string) => void;
    const read = vi.fn(() => new Promise<string>((resolve) => { finish = resolve; }));
    const images = createBbsAvatarImages(read), first = vi.fn(), second = vi.fn();
    const stop = images.subscribe(ref(), first);
    images.subscribe(ref(), second);
    await flush(); expect(read).toHaveBeenCalledTimes(1);
    stop(); finish(image); await flush();
    expect(first).not.toHaveBeenCalled(); expect(second).toHaveBeenCalledWith(image);
    const cached = vi.fn(); images.subscribe(ref(), cached);
    expect(cached).toHaveBeenCalledWith(image); expect(read).toHaveBeenCalledTimes(1);
  });

  it('caps reads at two and outstanding distinct resources at 32, with no overflow retry loop', async () => {
    const pending: ((src: string) => void)[] = [];
    const read = vi.fn(() => new Promise<string>((resolve) => pending.push(resolve)));
    const images = createBbsAvatarImages(read), listeners = Array.from({ length: 40 }, () => vi.fn());
    const stops = listeners.map((listener, i) => images.subscribe(ref(i), listener));
    await flush(); expect(read).toHaveBeenCalledTimes(2);
    expect(listeners.slice(0, 32).every((listener) => listener.mock.calls.length === 0)).toBe(true);
    expect(listeners.slice(32).every((listener) => listener.mock.calls[0]?.[0] === null)).toBe(true);
    pending[0](image); await flush(); expect(read).toHaveBeenCalledTimes(3);
    stops.forEach((stop) => stop()); pending.slice(1).forEach((resolve) => resolve(image)); await flush();
    expect(read).toHaveBeenCalledTimes(3); // Closing the board cancels the queued work.
  });

  it('does not read a just-closed view and cancellation cannot remove a newer subscription', async () => {
    const read = vi.fn(async () => image), images = createBbsAvatarImages(read);
    images.subscribe(ref(), vi.fn())(); await flush(); expect(read).not.toHaveBeenCalled();
    let finish!: (src: string) => void;
    const blocked = createBbsAvatarImages(() => new Promise<string>((resolve) => { finish = resolve; }));
    blocked.subscribe(ref(1), vi.fn()); blocked.subscribe(ref(2), vi.fn());
    const stop = blocked.subscribe(ref(3), vi.fn()); stop();
    const current = vi.fn(); blocked.subscribe(ref(3), current); stop();
    await flush(); finish(image); await flush(); finish(image); await flush();
    expect(current).toHaveBeenCalledWith(image);
  });

  it('retains only 16 successful values with LRU eviction and supports failed-decode invalidation', async () => {
    const read = vi.fn(async () => image), images = createBbsAvatarImages(read);
    for (let i = 0; i < 16; i++) { images.subscribe(ref(i), vi.fn()); await flush(); }
    images.subscribe(ref(0), vi.fn()); await flush(); expect(read).toHaveBeenCalledTimes(16);
    images.subscribe(ref(16), vi.fn()); await flush();
    images.subscribe(ref(0), vi.fn()); await flush(); expect(read).toHaveBeenCalledTimes(17);
    images.subscribe(ref(1), vi.fn()); await flush(); expect(read).toHaveBeenCalledTimes(18);
    images.forget(ref(1)); images.subscribe(ref(1), vi.fn()); await flush(); expect(read).toHaveBeenCalledTimes(19);
  });

  it('does not cache failures, automatically retry, or cache a departed view result', async () => {
    const read = vi.fn().mockRejectedValueOnce(new Error('gone')).mockResolvedValue(image);
    const images = createBbsAvatarImages(read), listener = vi.fn();
    images.subscribe(ref(), listener); await flush();
    expect(listener).toHaveBeenCalledWith(null); expect(read).toHaveBeenCalledTimes(1);
    images.subscribe(ref(), listener); await flush(); expect(read).toHaveBeenCalledTimes(2);
    let finish!: (src: string) => void;
    const lateRead = vi.fn(() => new Promise<string>((resolve) => { finish = resolve; }));
    const late = createBbsAvatarImages(lateRead);
    const stop = late.subscribe(ref(), listener); await flush(); stop(); finish(image); await flush();
    late.subscribe(ref(), listener); await flush(); expect(lateRead).toHaveBeenCalledTimes(2);
    finish(image); await flush();
  });
});
