import { describe, expect, it, vi } from 'vitest';
import { applyMutation, pollMetadata, prepareMutation, pairKey } from '../src/bbs_relay_metadata';
import { GROUP, INSTANCE, keys, NOW, members, verified, MetaStore } from './relay_support';
import type { Wake } from '../src/bbs_relay_session';

async function announced(store = new MetaStore()) {
  for (let i = 0; i < keys.length; i++) {
    const input = await verified(keys[i], 'announce', {
      requestId: `announcement-${i}-0001`,
      previous: null,
      createdAt: NOW,
      instance: INSTANCE[i],
      revision: 'f'.repeat(64),
      peerVersion: 4,
    });
    await applyMutation(await prepareMutation(input), members(), store, NOW);
  }
  return store;
}
async function intent(id: string, previous: string | null, now = NOW) {
  const input = await verified(
    keys[0],
    'wake',
    {
      requestId: id,
      previous,
      createdAt: now,
      instance: INSTANCE[0],
      peer: keys[1].deviceId,
      peerInstance: INSTANCE[1],
    },
    { nonce: id, now },
  );
  return prepareMutation(input);
}
describe('durable relay metadata gate', () => {
  it('replayed GET is read-only, non-destructive, no nonce scans/reads and no TTL renewal', async () => {
    const timer = vi.spyOn(globalThis, 'setTimeout');
    const interval = vi.spyOn(globalThis, 'setInterval');
    // Includes announce and wake before the read-only replay check.
    const store = await announced(),
      state = members();
    const created = await applyMutation(
      await intent('wake-request-0000001', null),
      state,
      store,
      NOW,
    );
    const before = structuredClone([...store.rows]),
      alarm = store.alarm;
    store.resetCounts();
    const input = await verified(keys[1], 'poll', null);
    const first = await pollMetadata(input, state, store, NOW);
    const replay = await pollMetadata(input, state, store, NOW + 1000);
    expect(replay).toEqual(first);
    expect(store.writes).toEqual([]);
    expect(store.reads.some((k) => /nonce|request|alarm/.test(k))).toBe(false);
    expect([...store.rows]).toEqual(before);
    expect(store.alarm).toBe(alarm);
    expect(first.items.some((r) => r.currentWake === created.current)).toBe(true);
    expect(timer).not.toHaveBeenCalled();
    expect(interval).not.toHaveBeenCalled();
    timer.mockRestore();
    interval.mockRestore();
  });
  it('announce retries read the fingerprint first and perform zero additional writes', async () => {
    const store = new MetaStore(),
      state = members();
    const body = {
      requestId: 'announce-request-0001',
      previous: null,
      createdAt: NOW,
      instance: INSTANCE[0],
      revision: 'f'.repeat(64),
      peerVersion: 4,
    };
    const prepared = await prepareMutation(await verified(keys[0], 'announce', body));
    const first = await applyMutation(prepared, state, store, NOW);
    expect(first).toEqual({ ok: true, current: body.requestId, changed: true });
    expect(store.writes.length).toBe(4);
    store.resetCounts();
    const retry = await prepareMutation(
      await verified(keys[0], 'announce', body, { now: NOW + 1000, nonce: 'fresh-nonce-0000001' }),
    );
    expect(await applyMutation(retry, state, store, NOW + 1000)).toEqual({
      ok: true,
      current: body.requestId,
      changed: false,
      replacedWake: undefined,
    });
    expect(store.reads).toHaveLength(1);
    expect(store.writes).toEqual([]);
    const conflict = await prepareMutation(
      await verified(keys[0], 'announce', { ...body, revision: 'e'.repeat(64) }),
    );
    await expect(applyMutation(conflict, state, store, NOW)).rejects.toThrow('request_conflict');
  });
  it('pending announce survives a day-long outage or lost response with the original request ID', async () => {
    const state = members(),
      store = new MetaStore();
    const body = {
      requestId: 'durable-announce-0001',
      previous: null,
      createdAt: NOW,
      instance: INSTANCE[0],
      revision: 'f'.repeat(64),
      peerVersion: 4,
    };
    const tomorrow = NOW + 86_400_000;
    const delayed = await verified(keys[0], 'announce', body, { now: tomorrow });
    // No first request reached the server during the outage.
    expect(
      await applyMutation(await prepareMutation(delayed), state, store, tomorrow),
    ).toMatchObject({ ok: true, current: body.requestId, changed: true });
    // The successful reply was lost. Cleanup removes only temporary receipts.
    for (const name of store.rows.keys())
      if (name.includes(':request:') || name.includes(':nonce:')) store.rows.delete(name);
    store.resetCounts();
    const retry = await verified(keys[0], 'announce', body, { now: tomorrow + 86_400_000 });
    expect(
      await applyMutation(await prepareMutation(retry), state, store, tomorrow + 86_400_000),
    ).toEqual({ ok: true, current: body.requestId, changed: false });
    expect(store.writes).toEqual([]);
    const changed = await verified(
      keys[0],
      'announce',
      { ...body, revision: 'e'.repeat(64) },
      { now: tomorrow + 86_400_000 },
    );
    await expect(
      applyMutation(await prepareMutation(changed), state, store, tomorrow + 86_400_000),
    ).rejects.toThrow('request_conflict');
  });
  it('new wake immediately replaces current; old requests only return original receipt, never restore it', async () => {
    const store = await announced(),
      state = members();
    const old = await intent('wake-request-0000001', null);
    const first = await applyMutation(old, state, store, NOW);
    const newer = await intent('wake-request-0000002', first.current, NOW + 1);
    const second = await applyMutation(newer, state, store, NOW + 1);
    expect(second).toMatchObject({ ok: true, changed: true, replacedWake: first.current });
    store.resetCounts();
    expect(await applyMutation(old, state, store, NOW + 2)).toMatchObject({
      ok: true,
      current: first.current,
      changed: false,
    });
    expect(store.writes).toEqual([]);
    expect((await store.get<Wake>(pairKey(keys[0].deviceId, keys[1].deviceId)))?.id).toBe(
      second.current,
    );
    const stale = await intent('wake-request-0000003', first.current, NOW + 3);
    expect(await applyMutation(stale, state, store, NOW + 3)).toEqual({
      ok: false,
      current: second.current,
      changed: false,
    });
    expect((await store.get<Wake>(pairKey(keys[0].deviceId, keys[1].deviceId)))?.id).toBe(
      second.current,
    );
  });
  it('wake TTL uses server time despite allowed clock skew, and receipt expiry cannot revive an old intent', async () => {
    const store = await announced(),
      state = members();
    const body = {
      requestId: 'clock-skew-wake-0001',
      previous: null,
      createdAt: NOW + 60_000,
      instance: INSTANCE[0],
      peer: keys[1].deviceId,
      peerInstance: INSTANCE[1],
    };
    const input = await verified(keys[0], 'wake', body, {
      now: NOW + 60_000,
      nonce: 'clock-skew-wake-nonce-0001',
    });
    const result = await applyMutation(await prepareMutation(input), state, store, NOW);
    const name = pairKey(keys[0].deviceId, keys[1].deviceId);
    const stored = await store.get<Wake>(name);
    expect(stored?.expiresAt).toBe(NOW + 120_000);
    const retry = await verified(keys[0], 'wake', body, { now: NOW + 70_000 });
    expect(
      await applyMutation(await prepareMutation(retry), state, store, NOW + 70_000),
    ).toMatchObject({ ok: true, current: result.current, changed: false });
    expect(await store.get<Wake>(name)).toEqual(stored);
    for (const key of store.rows.keys())
      if (key.includes(':request:') || key.includes(':nonce:')) store.rows.delete(key);
    const expired = await verified(keys[0], 'wake', body, { now: NOW + 360_001 });
    await expect(
      applyMutation(await prepareMutation(expired), state, store, NOW + 360_001),
    ).rejects.toThrow('relay_wake_expired');
    expect(await store.get<Wake>(name)).toEqual(stored);
  });
  it('serialized concurrent intents cannot overwrite a newer current-id; cold boot retains only metadata', async () => {
    const store = await announced(),
      state = members();
    const prepared = await Promise.all([
      intent('wake-race-request-0001', null),
      intent('wake-race-request-0002', null),
    ]);
    // Same serial transaction ordering supplied by the existing DO adapter.
    let tail = Promise.resolve();
    const run = (p: (typeof prepared)[number]) => {
      const result = tail.then(() => applyMutation(p, state, store, NOW));
      tail = result.then(() => {});
      return result;
    };
    const replies = await Promise.all(prepared.map(run));
    expect(replies.filter((r) => r.ok)).toHaveLength(1);
    expect(replies[1].current).toBe(replies[0].current);
    const cold = new MetaStore();
    cold.rows = structuredClone(store.rows);
    expect(await pollMetadata(await verified(keys[1], 'poll', null), state, cold, NOW)).toEqual(
      await pollMetadata(await verified(keys[1], 'poll', null), state, store, NOW),
    );
    expect([...cold.rows.keys()].some((k) => /ready|session|cipher|ack/.test(k))).toBe(false);
  });
  it('revocation and stale time are rechecked before replay receipts, and reused nonce cannot create new intent', async () => {
    const store = await announced(),
      state = members();
    const old = await intent('wake-request-0000001', null);
    await applyMutation(old, state, store, NOW);
    const removed = members();
    delete removed.members[keys[0].deviceId];
    await expect(applyMutation(old, removed, store, NOW)).rejects.toThrow('unauthorized');
    const rejoined = members();
    rejoined.members[keys[0].deviceId].membershipId = 'replacement-member-0001';
    await expect(applyMutation(old, rejoined, store, NOW)).rejects.toThrow('unauthorized');
    await expect(applyMutation(old, state, store, NOW + 300001)).rejects.toThrow('stale_signature');
    const newInput = await verified(
      keys[0],
      'wake',
      {
        requestId: 'wake-request-0000002',
        previous: null,
        createdAt: NOW,
        instance: INSTANCE[0],
        peer: keys[1].deviceId,
        peerInstance: INSTANCE[1],
      },
      { nonce: 'wake-request-0000001' },
    );
    await expect(applyMutation(await prepareMutation(newInput), state, store, NOW)).rejects.toThrow(
      'replayed_signature',
    );
  });
  it('late old revision CAS cannot replace newer announcement; expired wake reads do not mutate metadata', async () => {
    const store = await announced(),
      state = members();
    const p = await intent('wake-request-0000001', null);
    const result = await applyMutation(p, state, store, NOW);
    const old = await verified(
      keys[0],
      'announce',
      {
        requestId: 'announce-old-0000001',
        previous: null,
        createdAt: NOW,
        instance: INSTANCE[0],
        revision: 'e'.repeat(64),
        peerVersion: 4,
      },
      { nonce: 'announce-old-nonce-001' },
    );
    expect(await applyMutation(await prepareMutation(old), state, store, NOW)).toEqual({
      ok: false,
      current: 'announcement-0-0001',
      changed: false,
    });
    store.resetCounts();
    const read = await verified(keys[1], 'poll', null, { now: NOW + 120001 });
    const page = await pollMetadata(read, state, store, NOW + 120001);
    expect(page.items.find((r) => r.currentWake === result.current)?.wake).toBeNull();
    expect(store.writes).toEqual([]);
    expect([...store.rows.keys()].every((k) => k.startsWith('relay:'))).toBe(true);
    expect(state.groupId).toBe(GROUP);
  });
  it('a new membership sees no stale current wake and can create its first intent', async () => {
    const store = await announced(),
      oldState = members();
    const first = await applyMutation(
      await intent('old-member-wake-0001', null),
      oldState,
      store,
      NOW,
    );
    const state = members();
    const membership = 'rejoined-membership-0001';
    state.members[keys[0].deviceId].membershipId = membership;
    const announcement = await verified(
      keys[0],
      'announce',
      {
        requestId: 'rejoined-announce-0001',
        previous: null,
        createdAt: NOW,
        instance: INSTANCE[0],
        revision: 'f'.repeat(64),
        peerVersion: 4,
      },
      { membership },
      state,
    );
    await applyMutation(await prepareMutation(announcement), state, store, NOW);
    const read = await verified(keys[0], 'poll', null, { membership }, state);
    const page = await pollMetadata(read, state, store, NOW);
    expect(page.items.every((item) => item.currentWake === null)).toBe(true);
    const create = await verified(
      keys[0],
      'wake',
      {
        requestId: 'rejoined-wake-000001',
        previous: null,
        createdAt: NOW,
        instance: INSTANCE[0],
        peer: keys[1].deviceId,
        peerInstance: INSTANCE[1],
      },
      { membership, nonce: 'rejoined-wake-nonce-01' },
      state,
    );
    const result = await applyMutation(await prepareMutation(create), state, store, NOW);
    expect(result.ok).toBe(true);
    expect(result.current).not.toBe(first.current);
  });
});
