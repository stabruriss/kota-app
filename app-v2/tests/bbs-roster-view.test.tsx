import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { BbsRosterClientError } from '../src/bbs-roster-client';
import { useBbsRoster, type BbsRosterSource } from '../src/chrome/useBbsRoster';
import type { BbsRosterView } from '../src/types/bbs-roster';

const view = (v = 'a'): BbsRosterView => ({ version: v.repeat(64), devices: [] });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(res => { resolve = res; }); return { promise, resolve }; }
function source() {
  let changed = () => {}; const stop = vi.fn();
  const io: BbsRosterSource = { read: vi.fn().mockResolvedValue(view()), listen: vi.fn(async cb => { changed = cb; return stop; }) };
  return { io, stop, hint: () => changed() };
}
beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(new Date('2026-09-13T00:00:00Z')); });
afterEach(() => vi.useRealTimers());

describe('open-only roster subscription', () => {
  it('listens before hydrate, imports/closed view do nothing, and never polls', async () => {
    const task = source(), registration = deferred<() => void>(); vi.mocked(task.io.listen).mockReturnValue(registration.promise);
    const hook = renderHook(({ open }) => useBbsRoster(open, task.io), { initialProps: { open: false } });
    expect(task.io.listen).not.toHaveBeenCalled(); expect(task.io.read).not.toHaveBeenCalled();
    await act(async () => hook.rerender({ open: true })); expect(task.io.listen).toHaveBeenCalledOnce(); expect(task.io.read).not.toHaveBeenCalled();
    await act(async () => registration.resolve(task.stop)); expect(task.io.read).toHaveBeenCalledOnce();
    await act(async () => vi.advanceTimersByTimeAsync(60000)); expect(task.io.read).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
  });
  it('coalesces a thousand hints into one rate-limited follow-up without an event queue', async () => {
    const task = source(), first = deferred<BbsRosterView>(); vi.mocked(task.io.read).mockReturnValueOnce(first.promise);
    const hook = renderHook(() => useBbsRoster(true, task.io)); await act(async () => {});
    for (let i = 0; i < 1000; i++) task.hint();
    await act(async () => first.resolve(view())); expect(task.io.read).toHaveBeenCalledOnce();
    await act(async () => vi.advanceTimersByTimeAsync(500)); expect(task.io.read).toHaveBeenCalledTimes(2);
    for (let i = 0; i < 4; i++) { hook.result.current.refresh(); await act(async () => vi.advanceTimersByTimeAsync(100)); }
    expect(task.io.read).toHaveBeenCalledTimes(2);
    await act(async () => vi.advanceTimersByTimeAsync(100)); expect(task.io.read).toHaveBeenCalledTimes(3);
  });
  it('retains a complete view across failure/changed pages and bounds changed restarts', async () => {
    const task = source(); const hook = renderHook(() => useBbsRoster(true, task.io)); await act(async () => {});
    vi.mocked(task.io.read).mockRejectedValue(new BbsRosterClientError('roster_changed')); task.hint();
    await act(async () => vi.advanceTimersByTimeAsync(500)); expect(hook.result.current.view).toEqual(view());
    await act(async () => vi.advanceTimersByTimeAsync(500)); expect(task.io.read).toHaveBeenCalledTimes(3);
    await act(async () => vi.advanceTimersByTimeAsync(60000)); expect(task.io.read).toHaveBeenCalledTimes(3);
    vi.mocked(task.io.read).mockResolvedValue(view('b')); hook.result.current.refresh();
    await act(async () => vi.advanceTimersByTimeAsync(0)); expect(hook.result.current.view).toEqual(view('b')); expect(hook.result.current.error).toBeNull();
  });
  it('redacts arbitrary failures, clears listeners/timers and cancels old page chains on close', async () => {
    const task = source(), late = deferred<BbsRosterView>(); vi.mocked(task.io.read).mockReturnValueOnce(late.promise);
    const hook = renderHook(({ open }) => useBbsRoster(open, task.io), { initialProps: { open: true } }); await act(async () => {});
    const cancelled = vi.mocked(task.io.read).mock.calls[0][0]; expect(cancelled()).toBe(false);
    await act(async () => hook.rerender({ open: false })); expect(cancelled()).toBe(true); expect(task.stop).toHaveBeenCalledOnce();
    await act(async () => hook.rerender({ open: true })); expect(hook.result.current.view).toEqual(view());
    await act(async () => late.resolve(view('c'))); expect(hook.result.current.view).toEqual(view());
    vi.mocked(task.io.read).mockRejectedValueOnce({ secret: 'SECRET' }); task.hint();
    await act(async () => vi.advanceTimersByTimeAsync(500)); expect(hook.result.current.error).toBe('Could not refresh the agent roster.');
    hook.unmount(); expect(vi.getTimerCount()).toBe(0);
  });
  it('bounds listener wait, reads anyway and catches the gap when a listener finally arrives', async () => {
    const task = source(), registration = deferred<() => void>(); vi.mocked(task.io.listen).mockReturnValue(registration.promise);
    const hook = renderHook(() => useBbsRoster(true, task.io));
    await act(async () => vi.advanceTimersByTimeAsync(1999)); expect(task.io.read).not.toHaveBeenCalled();
    await act(async () => vi.advanceTimersByTimeAsync(1)); expect(task.io.read).toHaveBeenCalledOnce(); expect(hook.result.current.error).toMatch(/Live roster updates/);
    await act(async () => registration.resolve(task.stop)); await act(async () => vi.advanceTimersByTimeAsync(500));
    expect(task.io.read).toHaveBeenCalledTimes(2); expect(hook.result.current.error).toBeNull();
    hook.unmount(); expect(task.stop).toHaveBeenCalledOnce();
  });
  it('late listener is stopped after unmount; failed listener keeps manual retry available', async () => {
    const task = source(), registration = deferred<() => void>(); vi.mocked(task.io.listen).mockReturnValueOnce(registration.promise);
    const hook = renderHook(() => useBbsRoster(true, task.io)); hook.unmount();
    await act(async () => registration.resolve(task.stop)); expect(task.stop).toHaveBeenCalledOnce(); expect(task.io.read).not.toHaveBeenCalled();
    vi.mocked(task.io.listen).mockRejectedValue(new Error('SECRET'));
    const again = renderHook(() => useBbsRoster(true, task.io)); await act(async () => {});
    expect(again.result.current.view).toEqual(view()); expect(again.result.current.error).toMatch(/Live roster updates/);
    again.result.current.refresh(); await act(async () => vi.advanceTimersByTimeAsync(500)); expect(task.io.read).toHaveBeenCalledTimes(2);
  });
});
