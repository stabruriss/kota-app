import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { bbsRosterRead, onBbsRosterChanged, parseBbsRosterPage, readCompleteBbsRoster } from '../src/bbs-roster-client';
import type { BbsRosterRow } from '../src/types/bbs-roster';

const version = 'a'.repeat(64);
const device = (deviceId: string | null = null, local = true, rosterStatus: 'synced' | 'not_synced' = 'synced'): BbsRosterRow =>
  ({ kind: 'device', deviceId, name: 'Same Mac', local, online: local, rosterStatus, receivedAt: null });
const project = (deviceId: string | null = null, projectId = 'p'): BbsRosterRow => ({ kind: 'project', deviceId, projectId, name: 'Kota' });
const agent = (deviceId: string | null = null, agentId = 'a', avatar: unknown = { kind: 'none' }): BbsRosterRow =>
  ({ kind: 'agent', deviceId, projectId: 'p', agentId, name: 'Same name', targetRef: `${deviceId ?? 'local'}/p/${agentId}`, avatar } as BbsRosterRow);
const page = (items: BbsRosterRow[], next: string | null = null, v = version) => ({ version: v, items, next });
const complete = async (items: BbsRosterRow[]) => { vi.mocked(invoke).mockResolvedValueOnce(page(items)); return readCompleteBbsRoster(); };
beforeEach(() => { vi.clearAllMocks(); vi.mocked(isTauri).mockReturnValue(true); });

describe('public roster memory client', () => {
  it('is inert on import and reads only the exact paged IPC', async () => {
    vi.resetModules(); await import('../src/bbs-roster-client');
    expect(invoke).not.toHaveBeenCalled(); expect(listen).not.toHaveBeenCalled();
    vi.mocked(invoke).mockResolvedValueOnce(page([device(), project(), agent()], '3'))
      .mockResolvedValueOnce(page([device('peer', false), project('peer'), agent('peer')], '6'))
      .mockResolvedValueOnce(page([device('offline', false, 'not_synced')]));
    const view = await readCompleteBbsRoster();
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ['bbs_roster_read', { request: { version: null, after: null } }],
      ['bbs_roster_read', { request: { version, after: '3' } }],
      ['bbs_roster_read', { request: { version, after: '6' } }],
    ]);
    expect(view.devices).toHaveLength(3);
    expect(view.devices[0].projects?.[0].agents[0].targetRef).toBe('local/p/a');
    expect(view.devices[1].projects?.[0].agents[0].targetRef).toBe('peer/p/a');
    expect(view.devices[2].projects).toBeNull();
    expect((await complete([device()])).devices[0].projects).toEqual([]);
  });
  it('projects public fields, preserves future builtin IDs, and keeps available separate', async () => {
    const rows = [device(), project(), { ...agent(null, 'builtin', { kind: 'builtin', id: 'future-hero' }), path: '/private', token: 'secret' },
      agent(null, 'image', { kind: 'image', sha256: version, ext: 'png', sizeBytes: 600000, available: false })];
    const view = await complete(rows);
    expect(view.devices[0].projects?.[0].agents[0].avatar).toEqual({ kind: 'builtin', id: 'future-hero' });
    expect(view.devices[0].projects?.[0].agents[1].avatar).toMatchObject({ available: false, sizeBytes: 600000 });
    expect(JSON.stringify(view)).not.toMatch(/private|secret/);
  });
  it('rejects malformed pages/IDs/avatars without coercion or URL access', () => {
    for (const value of [null, {}, page([], '01'), page([], '-1'), page([], '1.5'), page([], '9007199254740992'),
      page([], null, 'A'.repeat(64)), page(Array(65).fill(device())), page([{ ...device(), local: 'true' } as never]),
      page([device(null, false)]), page([{ ...project(), projectId: '../p' } as never]),
      page([agent(null, 'bad', { kind: 'image', sha256: version, ext: 'svg', sizeBytes: 1, available: true })]),
      page([agent(null, 'bad', { kind: 'image', sha256: version, ext: 'png', sizeBytes: 600001, available: true })]),
      page([agent(null, 'bad', { kind: 'builtin', id: 'user:local' })]),
      page([{ ...device(), name: '汉'.repeat(6000) } as never])]) expect(() => parseBbsRosterPage(value)).toThrow('incompatible');
  });
  it('validates cross-page hierarchy, full identity uniqueness, refs and monotone offsets', async () => {
    for (const rows of [[project()], [device(), device()], [device(), agent()], [device(), project(), project()],
      [device(), project(), agent(), agent()], [device(), device('peer', false, 'not_synced'), project('peer')],
      [device(), project(), { ...agent(), targetRef: 'peer/p/a' } as BbsRosterRow], [device(), device('another', true)]]) {
      await expect(complete(rows)).rejects.toThrow('incompatible');
    }
    for (const bad of [page([], '0'), page([device()], '0'), page([device()], '2')]) {
      vi.mocked(invoke).mockResolvedValueOnce(bad);
      await expect(readCompleteBbsRoster()).rejects.toThrow('incompatible');
    }
  });
  it('has no total roster cap and keeps a legal 65th agent through multiple pages', async () => {
    const all = [device(), project(), ...Array.from({ length: 130 }, (_, i) => agent(null, `a${i}`))];
    for (let i = 0; i < all.length; i += 32) vi.mocked(invoke).mockResolvedValueOnce(page(all.slice(i, i + 32), i + 32 < all.length ? String(i + 32) : null));
    const view = await readCompleteBbsRoster();
    expect(view.devices[0].projects?.[0].agents).toHaveLength(130);
    expect(view.devices[0].projects?.[0].agents[64].agentId).toBe('a64');
  });
  it('discards a version-changed partial result and honors cancellation before another page', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(page([device()], '1')).mockRejectedValueOnce({ code: 'roster_changed' });
    await expect(readCompleteBbsRoster()).rejects.toMatchObject({ code: 'roster_changed' });
    vi.mocked(invoke).mockResolvedValueOnce(page([device()], '1')).mockResolvedValueOnce(page([project()], null, 'b'.repeat(64)));
    await expect(readCompleteBbsRoster()).rejects.toMatchObject({ code: 'roster_changed' });
    let closed = false;
    const read = vi.fn(async () => { closed = true; return page([device()], '1'); });
    await expect(readCompleteBbsRoster(read, () => closed)).rejects.toMatchObject({ code: 'cancelled' });
    expect(read).toHaveBeenCalledTimes(1);
  });
  it('whitelists only a single-field roster_changed rejection and is runtime-only', async () => {
    for (const error of ['roster_changed SECRET', { code: 'roster_changed', secret: 'SECRET' }, { code: 'future' }, new Error('SECRET')]) {
      vi.mocked(invoke).mockRejectedValueOnce(error);
      await expect(bbsRosterRead({ version: null, after: null })).rejects.toMatchObject({ code: 'read', message: 'Could not refresh the agent roster.' });
    }
    const n = vi.mocked(invoke).mock.calls.length;
    for (const request of [{ version: null, after: '1' }, { version, after: '-1' }, { version, after: '01' }, { version, after: null }]) {
      await expect(bbsRosterRead(request)).rejects.toMatchObject({ code: 'protocol' });
    }
    vi.mocked(isTauri).mockReturnValue(false);
    await expect(bbsRosterRead({ version: null, after: null })).rejects.toMatchObject({ code: 'unavailable' });
    expect(invoke).toHaveBeenCalledTimes(n);
  });
  it('ignores event payload and exposes only the roster hint name', async () => {
    const stop = vi.fn(), hint = vi.fn();
    vi.mocked(listen).mockResolvedValue(stop);
    expect(await onBbsRosterChanged(hint)).toBe(stop);
    expect(listen).toHaveBeenCalledWith('bbs-roster://changed', expect.any(Function));
    (vi.mocked(listen).mock.calls[0][1] as (event: unknown) => void)({ payload: { token: 'SECRET' } });
    expect(hint).toHaveBeenCalledExactlyOnceWith();
  });
});
