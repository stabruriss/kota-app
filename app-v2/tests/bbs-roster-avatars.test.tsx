import { act, fireEvent, render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: vi.fn() }));
vi.mock('../src/bbs-roster-avatars', async original => ({ ...await original<typeof import('../src/bbs-roster-avatars')>(),
  bbsRosterAvatarImages: { subscribe: vi.fn(), forget: vi.fn() } }));
import { invoke, isTauri } from '@tauri-apps/api/core';
import * as hero from '../src/lib/hero-avatars';
import { bbsRosterAvatarImages, bbsRosterAvatarRead, createBbsRosterAvatarImages, type BbsRosterAvatarRef } from '../src/bbs-roster-avatars';
import { BbsRosterAvatar } from '../src/chrome/BbsRosterAvatar';
import type { BbsRosterAvatar as Avatar } from '../src/types/bbs-roster';

const data = 'data:image/png;base64,YQ==';
const ref = (deviceId = 'peer', sha256 = 'a'.repeat(64)): BbsRosterAvatarRef => ({ deviceId, sha256, ext: 'png', sizeBytes: 1 });
const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
const descriptor = (available = true, sha256 = ref().sha256): Avatar => ({ kind: 'image', sha256, ext: 'png', sizeBytes: 1, available });
const observers: Observer[] = [];
class Observer {
  element!: Element;
  constructor(private callback: IntersectionObserverCallback) { observers.push(this); }
  observe(element: Element) { this.element = element; }
  disconnect = vi.fn();
  change(isIntersecting: boolean) { this.callback([{ target: this.element, isIntersecting } as IntersectionObserverEntry], this as unknown as IntersectionObserver); }
}
let callbacks: ((src: string | null) => void)[];
let stops: ReturnType<typeof vi.fn>[];
beforeEach(() => {
  vi.restoreAllMocks(); vi.clearAllMocks(); vi.mocked(isTauri).mockReturnValue(true);
  observers.length = 0; callbacks = []; stops = [];
  vi.stubGlobal('IntersectionObserver', Observer);
  vi.mocked(bbsRosterAvatarImages.subscribe).mockImplementation((_ref, callback) => { callbacks.push(callback); const stop = vi.fn(); stops.push(stop); return stop; });
});
afterEach(() => vi.unstubAllGlobals());

describe('device-scoped roster avatar reader and queue', () => {
  it('passes only device and hash, preserving roster authorization without a post reader fallback', async () => {
    vi.mocked(invoke).mockResolvedValue(data);
    expect(await bbsRosterAvatarRead({ ...ref('local'), path: '/private' } as never)).toBe(data);
    expect(invoke).toHaveBeenCalledExactlyOnceWith('bbs_roster_avatar_read', { request: { deviceId: 'local', sha256: ref().sha256 } });
    vi.mocked(invoke).mockRejectedValueOnce({ secret: 'SECRET' });
    const error = await bbsRosterAvatarRead(ref()).catch(error => error);
    expect(error).toMatchObject({ message: 'Roster avatar is unavailable.' }); expect(error).not.toHaveProperty('cause');
    expect(invoke).toHaveBeenCalledTimes(2);
  });
  it('checks raster format/declared size and rejects bad references before IPC', async () => {
    for (const bad of [{ ...ref(), deviceId: '../x' }, { ...ref(), sha256: 'a' }, { ...ref(), ext: 'svg' },
      { ...ref(), sizeBytes: 0 }, { ...ref(), sizeBytes: 600001 }]) await expect(bbsRosterAvatarRead(bad)).rejects.toThrow('unavailable');
    expect(invoke).not.toHaveBeenCalled();
    for (const value of [data.replace('png', 'jpeg'), 'data:image/png;base64,YWI=', 'https://evil/avatar', 'data:image/png;base64,=YQ=', null]) {
      vi.mocked(invoke).mockResolvedValueOnce(value); await expect(bbsRosterAvatarRead(ref())).rejects.toThrow('unavailable');
    }
    vi.mocked(isTauri).mockReturnValue(false);
    await expect(bbsRosterAvatarRead(ref())).rejects.toThrow('unavailable'); expect(invoke).toHaveBeenCalledTimes(5);
  });
  it('never shares authorization/cache across devices, dedupes within one and snapshots input', async () => {
    const read = vi.fn(async () => data), images = createBbsRosterAvatarImages(read), a = vi.fn(), b = vi.fn();
    const request = ref(); images.subscribe(request, a); request.deviceId = 'changed';
    images.subscribe(ref(), b); await flush();
    expect(read).toHaveBeenCalledExactlyOnceWith(ref()); expect(a).toHaveBeenCalledWith(data); expect(b).toHaveBeenCalledWith(data);
    images.subscribe(ref('another'), a); await flush(); expect(read).toHaveBeenCalledTimes(2);
    images.subscribe(ref(), a); expect(read).toHaveBeenCalledTimes(2);
  });
  it('bounds outstanding jobs/read concurrency and close discards queued work with no retry', async () => {
    const finish: ((src: string) => void)[] = [];
    const read = vi.fn(() => new Promise<string>(resolve => finish.push(resolve))), images = createBbsRosterAvatarImages(read);
    const callbacks = Array.from({ length: 40 }, () => vi.fn());
    const stops = callbacks.map((callback, i) => images.subscribe(ref(`device${i}`), callback));
    await flush(); expect(read).toHaveBeenCalledTimes(2);
    expect(callbacks.slice(32).every(callback => callback.mock.calls[0]?.[0] === null)).toBe(true);
    stops.forEach(stop => stop()); finish.forEach(resolve => resolve(data)); await flush(); expect(read).toHaveBeenCalledTimes(2);
    images.subscribe(ref('device0'), vi.fn()); await flush(); expect(read).toHaveBeenCalledTimes(3); // departed bytes weren't cached
    finish[2](data); await flush();
  });
  it('shares the established LRU 16 mechanics, with no cache for failed reads', async () => {
    const read = vi.fn().mockRejectedValueOnce(new Error('bad')).mockResolvedValue(data), images = createBbsRosterAvatarImages(read);
    const receive = vi.fn(); images.subscribe(ref(), receive); await flush(); expect(receive).toHaveBeenCalledWith(null);
    images.subscribe(ref(), receive); await flush(); expect(read).toHaveBeenCalledTimes(2);
    for (let i = 0; i < 16; i++) { images.subscribe(ref(`p${i}`), vi.fn()); await flush(); }
    images.subscribe(ref(), receive); await flush(); expect(read).toHaveBeenCalledTimes(19);
    images.forget(ref()); images.subscribe(ref(), receive); await flush(); expect(read).toHaveBeenCalledTimes(20);
  });
});
describe('roster avatar visibility and fallback', () => {
  it('uses a letter for none/missing/future builtin, never a same-named local hero', () => {
    const classSpy = vi.spyOn(hero, 'avatarClassForId'), styleSpy = vi.spyOn(hero, 'avatarImageStyleForId');
    const { container, rerender } = render(<BbsRosterAvatar deviceId="peer" name="颦儿" avatar={{ kind: 'none' }} />);
    for (const avatar of [descriptor(false), { kind: 'builtin', id: 'future-hero' }, { kind: 'none' }] as Avatar[]) {
      rerender(<BbsRosterAvatar deviceId="peer" name="颦儿" avatar={avatar} />);
      expect(container).toHaveTextContent('颦'); expect(observers).toHaveLength(0);
    }
    rerender(<BbsRosterAvatar deviceId="peer" name="颦儿" avatar={{ kind: 'builtin', id: 'violet' }} />);
    expect(container.firstElementChild).toHaveClass('system-violet'); expect(classSpy).not.toHaveBeenCalled(); expect(styleSpy).not.toHaveBeenCalled();
  });
  it('waits for visibility, drops offscreen images and leaves no subscriptions after close', () => {
    const { container, unmount } = render(<BbsRosterAvatar deviceId="peer" name="颦儿" avatar={descriptor()} />);
    expect(bbsRosterAvatarImages.subscribe).not.toHaveBeenCalled();
    act(() => observers[0].change(true)); act(() => callbacks[0](data)); expect(container.querySelector('img')).toHaveAttribute('src', data);
    expect(bbsRosterAvatarImages.subscribe).toHaveBeenCalledWith(ref(), expect.any(Function));
    act(() => observers[0].change(false)); expect(stops[0]).toHaveBeenCalledOnce(); expect(container.querySelector('img')).toBeNull();
    act(() => callbacks[0](data)); expect(container.querySelector('img')).toBeNull();
    act(() => observers[0].change(true)); act(() => callbacks[1](data)); unmount();
    expect(stops[1]).toHaveBeenCalledOnce(); expect(observers[0].disconnect).toHaveBeenCalledOnce();
  });
  it('is late-callback safe across device/hash changes, and decode failure does not retry', () => {
    const { container, rerender } = render(<BbsRosterAvatar deviceId="peer" name="颦儿" avatar={descriptor()} />);
    act(() => observers[0].change(true));
    rerender(<BbsRosterAvatar deviceId="other" name="晴雯" avatar={descriptor()} />);
    expect(stops[0]).toHaveBeenCalledOnce(); act(() => observers[1].change(true)); act(() => callbacks[0](data));
    expect(container).toHaveTextContent('晴'); expect(container.querySelector('img')).toBeNull();
    rerender(<BbsRosterAvatar deviceId="other" name="晴雯" avatar={descriptor(true, 'b'.repeat(64))} />);
    act(() => observers[2].change(true)); act(() => callbacks[1](data)); expect(container.querySelector('img')).toBeNull();
    act(() => callbacks[2](data)); fireEvent.error(container.querySelector('img')!);
    expect(container.querySelector('img')).toBeNull(); expect(container).toHaveTextContent('晴');
    expect(bbsRosterAvatarImages.forget).toHaveBeenCalledExactlyOnceWith(ref('other', 'b'.repeat(64)));
    expect(bbsRosterAvatarImages.subscribe).toHaveBeenCalledTimes(3);
    rerender(<BbsRosterAvatar deviceId="other" name="晴雯" avatar={descriptor(false)} />);
    expect(stops[2]).toHaveBeenCalledOnce();
  });
});
