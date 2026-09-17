import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { BbsSyncView } from '../src/bbs-sync-view';
import { useBbsSyncView, type BbsSyncViewSource } from '../src/chrome/useBbsSyncView';

const snapshot = (groupId: string | null = 'group-one'): BbsSyncView => ({
  deviceId: 'self', deviceName: 'Mac', workerAvailable: false, invitation: { state: 'none' },
  group: groupId ? { id: groupId, name: 'Group', role: 'member', members: [] } : null,
  phase: 'idle', progress: null, lastSuccessfulAt: null, error: null, controlRecoverable: false, serviceRecoverable: false,
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

function source() {
  let hint: () => void = () => {};
  const stop = vi.fn();
  const io: BbsSyncViewSource = {
    read: vi.fn().mockResolvedValue(snapshot()),
    listen: vi.fn().mockImplementation(async (changed: () => void) => { hint = changed; return stop; }),
  };
  return { io, stop, changed: () => hint() };
}

beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(new Date('2026-09-12T00:00:00Z')); });
afterEach(() => vi.useRealTimers());

describe('BBS visible-state subscription', () => {
  it('shares one refresh barrier and waits for a read begun after the request, not an older in-flight read', async () => {
    const task = source();
    const old = deferred<BbsSyncView>(), fresh = deferred<BbsSyncView>();
    vi.mocked(task.io.read).mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
    const hook = renderHook(() => useBbsSyncView(true, task.io));
    await act(async () => {});
    const done = vi.fn();
    const barrier = hook.result.current.refresh();
    void barrier.then(done);
    for (let i = 0; i < 1000; i++) expect(hook.result.current.refresh()).toBe(barrier);
    await act(async () => old.resolve(snapshot()));
    expect(done).not.toHaveBeenCalled();
    await act(async () => vi.advanceTimersByTimeAsync(500));
    expect(task.io.read).toHaveBeenCalledTimes(2);
    await act(async () => fresh.resolve({ ...snapshot(), phase: 'connecting' }));
    expect(done).toHaveBeenCalledOnce();
    expect(hook.result.current.view?.phase).toBe('connecting');
  });

  it('settles a refresh barrier on read failure or close without leaking a waiter or starting another read', async () => {
    const task = source();
    const hook = renderHook(() => useBbsSyncView(true, task.io));
    await act(async () => {});
    vi.mocked(task.io.read).mockRejectedValueOnce(new Error('PRIVATE'));
    const failed = vi.fn();
    void hook.result.current.refresh().then(failed);
    await act(async () => vi.advanceTimersByTimeAsync(500));
    expect(failed).toHaveBeenCalledOnce();
    expect(hook.result.current.error).not.toContain('PRIVATE');
    const closed = vi.fn();
    void hook.result.current.refresh().then(closed);
    hook.unmount();
    await act(async () => {});
    expect(closed).toHaveBeenCalledOnce();
    await act(async () => vi.advanceTimersByTimeAsync(60_000));
    expect(task.io.read).toHaveBeenCalledTimes(2);
  });

  it('does nothing while closed and starts with listen before reading', async () => {
    const task = source();
    const listening = deferred<() => void>();
    vi.mocked(task.io.listen).mockReturnValue(listening.promise);
    const hook = renderHook(({ open }) => useBbsSyncView(open, task.io), { initialProps: { open: false } });
    expect(task.io.read).not.toHaveBeenCalled();
    expect(task.io.listen).not.toHaveBeenCalled();
    await act(async () => hook.rerender({ open: true }));
    expect(task.io.listen).toHaveBeenCalledTimes(1);
    expect(task.io.read).not.toHaveBeenCalled();
    await act(async () => listening.resolve(task.stop));
    expect(task.io.read).toHaveBeenCalledTimes(1);
    expect(hook.result.current.view?.group?.id).toBe('group-one');
  });

  it('bounds an event flood during hydration to one follow-up read', async () => {
    const task = source();
    const first = deferred<BbsSyncView>();
    vi.mocked(task.io.read).mockReturnValueOnce(first.promise);
    const hook = renderHook(() => useBbsSyncView(true, task.io));
    await act(async () => {});
    for (let i = 0; i < 1000; i++) task.changed();
    await act(async () => first.resolve(snapshot()));
    expect(task.io.read).toHaveBeenCalledTimes(1);
    await act(async () => vi.advanceTimersByTimeAsync(500));
    expect(task.io.read).toHaveBeenCalledTimes(2);
    expect(hook.result.current.loading).toBe(false);
    await act(async () => vi.advanceTimersByTimeAsync(499));
    expect(task.io.read).toHaveBeenCalledTimes(2);
  });

  it('limits reads to two per second without delaying a busy stream forever', async () => {
    const task = source();
    renderHook(() => useBbsSyncView(true, task.io));
    await act(async () => {});
    for (let i = 0; i < 9; i++) {
      task.changed();
      await act(async () => vi.advanceTimersByTimeAsync(100));
    }
    expect(task.io.read).toHaveBeenCalledTimes(2);
    await act(async () => vi.advanceTimersByTimeAsync(100));
    expect(task.io.read).toHaveBeenCalledTimes(3);
  });

  it('uses a visible fallback for missed hints, stops it on close, and never invokes sync/cancel', async () => {
    const task = source();
    const hook = renderHook(({ open }) => useBbsSyncView(open, task.io), { initialProps: { open: true } });
    await act(async () => {});
    await act(async () => vi.advanceTimersByTimeAsync(5000));
    expect(task.io.read).toHaveBeenCalledTimes(2);
    await act(async () => hook.rerender({ open: false }));
    expect(task.stop).toHaveBeenCalledTimes(1);
    await act(async () => vi.advanceTimersByTimeAsync(60_000));
    task.changed();
    expect(task.io.read).toHaveBeenCalledTimes(2);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('does not poll an unjoined device when the listener works', async () => {
    const task = source();
    vi.mocked(task.io.read).mockResolvedValue(snapshot(null));
    renderHook(() => useBbsSyncView(true, task.io));
    await act(async () => {});
    await act(async () => vi.advanceTimersByTimeAsync(60_000));
    expect(task.io.read).toHaveBeenCalledTimes(1);
    task.changed();
    await act(async () => vi.advanceTimersByTimeAsync(0));
    expect(task.io.read).toHaveBeenCalledTimes(2);
  });

  it('unlistens a registration that finishes after the panel has closed', async () => {
    const task = source();
    const registration = deferred<() => void>();
    vi.mocked(task.io.listen).mockReturnValue(registration.promise);
    const hook = renderHook(() => useBbsSyncView(true, task.io));
    hook.unmount();
    await act(async () => registration.resolve(task.stop));
    expect(task.stop).toHaveBeenCalledTimes(1);
    expect(task.io.read).not.toHaveBeenCalled();
  });

  it('does not install an old response over a reopened view', async () => {
    const task = source();
    const old = deferred<BbsSyncView>();
    vi.mocked(task.io.read).mockReturnValueOnce(old.promise).mockResolvedValue(snapshot('group-two'));
    const hook = renderHook(({ open }) => useBbsSyncView(open, task.io), { initialProps: { open: true } });
    await act(async () => {});
    await act(async () => hook.rerender({ open: false }));
    await act(async () => hook.rerender({ open: true }));
    expect(hook.result.current.view?.group?.id).toBe('group-two');
    await act(async () => old.resolve(snapshot('group-one')));
    expect(hook.result.current.view?.group?.id).toBe('group-two');
  });

  it('preserves the last snapshot on read failure and recovers without a retry storm', async () => {
    const task = source();
    const hook = renderHook(() => useBbsSyncView(true, task.io));
    await act(async () => {});
    vi.mocked(task.io.read).mockRejectedValueOnce({ secret: 'never render' });
    task.changed();
    await act(async () => vi.advanceTimersByTimeAsync(500));
    expect(hook.result.current.view?.group?.id).toBe('group-one');
    expect(hook.result.current.error).toBe('Could not refresh device sync status.');
    expect(task.io.read).toHaveBeenCalledTimes(2);
    await act(async () => vi.advanceTimersByTimeAsync(4999));
    expect(task.io.read).toHaveBeenCalledTimes(2);
    await act(async () => vi.advanceTimersByTimeAsync(1));
    expect(task.io.read).toHaveBeenCalledTimes(3);
    expect(hook.result.current.error).toBeNull();
  });

  it('falls back visibly when event registration fails, including before first join', async () => {
    const task = source();
    vi.mocked(task.io.listen).mockRejectedValue(new Error('listener failed'));
    vi.mocked(task.io.read).mockResolvedValue(snapshot(null));
    const hook = renderHook(() => useBbsSyncView(true, task.io));
    await act(async () => {});
    expect(hook.result.current.error).toContain('Live status updates are unavailable.');
    expect(task.io.read).toHaveBeenCalledTimes(1);
    await act(async () => vi.advanceTimersByTimeAsync(5000));
    expect(task.io.read).toHaveBeenCalledTimes(2);
  });
});
