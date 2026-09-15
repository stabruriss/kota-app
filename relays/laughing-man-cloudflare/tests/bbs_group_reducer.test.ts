import { describe, expect, it } from 'vitest';
import {
  createGroup,
  INVITE_TTL_MS,
  MAX_RECEIPTS,
  RECEIPT_TTL_MS,
  reduce,
  type Actor,
  type GroupReducerState,
  type GroupRequest,
} from '../src/bbs_group_reducer';

const owner: Actor = { kind: 'owner' };
const alice: Actor = { kind: 'joiner', deviceId: 'alice', publicKey: 'alice-key' };
const bob: Actor = { kind: 'joiner', deviceId: 'bob', publicKey: 'bob-key' };
const join: GroupRequest = { kind: 'redeem', hash: 'hash', requestId: 'join-1', name: 'Alice' };
function fixture() {
  let state = createGroup(
    'group-1',
    { deviceId: 'owner', publicKey: 'owner-key', name: 'Owner', membershipId: 'create-1' },
    1,
  );
  return {
    get state() {
      return state;
    },
    run(actor: Actor, request: GroupRequest, now = 10) {
      const result = reduce(state, actor, request, now);
      state = result.state;
      return result.response;
    },
  };
}
function invited() {
  const f = fixture();
  f.run(owner, { kind: 'invite', gen: 1, hash: 'hash', requestId: 'invite-1' });
  return f;
}
describe('BBS control state', () => {
  it('consumes exactly once, while retrying the winning request recovers its membership', () => {
    const f = invited();
    const before = structuredClone(f.state);
    const first = f.run(alice, join);
    expect(first.ok).toBe(true);
    expect(f.run(bob, { ...join, requestId: 'join-2' })).toMatchObject({
      ok: false,
      error: 'invitation_used',
    });
    expect(f.run(alice, join)).toEqual(first);
    expect(Object.keys(f.state.members)).toHaveLength(2);
    expect(before.invite.hash).toBe('hash');
  });
  it('rejects replay after removal, including after a fresh authorized rejoin', () => {
    const f = invited();
    f.run(alice, join);
    f.run(owner, { kind: 'remove', deviceId: 'alice', requestId: 'remove-1' });
    expect(f.run(alice, join).error).toBe('removed');
    f.run(owner, { kind: 'invite', gen: 2, hash: 'new', requestId: 'invite-2' });
    expect(f.run(alice, { ...join, hash: 'new', requestId: 'join-new' }).ok).toBe(true);
    expect(f.run(alice, join).error).toBe('removed');
  });
  it('rejects another public key replaying a successful requestId', () => {
    const f = invited();
    f.run(alice, join);
    expect(f.run(bob, join).error).toBe('request_conflict');
    expect(f.run({ ...alice, publicKey: 'forged' }, join).error).toBe('request_conflict');
  });
  it('dissolve invalidates invitation and all prior receipts', () => {
    const f = invited();
    f.run(alice, join);
    f.run(owner, { kind: 'dissolve', requestId: 'dissolve-1' });
    expect(f.run(alice, join).error).toBe('removed');
    expect(f.run(owner, { kind: 'invite', gen: 2, hash: 'new', requestId: 'invite-2' }).error).toBe(
      'removed',
    );
    expect(f.state.invite.hash).toBeNull();
    expect(f.state.members).toEqual({});
  });
  it('registers generations idempotently, rejects changed payload and stale generation', () => {
    const f = invited();
    expect(f.run(owner, { kind: 'invite', gen: 1, hash: 'hash', requestId: 'invite-1' }).ok).toBe(
      true,
    );
    expect(
      f.run(owner, { kind: 'invite', gen: 1, hash: 'other', requestId: 'invite-1' }).error,
    ).toBe('request_conflict');
    expect(
      f.run(owner, { kind: 'invite', gen: 1, hash: 'other', requestId: 'invite-2' }).error,
    ).toBe('generation_conflict');
    f.run(owner, { kind: 'invite', gen: 2, hash: 'next', requestId: 'invite-2' });
    expect(f.run(alice, join).ok).toBe(false);
  });
  it('uses server time, expires codes, and keeps twenty old hash outcomes', () => {
    const f = invited();
    expect(f.state.invite.expiresAt).toBe(10 + INVITE_TTL_MS);
    expect(f.run(alice, join, 10 + INVITE_TTL_MS).error).toBe('invitation_expired');
    for (let gen = 2; gen < 24; gen++)
      f.run(owner, { kind: 'invite', gen, hash: `hash-${gen}`, requestId: `invite-${gen}` });
    expect(f.state.oldHashes).toHaveLength(20);
  });
  it('members cannot refresh, remove other members, or dissolve; leave revokes replay', () => {
    const f = invited();
    f.run(alice, join);
    const member: Actor = { ...alice, kind: 'member' };
    for (const request of [
      { kind: 'invite', gen: 2, hash: 'next', requestId: 'x' },
      { kind: 'remove', deviceId: 'owner', requestId: 'y' },
      { kind: 'dissolve', requestId: 'z' },
    ] as GroupRequest[])
      expect(f.run(member, request).error).toBe('owner_required');
    expect(f.run(member, { kind: 'leave', requestId: 'leave-1' }).ok).toBe(true);
    expect(f.run(alice, join).error).toBe('removed');
    expect(f.run(member, { kind: 'status' }).error).toBe('unauthorized');
  });
  it('bounds receipt count and lifetime; status is read-only and includes member public keys', () => {
    const f = invited();
    for (let gen = 2; gen < MAX_RECEIPTS + 8; gen++)
      f.run(owner, { kind: 'invite', gen, hash: `h${gen}`, requestId: `i${gen}` }, gen);
    expect(Object.keys(f.state.receipts)).toHaveLength(MAX_RECEIPTS);
    const before = structuredClone(f.state);
    expect(f.run(owner, { kind: 'status' }).members?.[0].publicKey).toBe('owner-key');
    expect(f.state).toEqual(before);
    f.run(owner, { kind: 'heartbeat' }, RECEIPT_TTL_MS + 1000);
    expect(f.state.receipts).toEqual({});
  });
});

it('retains active join recovery and revoked join evidence beyond the general receipt budget', () => {
  const f = invited();
  const result = f.run(alice, join);
  for (let gen = 2; gen < MAX_RECEIPTS + 8; gen++)
    f.run(owner, { kind: 'invite', gen, hash: `h-${gen}`, requestId: `i-${gen}` }, gen + 100);
  expect(f.state.receipts['join-1']).toBeUndefined();
  expect(f.run(alice, join, RECEIPT_TTL_MS + 1000)).toEqual(result);
  expect(f.run(bob, join).error).toBe('request_conflict');
  f.run(owner, { kind: 'remove', deviceId: 'alice', requestId: 'remove-1' });
  expect(f.run(alice, join, RECEIPT_TTL_MS * 2).error).toBe('removed');
});
