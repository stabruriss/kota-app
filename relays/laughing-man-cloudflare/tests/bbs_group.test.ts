import { beforeEach, describe, expect, it, vi } from 'vitest';
import worker, { BbsGroup, RelayState } from '../src/index';
import { MemoryState } from './durable_runtime';
import { signedRequest, signingKey } from './support';
import { sha256 } from '../src/bbs_auth';
import { RateLimiter } from '../src/bbs_auth';
import type { Env } from '../src/index';

const GROUP = '00000000-0000-4000-8000-000000000001';
function environment() {
  const states = new Map<string, MemoryState>();
  const objects = new Map<string, BbsGroup>();
  const relayState = new MemoryState();
  relayState.data.set('config:desktopSecret', 'paired-secret');
  const env: any = {};
  const relay = new RelayState(relayState as any, env);
  env.RELAY_STATE = { idFromName: (name: string) => name, get: () => relay };
  env.BBS_GROUP = {
    idFromName: (name: string) => name,
    get: (name: string) => {
      if (!objects.has(name)) {
        const ctx = new MemoryState();
        states.set(name, ctx);
        objects.set(name, new BbsGroup(ctx as any, env));
      }
      return objects.get(name)!;
    },
  };
  return { env: env as Env, states, objects, relayState };
}
async function setup() {
  const context = environment();
  const key = await signingKey();
  const owner = async (path: string, fields: object, secret = 'paired-secret') =>
    worker.fetch(
      new Request(`https://worker.example/bbs/owner/${path}`, {
        method: path === 'invitation' ? 'PUT' : 'POST',
        headers: { 'x-kota-standby-secret': secret, 'cf-connecting-ip': key.deviceId },
        body: JSON.stringify({ protocolVersion: 1, expectedGroupId: GROUP, ...fields }),
      }),
      context.env,
    );
  const create = {
    expectedGroupId: null,
    groupId: GROUP,
    requestId: 'create-request-00001',
    deviceId: key.deviceId,
    publicKey: key.publicKey,
    name: 'Owner',
  };
  const created = await owner('create', create);
  expect(created.status).toBe(200);
  const token = 'a'.repeat(64);
  const hash = await sha256(token);
  expect(
    (await owner('invitation', { hash, gen: 1, requestId: 'invite-request-00001' })).status,
  ).toBe(200);
  const join = async (
    candidate: Awaited<ReturnType<typeof signingKey>>,
    requestId: string,
    code = token,
  ) =>
    worker.fetch(
      await signedRequest(
        candidate,
        '/bbs/join',
        { protocolVersion: 1, token: code, requestId, name: 'Member' },
        '',
      ),
      context.env,
    );
  const member = async (
    candidate: Awaited<ReturnType<typeof signingKey>>,
    action: string,
    fields: object = {},
  ) =>
    worker.fetch(
      await signedRequest(
        candidate,
        `/bbs/groups/${GROUP}/${action}`,
        { protocolVersion: 1, expectedGroupId: GROUP, ...fields },
        GROUP,
      ),
      context.env,
    );
  return { ...context, key, owner, create, join, member, token, hash };
}
describe('BbsGroup DO request and storage integration', () => {
  it('keeps malformed relay requests from resetting the group object', async () => {
    const f = await setup();
    const bad = await worker.fetch(
      new Request(`https://worker.example/bbs/relay/not-a-route?group=${GROUP}`, { headers: { 'cf-connecting-ip': 'bad' } }),
      f.env,
    );
    expect(bad.status).toBe(404);
    expect((await f.owner('status')).status).toBe(200);
  });

  it('uses separate high abuse and authenticated-device rate buckets', () => {
    const limiter = new RateLimiter();
    for (let i = 0; i < 3000; i++) expect(limiter.allow('shared-ip', '/bbs/relay', 0, 3000)).toBe(true);
    expect(limiter.allow('shared-ip', '/bbs/relay', 0, 3000)).toBe(false);
    for (let i = 0; i < 1500; i++) expect(limiter.allow('device-a', 'receive', 0, 1500)).toBe(true);
    expect(limiter.allow('device-a', 'receive', 0, 1500)).toBe(false);
    expect(limiter.allow('device-b', 'receive', 0, 1500)).toBe(true);
  });

  it('cleans expired relay metadata without dropping live rows', async () => {
    const f = environment();
    const ctx = new MemoryState();
    const group = new BbsGroup(ctx as any, f.env);
    const now = Date.now();
    await ctx.storage.put('relay:old-request', { expiresAt: now - 1, response: {} });
    await ctx.storage.put('relay:old-nonce', now - 1);
    await ctx.storage.put('relay:wake:old', { expiresAt: now - 1 });
    await ctx.storage.put('relay:live-request', { expiresAt: now + 60_000, response: {} });
    await group.alarm();
    expect(ctx.data.has('relay:old-request')).toBe(false);
    expect(ctx.data.has('relay:old-nonce')).toBe(false);
    expect(ctx.data.has('relay:wake:old')).toBe(false);
    expect(ctx.data.has('relay:live-request')).toBe(true);
    expect(ctx.alarm).toBeGreaterThanOrEqual(now + 59_000);
  });

  it('does not reschedule an alarm when no relay or signal rows remain', async () => {
    const f = environment();
    const ctx = new MemoryState();
    const group = new BbsGroup(ctx as any, f.env);
    await group.alarm();
    expect(ctx.alarm).toBeNull();
  });
  it('advertises the BBS-capable release in LM health and heartbeat without changing protocols', async () => {
    const f = environment();
    const health = await worker.fetch(new Request('https://worker.example/health'), f.env);
    expect(health.status).toBe(200);
    expect(await health.json()).toMatchObject({ relayVersion: '0.1.4', protocolVersion: 'kota-lm-standby.v1' });
    const heartbeat = await worker.fetch(new Request('https://worker.example/desktop/heartbeat', {
      method: 'POST',
      headers: { 'x-kota-standby-secret': 'paired-secret' },
      body: '{}',
    }), f.env);
    expect(heartbeat.status).toBe(200);
    expect(await heartbeat.json()).toMatchObject({ relayVersion: '0.1.4', protocolVersion: 'kota-lm-standby.v1' });
    const bbs = await worker.fetch(new Request('https://worker.example/bbs/owner/status'), f.env);
    expect(bbs.status).toBe(404);
    expect(await bbs.json()).toMatchObject({ protocolVersion: 1, error: 'not_found' });
    expect(f.relayState.data.get('config:desktopSecret')).toBe('paired-secret');
  });
  it('merges heartbeat membership, bounded signals and owner generation without code/hash', async () => {
    const f = await setup();
    const a = await signingKey();
    await f.join(a, 'join-request-000001');
    await f.owner('invitation', { hash: await sha256('b'.repeat(64)), gen: 2, requestId: 'invite-request-00002' });
    for (let i = 0; i < 10; i++) {
      expect((await f.member(a, 'signal', { to: f.key.deviceId, payload: 'signed-wake', requestId: `signal-heartbeat-${i}` })).status).toBe(200);
    }
    const owner = await (await f.member(f.key, 'heartbeat')).json() as any;
    expect(owner.members).toHaveLength(2);
    expect(owner.memberVersion).toBeGreaterThan(0);
    expect(owner.signals).toHaveLength(8);
    expect(owner.invite).toMatchObject({gen: 2, hasCode: true});
    expect(owner.invite).not.toHaveProperty('hash');
    expect(JSON.stringify(owner)).not.toContain(f.hash);
    const rest = await (await f.member(f.key, 'heartbeat')).json() as any;
    expect(rest.signals).toHaveLength(2);
    const empty = await (await f.member(f.key, 'heartbeat')).json() as any;
    expect(empty.signals).toEqual([]);
    const member = await (await f.member(a, 'heartbeat')).json() as any;
    expect(member.invite).toBeUndefined();
    expect(member.signals).toEqual([]);
    const ownerStatus = await (await f.owner('status', {})).json() as any;
    expect(ownerStatus.invite).toMatchObject({gen: 2, hasCode: true});
    expect(ownerStatus.invite).not.toHaveProperty('hash');
  });
  it('rejects unpaired/forged owner, unauthenticated status and legacy register/pull paths', async () => {
    const f = await setup();
    expect((await f.owner('status', {}, 'wrong')).status).toBe(401);
    f.relayState.data.delete('config:desktopSecret');
    expect((await f.owner('status', {})).status).toBe(401);
    for (const action of ['status', 'member/register', 'signal/pull']) {
      const result = await worker.fetch(
        new Request(`https://worker.example/bbs/groups/${GROUP}/${action}`, {
          method: 'POST',
          body: JSON.stringify({ protocolVersion: 1 }),
        }),
        f.env,
      );
      expect(result.status).not.toBe(200);
    }
  });
  it('serializes concurrent redemption and recovers a lost response using the same requestId', async () => {
    const f = await setup();
    const a = await signingKey();
    const b = await signingKey();
    const responses = await Promise.all([
      f.join(a, 'join-request-000001'),
      f.join(b, 'join-request-000002'),
    ]);
    expect(responses.filter((response) => response.status === 200)).toHaveLength(1);
    const winner = responses[0].status === 200 ? a : b;
    const id = winner === a ? 'join-request-000001' : 'join-request-000002';
    const recovered = await f.join(winner, id);
    expect(await recovered.json()).toMatchObject({ ok: true, membershipId: id });
    const status = (await (await f.member(winner, 'status')).json()) as any;
    expect(status.members).toHaveLength(2);
    expect(status.members.some((entry: any) => entry.publicKey === winner.publicKey)).toBe(true);
    expect(status).not.toHaveProperty('invite');
    expect(JSON.stringify([...f.states.get(GROUP)!.data.values()])).not.toContain(f.token);
    expect(JSON.stringify([...f.states.get('owner')!.data.values()])).not.toContain(f.token);
  });
  it('uses nonces once, rejects tampered signatures and denies a removed member plus old join replay', async () => {
    const f = await setup();
    const a = await signingKey();
    await f.join(a, 'join-request-000001');
    const request = await signedRequest(
      a,
      `/bbs/groups/${GROUP}/status`,
      { protocolVersion: 1, expectedGroupId: GROUP },
      GROUP,
    );
    const copy = request.clone();
    expect((await worker.fetch(request, f.env)).status).toBe(200);
    expect((await worker.fetch(copy, f.env)).status).toBe(409);
    const bad = await signedRequest(
      a,
      `/bbs/groups/${GROUP}/status`,
      { protocolVersion: 1, expectedGroupId: GROUP },
      GROUP,
    );
    bad.headers.set('x-kota-bbs-signature', btoa('\0'.repeat(64)));
    expect((await worker.fetch(bad, f.env)).status).toBe(401);
    expect(
      (await f.owner('remove', { deviceId: a.deviceId, requestId: 'remove-request-00001' })).status,
    ).toBe(200);
    expect((await f.join(a, 'join-request-000001')).status).toBe(409);
    const nonceCount = [...f.states.get(GROUP)!.data.keys()].filter((key) =>
      key.startsWith('nonce:'),
    ).length;
    expect((await f.member(a, 'status')).status).toBe(403);
    expect(
      [...f.states.get(GROUP)!.data.keys()].filter((key) => key.startsWith('nonce:')),
    ).toHaveLength(nonceCount);
    const status = (await (await f.owner('status', {})).json()) as any;
    expect(status.members).toHaveLength(1);
    expect(f.states.get(GROUP)!.data.has('revoked:join-request-000001')).toBe(true);
    expect((f.states.get(GROUP)!.data.get('state') as any).revokedJoins).toEqual({});
  });
  it('keeps signals in bounded independent expiring keys, checks recipient and deletes after pull', async () => {
    const f = await setup();
    const a = await signingKey();
    await f.join(a, 'join-request-000001');
    for (let n = 0; n < 32; n++)
      expect(
        (
          await f.member(a, 'signal', {
            to: f.key.deviceId,
            payload: 'opaque-signed-sdp',
            requestId: `signal-request-${String(n).padStart(5, '0')}`,
          })
        ).status,
      ).toBe(200);
    expect(
      (
        await f.member(a, 'signal', {
          to: f.key.deviceId,
          payload: 'overflow',
          requestId: 'signal-request-overflow',
        })
      ).status,
    ).toBe(429);
    expect((await f.member(a, 'signal/pull')).status).toBe(200);
    const received = (await (await f.member(f.key, 'signal/pull')).json()) as any;
    expect(received.signals).toHaveLength(8);
    const stored = f.states.get(GROUP)!;
    expect((stored.data.get('state') as any).signals).toBeUndefined();
    const realNow = Date.now();
    vi.spyOn(Date, 'now').mockReturnValue(realNow + 121_000);
    await f.objects.get(GROUP)!.alarm();
    vi.restoreAllMocks();
    expect([...stored.data.keys()].filter((key) => key.startsWith('signal:'))).toHaveLength(0);
  });
  it('dissolves durably and cannot revive the group through old creation or redemption', async () => {
    const f = await setup();
    const a = await signingKey();
    await f.join(a, 'join-request-000001');
    expect((await f.owner('dissolve', { requestId: 'dissolve-request-001' })).status).toBe(200);
    expect((await f.owner('dissolve', { requestId: 'dissolve-request-001' })).status).toBe(200);
    expect((await f.owner('create', f.create)).status).toBe(409);
    expect((await f.join(a, 'join-request-000001')).status).toBe(409);
    expect((f.states.get('owner')!.data.get(`group:${GROUP}`) as any).phase).toBe('revoked');
    expect([...f.states.get(GROUP)!.data.keys()]).toEqual(['state']);
  });
  it('rejects stale expected group and preserves the existing RelayState routes and storage', async () => {
    const f = await setup();
    const before = structuredClone(f.relayState.data);
    expect(
      (
        await f.owner('remove', {
          expectedGroupId: 'another-group-0001',
          deviceId: f.key.deviceId,
          requestId: 'request-remove-001',
        })
      ).status,
    ).toBe(409);
    const response = await worker.fetch(new Request('https://worker.example/health'), f.env);
    expect(response.status).toBe(200);
    expect(f.relayState.data).toEqual(before);
    expect([...f.relayState.data.keys()].some((key) => key.includes('bbs'))).toBe(false);
    const denied = await worker.fetch(
      new Request('https://worker.example/desktop/state', { method: 'POST', body: '{}' }),
      f.env,
    );
    expect(denied.status).toBe(401);
  });
});
