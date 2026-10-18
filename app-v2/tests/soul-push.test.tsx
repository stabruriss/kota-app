import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useSoulPushSettings, type SoulPushSettingsSnapshot } from '../src/lib/soul-push';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
const listeners = new Map<string, (event: { payload: unknown }) => void>();
const stop = vi.fn();
const initial = (): SoulPushSettingsSnapshot => ({ sourceRevision: 'fixture-1', jevReady: true, settings: {
  schema: 1, enabled: true, markThreshold: .85, dreamThreshold: .90, maxMarks: 3, maxDreams: 2, cooldownReplies: 10, minimumMessageLength: 60,
} });
const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
};
let disk: SoulPushSettingsSnapshot;
let revision: number;
const writes = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === 'puppeteer_soul_save');
const push = (payload: SoulPushSettingsSnapshot) => act(() => { listeners.get('puppeteer-soul://changed')?.({ payload }); });
beforeEach(() => {
  vi.resetAllMocks(); listeners.clear(); disk = initial(); revision = 1;
  vi.mocked(listen).mockImplementation(async (name, callback) => { listeners.set(name, callback as never); return stop; });
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === 'puppeteer_soul_read') return structuredClone(disk) as never;
    if (command !== 'puppeteer_soul_save') throw new Error('unexpected command');
    const request = args as { settings: SoulPushSettingsSnapshot['settings']; sourceRevision: string };
    if (request.sourceRevision !== disk.sourceRevision) throw { code: 'soul_settings_conflict' };
    disk = { ...disk, sourceRevision: `fixture-${++revision}`, settings: { ...request.settings } };
    return structuredClone(disk) as never;
  });
});

it('subscribes before reading, takes first-use enabled from the backend and releases both listeners', async () => {
  disk.settings.enabled = false;
  const { result, unmount } = renderHook(useSoulPushSettings);
  expect(result.current.settings).toBeNull();
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.settings?.enabled).toBe(false);
  expect(vi.mocked(listen).mock.invocationCallOrder[1]).toBeLessThan(vi.mocked(invoke).mock.invocationCallOrder[0]);
  expect(writes()).toHaveLength(0);
  unmount(); expect(stop).toHaveBeenCalledTimes(2);
});

it('preserves an editing draft and its original revision across an external update; conflict requires Reload', async () => {
  const { result } = renderHook(useSoulPushSettings);
  await waitFor(() => expect(result.current.settings).not.toBeNull());
  act(() => result.current.beginEdit());
  disk = { ...disk, sourceRevision: 'external', settings: { ...disk.settings, maxMarks: 5 } };
  push(disk);
  expect(result.current.settings?.maxMarks).toBe(3);
  act(() => result.current.commit({ markThreshold: .70 }));
  await waitFor(() => expect(result.current.error?.code).toBe('soul_settings_conflict'));
  expect(writes()[0][1]).toMatchObject({ sourceRevision: 'fixture-1', settings: { markThreshold: .70, maxMarks: 3 } });
  expect(result.current.settings?.markThreshold).toBe(.70);
  act(() => result.current.retry());
  expect(writes()).toHaveLength(1);
  act(() => result.current.reload());
  await waitFor(() => expect(result.current.error).toBeNull());
  expect(result.current.settings?.maxMarks).toBe(5);
  expect(result.current.settings?.markThreshold).toBe(.85);
});

it('keeps a failed draft without automatic retry, then explicitly retries the same CAS baseline', async () => {
  const { result } = renderHook(useSoulPushSettings);
  await waitFor(() => expect(result.current.settings).not.toBeNull());
  vi.mocked(invoke).mockRejectedValueOnce({ code: 'soul_settings_write_failed', message: 'PRIVATE_PATH' });
  act(() => result.current.commit({ cooldownReplies: 0 }));
  await waitFor(() => expect(result.current.error?.operation).toBe('save'));
  act(() => result.current.commit({ minimumMessageLength: 0 }));
  expect(writes()).toHaveLength(1);
  expect(result.current.settings).toMatchObject({ cooldownReplies: 0, minimumMessageLength: 0 });
  act(() => result.current.retry());
  await waitFor(() => expect(result.current.busy).toBe(false));
  expect(writes()).toHaveLength(2);
  expect(writes()[1][1]).toMatchObject({ sourceRevision: 'fixture-1', settings: { cooldownReplies: 0, minimumMessageLength: 0 } });
  expect(result.current.error).toBeNull();
});

it('saves one action at a time and uses the previous successful revision for the next action', async () => {
  const { result } = renderHook(useSoulPushSettings);
  await waitFor(() => expect(result.current.settings).not.toBeNull());
  const pending = deferred<SoulPushSettingsSnapshot>();
  vi.mocked(invoke).mockImplementationOnce(() => pending.promise as never);
  act(() => { result.current.commit({ maxMarks: 4 }); result.current.commit({ maxMarks: 5 }); });
  expect(writes()).toHaveLength(1); expect(result.current.busy).toBe(true);
  disk = { ...disk, sourceRevision: 'saved-action', settings: { ...disk.settings, maxMarks: 4 } };
  await act(async () => pending.resolve(disk));
  act(() => result.current.commit({ maxMarks: 5 }));
  await waitFor(() => expect(result.current.busy).toBe(false));
  expect(writes()[1][1]).toMatchObject({ sourceRevision: 'saved-action', settings: { maxMarks: 5 } });
});

it('does not let a late save response replace a newer external event', async () => {
  const { result } = renderHook(useSoulPushSettings);
  await waitFor(() => expect(result.current.settings).not.toBeNull());
  const pending = deferred<SoulPushSettingsSnapshot>();
  vi.mocked(invoke).mockImplementationOnce(() => pending.promise as never);
  act(() => result.current.commit({ maxDreams: 4 }));
  const saved = { ...disk, sourceRevision: 'saved', settings: { ...disk.settings, maxDreams: 4 } };
  push(saved);
  const external = { ...saved, sourceRevision: 'newer', settings: { ...saved.settings, maxDreams: 1 } };
  push(external);
  expect(result.current.settings?.maxDreams).toBe(4);
  await act(async () => pending.resolve(saved));
  expect(result.current.settings?.maxDreams).toBe(1);
  expect(result.current.snapshot?.sourceRevision).toBe('newer');
});

it('ignores a late read after a changed event and never replaces unread settings with UI defaults', async () => {
  const pending = deferred<SoulPushSettingsSnapshot>();
  vi.mocked(invoke).mockImplementationOnce(() => pending.promise as never);
  const { result } = renderHook(useSoulPushSettings);
  await waitFor(() => expect(invoke).toHaveBeenCalledOnce());
  const external = { ...disk, sourceRevision: 'newer', settings: { ...disk.settings, enabled: false } };
  push(external);
  await act(async () => pending.resolve(initial()));
  expect(result.current.settings?.enabled).toBe(false);
  expect(result.current.snapshot?.sourceRevision).toBe('newer');
});

it('keeps read failures empty until an explicit retry succeeds', async () => {
  vi.mocked(invoke).mockRejectedValueOnce({ code: 'soul_settings_invalid' });
  const { result } = renderHook(useSoulPushSettings);
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.settings).toBeNull(); expect(result.current.jevReady).toBeNull();
  expect(writes()).toHaveLength(0);
  act(() => result.current.retry());
  await waitFor(() => expect(result.current.settings?.schema).toBe(1));
  expect(result.current.error).toBeNull();
});

it('refreshes Jev readiness without resetting persisted enabled or an editing draft', async () => {
  const { result } = renderHook(useSoulPushSettings);
  await waitFor(() => expect(result.current.settings).not.toBeNull());
  act(() => result.current.beginEdit());
  disk = { ...disk, jevReady: false };
  act(() => listeners.get('local-jev://changed')?.({ payload: { ready: false } }));
  await waitFor(() => expect(result.current.jevReady).toBe(false));
  expect(result.current.settings?.enabled).toBe(true); expect(result.current.draft).not.toBeNull();
  expect(writes()).toHaveLength(0);
});

it('does not unlock Jev from a stale save response after the key was removed', async () => {
  const { result } = renderHook(useSoulPushSettings);
  await waitFor(() => expect(result.current.settings).not.toBeNull());
  const pending = deferred<SoulPushSettingsSnapshot>();
  vi.mocked(invoke).mockImplementationOnce(() => pending.promise as never);
  act(() => result.current.commit({ maxMarks: 4 }));
  disk = { ...disk, sourceRevision: 'saved', jevReady: false, settings: { ...disk.settings, maxMarks: 4 } };
  act(() => listeners.get('local-jev://changed')?.({ payload: { ready: false } }));
  await waitFor(() => expect(result.current.jevReady).toBe(false));
  await act(async () => pending.resolve({ ...disk, jevReady: true }));
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.jevReady).toBe(false); expect(writes()).toHaveLength(1);
});
