import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));

import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  bbsSyncStatus, bbsSyncStatusView, bbsSyncViewSource, onBbsSyncChanged, parseBbsSyncStatus,
  bbsSyncInvitation, bbsSyncJoin, bbsSyncDisconnect, bbsSyncRename, bbsSyncRemove, bbsSyncStart, bbsSyncCancel,
} from '../src/bbs-sync-client';
import type { BbsSyncStatus } from '../src/types/bbs-sync';
import { useBbsSyncView } from '../src/chrome/useBbsSyncView';
import retryRecovery from './fixtures/bbs-retry-recovery.json';

function status(): BbsSyncStatus {
  return {
    protocolVersion: 1, device: { id: 'self', name: 'Mac' }, worker: { configured: true, canCreateGroup: false },
    group: { id: 'group-one', name: 'Studio', role: 'owner', members: [
      { id: 'self', name: 'Mac', role: 'owner', online: true, publicKey: 'public-key' },
      { id: 'peer', name: 'Other Mac', role: 'member', online: false, publicKey: 'other-public-key' },
    ] },
    invitation: 'ready', invitationGeneration: '7', sync: { phase: 'idle', completed: null, total: null, lastSuccessfulAt: null, error: null, controlRecoverable: false },
  };
}

beforeEach(() => { vi.resetAllMocks(); vi.mocked(isTauri).mockReturnValue(true); });

describe('BBS sync IPC status client', () => {
  it('reads the actual Manager recovery snapshots and maps its exact busy rejection', async () => {
    for (const raw of [retryRecovery.busy, retryRecovery.recovered, retryRecovery.controlOverExchange,
      retryRecovery.retainedExchange, retryRecovery.started]) {
      vi.mocked(invoke).mockResolvedValueOnce(raw);
      const parsed = await bbsSyncStatus();
      expect(parsed.sync).toEqual(raw.sync);
      expect(bbsSyncStatusView(parsed).controlRecoverable).toBe(raw.sync.controlRecoverable);
    }
    expect(vi.mocked(invoke).mock.calls).toEqual(Array.from({ length: 5 }, () => ['bbs_sync_status']));
    expect(retryRecovery.busyError).toEqual({ code: 'sync_busy' });
    vi.mocked(invoke).mockRejectedValueOnce(retryRecovery.busyError);
    await expect(bbsSyncStart({ expectedGroupId: retryRecovery.busy.group.id })).rejects.toMatchObject({
      code: 'sync_busy', message: retryRecovery.details.control_in_use,
    });
    expect(listen).not.toHaveBeenCalled();
  });

  it('is inert on import and uses one read-only command with no listener or start', async () => {
    vi.resetModules();
    await import('../src/bbs-sync-client');
    expect(invoke).not.toHaveBeenCalled();
    expect(listen).not.toHaveBeenCalled();
    vi.mocked(invoke).mockResolvedValue(status());
    expect(await bbsSyncStatus()).toEqual(status());
    expect(invoke).toHaveBeenCalledExactlyOnceWith('bbs_sync_status');
    expect(listen).not.toHaveBeenCalled();
  });

  it('rejects browser use without manufacturing membership or success', async () => {
    vi.mocked(isTauri).mockReturnValue(false);
    await expect(bbsSyncStatus()).rejects.toMatchObject({ code: 'unavailable' });
    await expect(onBbsSyncChanged(vi.fn())).rejects.toMatchObject({ code: 'unavailable' });
    expect(invoke).not.toHaveBeenCalled();
    expect(listen).not.toHaveBeenCalled();
  });

  it('rejects unknown protocol versions instead of coercing them', () => {
    for (const protocolVersion of [undefined, '1', 0, 2]) {
      expect(() => parseBbsSyncStatus({ ...status(), protocolVersion })).toThrow(/incompatible/);
    }
  });

  it('validates membership and progress without accepting partial connected state', () => {
    const good = status();
    for (const bad of [
      { ...good, group: { ...good.group, role: null } },
      { ...good, group: { ...good.group, id: null } },
      { ...good, group: { ...good.group, members: [good.group.members[0], good.group.members[0]] } },
      { ...good, sync: { ...good.sync, completed: '1', total: 2 } },
      { ...good, sync: { ...good.sync, completed: 3, total: 2 } },
      { ...good, sync: { ...good.sync, completed: -1, total: 2 } },
      { ...good, sync: { ...good.sync, completed: null, total: 2 } },
      { ...good, sync: { ...good.sync, completed: 0, total: Number.MAX_SAFE_INTEGER + 1 } },
      { ...good, sync: { ...good.sync, phase: 'success' } },
      { ...good, sync: { ...good.sync, phase: ['idle'] } },
      { ...good, invitation: { toString: () => 'ready' } },
      { ...good, sync: { ...good.sync, lastSuccessfulAt: 'not a timestamp' } },
    ]) expect(() => parseBbsSyncStatus(bad)).toThrow(/incompatible/);
  });

  it('accepts an unjoined device without forcing identity initialization', () => {
    const local = { ...status(), invitation: 'none', invitationGeneration: null, device: { id: '', name: 'Mac' }, group: { id: null, name: null, role: null, members: [] } };
    expect(parseBbsSyncStatus(local).group.id).toBeNull();
    expect(bbsSyncStatusView(parseBbsSyncStatus(local)).group).toBeNull();
  });

  it('only enables control recovery from an explicit boolean on a joined status', () => {
    const raw = status();
    const recoverable = { ...raw, sync: { ...raw.sync, controlRecoverable: true } };
    expect(bbsSyncStatusView(parseBbsSyncStatus(recoverable)).controlRecoverable).toBe(true);
    // Preserve old protocol-1 fixtures without granting an unadvertised capability.
    const { controlRecoverable: _, ...legacySync } = raw.sync;
    expect(parseBbsSyncStatus({ ...raw, sync: legacySync }).sync.controlRecoverable).toBe(false);
    for (const controlRecoverable of [null, 'true', 1, [], {}]) {
      expect(() => parseBbsSyncStatus({ ...raw, sync: { ...raw.sync, controlRecoverable } })).toThrow(/incompatible/);
    }
    expect(() => parseBbsSyncStatus({ ...recoverable, invitation: 'none', invitationGeneration: null,
      group: { id: null, name: null, role: null, members: [] } })).toThrow(/incompatible/);
    expect(parseBbsSyncStatus({ ...raw, sync: { ...legacySync, error: 'controlRecoverable=true sync_busy' } }).sync.controlRecoverable).toBe(false);
  });

  it('drops unknown/private fields instead of forwarding the raw IPC object', () => {
    const raw = { ...status(), privateKey: 'SECRET', invitationToken: 'SECRET',
      device: { ...status().device, secret: 'SECRET' },
      group: { ...status().group, secret: 'SECRET', members: status().group.members.map((member) => ({ ...member, secret: 'SECRET' })) },
    };
    expect(JSON.stringify(parseBbsSyncStatus(raw))).not.toContain('SECRET');
    expect(JSON.stringify(bbsSyncStatusView(parseBbsSyncStatus(raw)))).not.toContain('publicKey');
  });

  it('does not display structured or string invocation errors containing secrets', async () => {
    for (const error of [{ secret: 'PRIVATE' }, new Error('PRIVATE'), 'PRIVATE']) {
      vi.mocked(invoke).mockRejectedValueOnce(error);
      await expect(bbsSyncStatus()).rejects.toMatchObject({ code: 'read', message: 'Could not refresh device sync status.' });
    }
  });

  it('treats events only as hints and returns the exact unlistener', async () => {
    const stop = vi.fn();
    vi.mocked(listen).mockResolvedValueOnce(stop);
    const changed = vi.fn();
    expect(await onBbsSyncChanged(changed)).toBe(stop);
    expect(listen).toHaveBeenCalledWith('bbs-sync://changed', expect.any(Function));
    const callback = vi.mocked(listen).mock.calls[0][1];
    callback({ event: 'bbs-sync://changed', id: 1, payload: { phase: 'success', secret: 'PRIVATE' } });
    expect(changed).toHaveBeenCalledExactlyOnceWith();
    expect(invoke).not.toHaveBeenCalled();
  });

  it('redacts failed event registration without retrying or reading state', async () => {
    vi.mocked(listen).mockRejectedValueOnce(new Error('PRIVATE'));
    await expect(onBbsSyncChanged(vi.fn())).rejects.toMatchObject({
      code: 'listen', message: 'Could not listen for device sync updates.',
    });
    expect(listen).toHaveBeenCalledTimes(1);
    expect(invoke).not.toHaveBeenCalled();
  });

  it('uses a module-stable source and never treats the public ready flag as a copyable invitation', async () => {
    vi.mocked(invoke).mockResolvedValue(status());
    expect(bbsSyncViewSource.listen).toBe(onBbsSyncChanged);
    const view = await bbsSyncViewSource.read();
    expect(view.invitation).toEqual({ state: 'preparing' });
    expect(view.workerAvailable).toBe(true);
    expect(view.group?.members).toHaveLength(2);
    expect(view.progress).toBeNull();
    expect(view.lastSuccessfulAt).toBeNull();
  });

  it('connects the visible hook listen-first and closes without canceling or disconnecting', async () => {
    const order: string[] = [];
    const stop = vi.fn();
    vi.mocked(listen).mockImplementation(async () => { order.push('listen'); return stop; });
    vi.mocked(invoke).mockImplementation(async () => { order.push('read'); return status() as never; });
    const { result, rerender, unmount } = renderHook(({ open }) => useBbsSyncView(open, bbsSyncViewSource), { initialProps: { open: false } });
    expect(order).toEqual([]);
    await act(async () => rerender({ open: true }));
    expect(order).toEqual(['listen', 'read']);
    expect(result.current.view?.group?.id).toBe('group-one');
    await act(async () => rerender({ open: false }));
    expect(stop).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledExactlyOnceWith('bbs_sync_status');
    unmount();
  });
});

describe('BBS sync management client', () => {
  it.each([
    ['stale_signature', 'Request expired; check this device’s clock and the other devices’ clocks, then retry.'],
    ['worker_update_required', 'Worker is outdated; update it from the Laughing Man card and retry.'],
    ['sync_busy', 'Another Kota app is using sync; quit it and click Retry.'],
  ])('maps only the agreed %s enum without raw strings, causes or retries', async (code, message) => {
    const tasks = [
      () => bbsSyncInvitation({ expectedGroupId: null, refresh: false }),
      () => bbsSyncJoin({ expectedGroupId: null, invitation: 'PRIVATE' }),
      () => bbsSyncDisconnect({ expectedGroupId: 'group-one' }),
      () => bbsSyncRename({ expectedGroupId: 'group-one', name: 'Mac' }),
      () => bbsSyncRemove({ expectedGroupId: 'group-one', deviceId: 'peer' }),
      () => bbsSyncStart({ expectedGroupId: 'group-one' }),
      () => bbsSyncCancel({ expectedGroupId: 'group-one' }),
    ];
    vi.mocked(invoke).mockRejectedValue({ code });
    for (const task of tasks) await expect(task()).rejects.toMatchObject({ code, message });
    expect(invoke).toHaveBeenCalledTimes(tasks.length);
    for (const failure of [code, JSON.stringify({ code }), [code],
      { code, token: 'PRIVATE' }, { code: 'new_error' }, new Error(`${code} PRIVATE`)]) {
      vi.mocked(invoke).mockRejectedValueOnce(failure);
      const error = await bbsSyncJoin({ expectedGroupId: null, invitation: 'PRIVATE' }).catch((caught: unknown) => caught);
      expect(error).toMatchObject({ code: 'join', message: 'Could not join this group. Check the invitation and try again.' });
      expect(error).not.toHaveProperty('cause');
      expect(String(error)).not.toContain('PRIVATE');
    }
  });

  it('passes a fixed group fence to every command and only returns unit', async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    const expectedGroupId = 'group-one';
    await expect(bbsSyncJoin({ expectedGroupId, invitation: 'opaque invitation' })).resolves.toBeUndefined();
    await expect(bbsSyncDisconnect({ expectedGroupId })).resolves.toBeUndefined();
    await expect(bbsSyncRename({ expectedGroupId, name: 'Study Mac' })).resolves.toBeUndefined();
    await expect(bbsSyncRemove({ expectedGroupId, deviceId: 'peer' })).resolves.toBeUndefined();
    await expect(bbsSyncStart({ expectedGroupId })).resolves.toBeUndefined();
    await expect(bbsSyncCancel({ expectedGroupId })).resolves.toBeUndefined();
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ['bbs_sync_join', { request: { expectedGroupId, invitation: 'opaque invitation' } }],
      ['bbs_sync_disconnect', { request: { expectedGroupId } }],
      ['bbs_sync_rename', { request: { expectedGroupId, name: 'Study Mac' } }],
      ['bbs_sync_remove', { request: { expectedGroupId, deviceId: 'peer' } }],
      ['bbs_sync_start', { request: { expectedGroupId } }],
      ['bbs_sync_cancel', { request: { expectedGroupId } }],
    ]);
    expect(listen).not.toHaveBeenCalled();
  });

  it('does not send excess caller fields to IPC', async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    const request = { expectedGroupId: null, name: 'Study Mac', secret: 'PRIVATE' };
    await bbsSyncRename(request);
    expect(invoke).toHaveBeenCalledExactlyOnceWith('bbs_sync_rename', { request: { expectedGroupId: null, name: 'Study Mac' } });
  });

  it('fetches invitation material only on demand and projects its response', async () => {
    const result = { groupId: 'new-group', generation: '7', invitation: 'kota-bbs://example.workers.dev/join#private-token' };
    vi.mocked(invoke).mockResolvedValue({ ...result, privateKey: 'SECRET' });
    await expect(bbsSyncInvitation({ expectedGroupId: null, refresh: false })).resolves.toEqual(result);
    expect(invoke).toHaveBeenCalledExactlyOnceWith('bbs_sync_invitation', { request: { expectedGroupId: null, refresh: false } });
    expect(listen).not.toHaveBeenCalled();
    vi.mocked(invoke).mockResolvedValue(result);
    await bbsSyncInvitation({ expectedGroupId: 'new-group', refresh: true });
    expect(invoke).toHaveBeenLastCalledWith('bbs_sync_invitation', { request: { expectedGroupId: 'new-group', refresh: true } });
  });

  it('rejects malformed or wrong-group invitations without retaining raw response in errors', async () => {
    for (const result of [null, { groupId: 'group-one' }, { groupId: 'group-one', invitation: '' },
      { groupId: 'group-one', invitation: 'x'.repeat(4097) }, { groupId: 'other-group', invitation: 'PRIVATE' }]) {
      vi.mocked(invoke).mockResolvedValueOnce(result);
      await expect(bbsSyncInvitation({ expectedGroupId: 'group-one', refresh: false })).rejects.toMatchObject({ code: 'protocol' });
    }
  });

  it('uses exact decimal generation identities without Number or lexical ordering', async () => {
    const large = '9007199254740993';
    const parsed = parseBbsSyncStatus({ ...status(), invitationGeneration: large });
    expect(bbsSyncStatusView(parsed).invitationGeneration).toBe(large);
    for (const invitationGeneration of [undefined, 7, '07', '-1', '1e3', '']) {
      expect(() => parseBbsSyncStatus({ ...status(), invitationGeneration })).toThrow(/incompatible/);
    }
    expect(() => parseBbsSyncStatus({ ...status(), invitationGeneration: null })).toThrow(/incompatible/);
    expect(() => parseBbsSyncStatus({ ...status(), group: { ...status().group, role: 'member' } })).toThrow(/incompatible/);
    for (const generation of [undefined, 7, '07']) {
      vi.mocked(invoke).mockResolvedValueOnce({ groupId: 'group-one', generation, invitation: 'PRIVATE' });
      await expect(bbsSyncInvitation({ expectedGroupId: 'group-one', refresh: false })).rejects.toMatchObject({ code: 'protocol' });
    }
  });

  it('does not retry an ambiguous mutation or display raw rejection material', async () => {
    vi.mocked(invoke).mockRejectedValueOnce({ error: 'request failed', invitation: 'PRIVATE' });
    await expect(bbsSyncJoin({ expectedGroupId: null, invitation: 'PRIVATE' })).rejects.toMatchObject({
      code: 'join', message: 'Could not join this group. Check the invitation and try again.',
    });
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(listen).not.toHaveBeenCalled();
  });

  it('does not mistake resolved structured failures or statuses for command success', async () => {
    for (const result of [{ ok: false }, status(), false]) {
      vi.mocked(invoke).mockResolvedValueOnce(result);
      await expect(bbsSyncStart({ expectedGroupId: 'group-one' })).rejects.toMatchObject({ code: 'protocol' });
    }
  });

  it('refuses mutations outside Kota without a fake success fallback', async () => {
    vi.mocked(isTauri).mockReturnValue(false);
    await expect(bbsSyncJoin({ expectedGroupId: null, invitation: 'PRIVATE' })).rejects.toMatchObject({ code: 'unavailable' });
    await expect(bbsSyncInvitation({ expectedGroupId: null, refresh: false })).rejects.toMatchObject({ code: 'unavailable' });
    expect(invoke).not.toHaveBeenCalled();
  });
});
