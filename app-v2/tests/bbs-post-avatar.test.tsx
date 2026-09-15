import { act, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as hero from '../src/lib/hero-avatars';
import type { BbsPost } from '../src/pty-client';

vi.mock('../src/bbs-sync-avatars', async (original) => ({
  ...await original<typeof import('../src/bbs-sync-avatars')>(),
  bbsSyncAvatarImages: { subscribe: vi.fn(), forget: vi.fn() },
}));
import { bbsSyncAvatarImages } from '../src/bbs-sync-avatars';
import { BbsPostAvatar } from '../src/chrome/BbsPostAvatar';

const a = 'a'.repeat(64), b = 'b'.repeat(64);
const data = 'data:image/png;base64,YQ==';
const local = { agentId: 'hero-cc', agentAvatar: 'user:local-avatar' };
const imagePost = (sha256 = a, available = true): Pick<BbsPost, 'agentId' | 'agentAvatar' | 'syncAvatar'> => ({
  ...local, syncAvatar: { kind: 'image', sha256, ext: 'png', available, localPath: available ? '/private/origin.png' : null },
});
const observers: Observer[] = [];
class Observer {
  element!: Element;
  constructor(private callback: IntersectionObserverCallback) { observers.push(this); }
  observe(element: Element) { this.element = element; }
  disconnect = vi.fn();
  change(isIntersecting: boolean) {
    this.callback([{ target: this.element, isIntersecting } as IntersectionObserverEntry], this as unknown as IntersectionObserver);
  }
}
let callbacks: ((src: string | null) => void)[];
let stops: ReturnType<typeof vi.fn>[];
beforeEach(() => {
  vi.restoreAllMocks(); vi.clearAllMocks(); observers.length = 0; callbacks = []; stops = [];
  vi.stubGlobal('IntersectionObserver', Observer);
  vi.mocked(bbsSyncAvatarImages.subscribe).mockImplementation((_ref, callback) => {
    callbacks.push(callback); const stop = vi.fn(); stops.push(stop); return stop;
  });
});
afterEach(() => vi.unstubAllGlobals());

describe('BBS post origin avatar presentation', () => {
  it('preserves local author/meta/human fallback and never starts a new read', () => {
    const avatarClass = vi.spyOn(hero, 'avatarClassForId').mockReturnValue('user-avatar');
    const style = vi.spyOn(hero, 'avatarImageStyleForId').mockReturnValue({ backgroundImage: `url("${data}")`, backgroundSize: '114%',
      backgroundPosition: 'center', backgroundRepeat: 'no-repeat', borderRadius: '50%', overflow: 'hidden' });
    const { container, rerender } = render(<BbsPostAvatar post={local} />);
    expect(avatarClass).toHaveBeenCalledWith('user:local-avatar', null);
    expect(style).toHaveBeenCalledWith('user:local-avatar');
    expect(container.firstElementChild).toHaveClass('bbs-msg-avatar', 'user-avatar');
    expect(container.firstElementChild).toHaveStyle({ backgroundSize: '114%' });
    rerender(<BbsPostAvatar post={{ agentId: 'human' }} small />);
    expect(avatarClass).toHaveBeenLastCalledWith('user-default', null);
    rerender(<BbsPostAvatar post={{ agentId: 'hero-cc' }} meta={{ avatarId: 'claude' }} />);
    expect(avatarClass).toHaveBeenLastCalledWith('claude', null);
    expect(bbsSyncAvatarImages.subscribe).not.toHaveBeenCalled(); expect(observers).toHaveLength(0);
  });

  it('renders only whitelisted remote builtins and never consults the local hero cache', () => {
    const classSpy = vi.spyOn(hero, 'avatarClassForId'), styleSpy = vi.spyOn(hero, 'avatarImageStyleForId');
    const fallbackSpy = vi.spyOn(hero, 'avatarClassForAgentFallback');
    const { container, rerender } = render(<BbsPostAvatar post={{ ...local, syncAvatar: { kind: 'builtin', id: 'violet' } }} meta={{ avatarId: 'claude' }} />);
    expect(container.firstElementChild).toHaveClass('system-violet');
    for (const origin of [{ kind: 'none' }, { kind: 'builtin', id: 'user:local-avatar' }, null, { kind: 'future' }]) {
      rerender(<BbsPostAvatar post={{ ...local, syncAvatar: origin as never }} meta={{ avatarId: 'claude' }} />);
      expect(container.firstElementChild).toHaveClass('provider-codex');
      expect(container.firstElementChild).not.toHaveStyle({ backgroundImage: `url("${data}")` });
    }
    expect(classSpy).not.toHaveBeenCalled(); expect(styleSpy).not.toHaveBeenCalled(); expect(fallbackSpy).not.toHaveBeenCalled();
    expect(bbsSyncAvatarImages.subscribe).not.toHaveBeenCalled();
  });

  it('defers visible images, passes only hash/ext, releases offscreen data and disconnects on close', async () => {
    const { container, unmount } = render(<BbsPostAvatar post={imagePost()} small />);
    expect(bbsSyncAvatarImages.subscribe).not.toHaveBeenCalled();
    expect(container.firstElementChild).toHaveClass('sm', 'provider-codex');
    act(() => observers[0].change(true));
    expect(bbsSyncAvatarImages.subscribe).toHaveBeenCalledWith({ sha256: a, ext: 'png' }, expect.any(Function));
    act(() => callbacks[0](data));
    expect(container.querySelector('img')).toHaveAttribute('src', data);
    expect(container.firstElementChild).toHaveAttribute('aria-hidden', 'true');
    act(() => observers[0].change(false));
    expect(stops[0]).toHaveBeenCalledOnce();
    expect(container.querySelector('img')).toBeNull();
    act(() => callbacks[0](data)); expect(container.querySelector('img')).toBeNull();
    act(() => observers[0].change(true)); await waitFor(() => expect(callbacks).toHaveLength(2));
    unmount(); expect(stops[1]).toHaveBeenCalledOnce(); expect(observers[0].disconnect).toHaveBeenCalledOnce();
    act(() => callbacks[1](data)); expect(container).toBeEmptyDOMElement();
  });

  it('keeps unavailable avatars default until a verified snapshot makes them readable', () => {
    const { container, rerender } = render(<BbsPostAvatar post={imagePost(a, false)} />);
    expect(observers).toHaveLength(0); expect(bbsSyncAvatarImages.subscribe).not.toHaveBeenCalled();
    expect(container.firstElementChild).toHaveClass('provider-codex');
    rerender(<BbsPostAvatar post={imagePost()} />);
    act(() => observers[0].change(true)); act(() => callbacks[0](null));
    expect(container.querySelector('img')).toBeNull(); expect(bbsSyncAvatarImages.subscribe).toHaveBeenCalledOnce();
  });

  it('isolates old hash callbacks and clears failed decode without automatic retries', () => {
    const { container, rerender } = render(<BbsPostAvatar post={imagePost()} />);
    act(() => observers[0].change(true));
    rerender(<BbsPostAvatar post={imagePost(b)} />);
    expect(stops[0]).toHaveBeenCalledOnce();
    act(() => observers[1].change(true)); act(() => callbacks[0](data));
    expect(container.querySelector('img')).toBeNull();
    act(() => callbacks[1](data));
    fireEvent.error(container.querySelector('img')!);
    expect(container.querySelector('img')).toBeNull();
    expect(bbsSyncAvatarImages.forget).toHaveBeenCalledExactlyOnceWith({ sha256: b, ext: 'png' });
    expect(bbsSyncAvatarImages.subscribe).toHaveBeenCalledTimes(2);
  });
});
