import { sign } from 'node:crypto';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { BbsGroup } from '../src/bbs_group';
import { createGroup, type GroupReducerState } from '../src/bbs_group_reducer';
import { RelayMemory } from '../src/bbs_relay_memory';
import { pairKey } from '../src/bbs_relay_metadata';
import { MAX_DECLARATION_BYTES, tlsSigningMessage, type SignedTlsStatement } from '../src/bbs_relay_session';
import { MemoryState } from './durable_runtime';
import { declaration, digest, GROUP, INSTANCE, key, keys, NOW, receiveQuery, signed, wake } from './relay_support';

beforeEach(() => vi.spyOn(Date, 'now').mockReturnValue(NOW));
afterEach(() => vi.restoreAllMocks());

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

async function fixture(open = true, offer = true) {
  const ctx = new MemoryState();
  const owner = keys[0];
  const state = createGroup(GROUP, {
    deviceId: owner.deviceId, publicKey: owner.publicKey,
    membershipId: owner.membershipId, name: 'Fixture owner',
  }, NOW);
  const peer = keys[1];
  state.members[peer.deviceId] = {
    deviceId: peer.deviceId, publicKey: peer.publicKey,
    membershipId: peer.membershipId, name: 'Fixture peer',
    role: 'member', lastSeenAt: NOW,
  };
  await ctx.storage.put('state', state);
  const group = new BbsGroup(ctx as any, {} as any);
  // Inspect the DO-owned memory, not a substitute reducer or a mocked signature.
  const memory = (group as unknown as { relay: RelayMemory }).relay;
  const boot = memory.boot;
  const intent = wake();
  await ctx.storage.put(pairKey(owner.deviceId, peer.deviceId), intent);
  const call = (actor: (typeof keys)[number], route: string, body: unknown = null,
    opts: Parameters<typeof signed>[3] = {}) =>
    group.fetch(signed(actor, route, body, { boot, ...opts }).request);
  const ready = { wake: intent.id, clientInstance: intent.clientInstance, serverInstance: intent.serverInstance };
  for (const actor of keys) expect((await call(actor, 'ready', ready)).status).toBe(200);
  const statements = declaration(intent, boot);
  if (offer) expect((await call(owner, 'open', { client: statements.client, server: null })).status).toBe(200);
  let session = '';
  if (open) {
    const response = await call(owner, 'open', statements);
    expect(response.status).toBe(200);
    session = (await response.json() as { session: string }).session;
  }
  const revoke = async (actor: (typeof keys)[number]) => {
    const current = await ctx.storage.get<GroupReducerState>('state');
    delete current!.members[actor.deviceId];
    current!.memberVersion++;
    await ctx.storage.put('state', current);
  };
  return { ctx, group, memory, boot, call, revoke, statements, session, ready };
}

// Pause only after real WebCrypto verification succeeded, before its caller
// can continue. A storage update here models another DO request during await.
function pauseVerification(ordinal = 1) {
  const entered = deferred(), release = deferred();
  const verify = crypto.subtle.verify.bind(crypto.subtle);
  let calls = 0;
  vi.spyOn(crypto.subtle, 'verify').mockImplementation(async (...args) => {
    const okay = await verify(...args);
    if (++calls === ordinal) {
      expect(okay).toBe(true);
      entered.resolve();
      await release.promise;
    }
    return okay;
  });
  return { entered, release };
}

async function announcements(f: Awaited<ReturnType<typeof fixture>>) {
  for (const [index, actor] of keys.entries()) {
    await f.ctx.storage.put(`relay:announcement:${actor.deviceId}`, {
      device: actor.deviceId, membership: actor.membershipId,
      requestId: `announce-fixture-${index}`, fingerprint: 'f'.repeat(64),
      instance: INSTANCE[index], revision: 'a'.repeat(64), peerVersion: 4,
    });
  }
}
const newWake = () => ({
  requestId: 'new-wake-fixture-0001', previous: wake().id, createdAt: NOW,
  instance: INSTANCE[0], peer: keys[1].deviceId, peerInstance: INSTANCE[1],
});

interface PollPage {
  boot: string;
  items: Array<{ device: string; handshake: {
    ready: { client: boolean; server: boolean };
    client: SignedTlsStatement | null;
    server: SignedTlsStatement | null;
    session: string | null;
    closed: boolean;
  } | null }>;
  next: string | null;
}
async function poll(f: Awaited<ReturnType<typeof fixture>>, actor = keys[0]) {
  const response = await f.call(actor, 'poll');
  expect(response.status, await response.clone().text()).toBe(200);
  return await response.json() as PollPage;
}
async function error(response: Response, status: number, code: string) {
  expect(response.status).toBe(status);
  expect(await response.json()).toEqual({ ok: false, error: code });
}
function resign(value: SignedTlsStatement, actor = keys[0]): SignedTlsStatement {
  return { statement: value.statement,
    signature: sign(null, Buffer.from(tlsSigningMessage(value.statement)), actor.privateKey).toString('base64') };
}

describe('relay boot-local handshake discovery', () => {
  it('exchanges both signed declarations through actual fetch without durable writes or third-member disclosure', async () => {
    const f = await fixture(false, false);
    const third = key('51'.repeat(32), 'membership-third-0001');
    const state = await f.ctx.storage.get<GroupReducerState>('state');
    state!.members[third.deviceId] = { deviceId: third.deviceId, publicKey: third.publicKey,
      membershipId: third.membershipId, name: 'Third member', role: 'member', lastSeenAt: NOW };
    await f.ctx.storage.put('state', state);
    const durable = structuredClone(f.ctx.data);
    const put = vi.spyOn(f.ctx.storage, 'put');
    const alarm = vi.spyOn(f.ctx.storage, 'setAlarm');
    const get = vi.spyOn(f.ctx.storage, 'get');
    const list = vi.spyOn(f.ctx.storage, 'list');
    const timers = vi.spyOn(globalThis, 'setTimeout');
    const before = await poll(f);
    expect(before.boot).toBe(f.boot);
    expect(before.items.find((i) => i.device === keys[1].deviceId)?.handshake).toEqual({
      ready: { client: true, server: true }, client: null, server: null, session: null, closed: false,
    });
    const offer = { client: f.statements.client, server: null };
    const accepted = await f.call(keys[0], 'open', offer);
    expect(accepted.status).toBe(200);
    expect(await accepted.json()).toEqual({ boot: f.boot, offered: true });
    const serverView = await poll(f, keys[1]);
    const client = serverView.items.find((i) => i.device === keys[0].deviceId)!.handshake!.client;
    expect(client).toEqual(f.statements.client);
    const opened = await f.call(keys[1], 'open', { client, server: f.statements.server });
    expect(opened.status).toBe(200);
    const session = await opened.json() as { session: string };
    const clientView = await poll(f);
    expect(clientView.items.find((i) => i.device === keys[1].deviceId)?.handshake).toEqual({
      ready: { client: true, server: true }, ...f.statements, session: session.session, closed: false,
    });
    const retry = await f.call(keys[0], 'open', f.statements);
    expect(retry.status).toBe(200);
    expect(await retry.json()).toEqual({ boot: f.boot, session: session.session, closed: false });
    const unseen = await poll(f, third);
    expect(unseen.items.every((i) => i.handshake === null)).toBe(true);
    expect(JSON.stringify(unseen)).not.toContain(f.statements.client.signature);
    expect(JSON.stringify(unseen)).not.toContain(f.statements.server.signature);
    expect(f.ctx.data).toEqual(durable);
    expect(put).not.toHaveBeenCalled();
    expect(alarm).not.toHaveBeenCalled();
    expect(get.mock.calls.every(([k]) => !String(k).includes('nonce'))).toBe(true);
    expect(list.mock.calls.every(([o]) => !o.prefix.includes('nonce'))).toBe(true);
    expect(timers).not.toHaveBeenCalled();
    if (process.env.KOTA_EMIT_RELAY_DISCOVERY === '1') {
      console.log('BBS_RELAY_DISCOVERY_FIXTURE ' + JSON.stringify({
        now: NOW, initial: before, offered: serverView, opened: clientView, openResponse: session,
      }));
    }
  });

  it('admits only the small device once and requires byte-identical client declarations in the dual open', async () => {
    const f = await fixture(false, false);
    const offer = { client: f.statements.client, server: null };
    await error(await f.call(keys[1], 'open', offer), 403, 'unauthorized');
    expect((await f.call(keys[0], 'open', offer)).status).toBe(200);
    expect((await f.call(keys[0], 'open', offer)).status).toBe(200);
    const changed = structuredClone(f.statements.client);
    changed.statement.certificateSha256 = 'f'.repeat(64);
    await error(await f.call(keys[0], 'open', { client: resign(changed), server: null }), 409, 'relay_wake_used');
    // Same valid device signature and semantic statement, different signed JSON bytes.
    const reordered = { signature: f.statements.client.signature, statement: f.statements.client.statement };
    await error(await f.call(keys[1], 'open', { client: reordered, server: f.statements.server }), 409, 'invalid_tls_statement');
    expect(f.memory.stats().sessions).toBe(0);
    expect((await f.call(keys[1], 'open', f.statements)).status).toBe(200);
    await error(await f.call(keys[0], 'open', offer), 409, 'relay_wake_used');
    expect(f.memory.stats().sessions).toBe(1);
  });

  it('rechecks a staged offer after its device signature verification and stores nothing on revocation', async () => {
    const f = await fixture(false, false);
    const before = f.memory.stats();
    const gate = pauseVerification(2);
    const response = f.call(keys[0], 'open', { client: f.statements.client, server: null });
    await gate.entered.promise;
    try { await f.revoke(keys[0]); } finally { gate.release.resolve(); }
    await error(await response, 403, 'unauthorized');
    expect(f.memory.stats()).toEqual(before);
    expect(JSON.stringify(await poll(f, keys[1]))).not.toContain(f.statements.client.signature);
  });

  it('rejects an old wake replaced while offer verification is awaiting', async () => {
    const f = await fixture(false, false);
    await announcements(f);
    const gate = pauseVerification(2);
    const response = f.call(keys[0], 'open', { client: f.statements.client, server: null });
    await gate.entered.promise;
    try { expect((await f.call(keys[0], 'wake', newWake())).status).toBe(200); }
    finally { gate.release.resolve(); }
    await error(await response, 409, 'relay_wake_changed');
    expect(f.memory.stats()).toEqual({ sessions: 0, ready: 0, payloadBytes: 0 });
    expect(JSON.stringify(await poll(f))).not.toContain(f.statements.client.signature);
  });

  it('drops declarations on final close and wake replacement while retaining closed replay protection', async () => {
    const f = await fixture();
    expect((await f.call(keys[1], 'ack', { through: '0', final: true }, {
      session: f.session, final: true,
    })).status).toBe(200);
    expect(f.memory.stats()).toEqual({ sessions: 1, ready: 0, payloadBytes: 0 });
    const closed = (await poll(f)).items.find((i) => i.device === keys[1].deviceId)!.handshake;
    expect(closed).toEqual({ ready: { client: false, server: false },
      client: null, server: null, session: f.session, closed: true });
    await error(await f.call(keys[0], 'open', { client: f.statements.client, server: null }), 409, 'relay_wake_used');
    const replay = await f.call(keys[0], 'open', f.statements);
    expect(await replay.json()).toEqual({ boot: f.boot, session: f.session, closed: true });
    await announcements(f);
    expect((await f.call(keys[0], 'wake', newWake())).status).toBe(200);
    const replaced = (await poll(f)).items.find((i) => i.device === keys[1].deviceId)!.handshake;
    expect(replaced).toEqual({ ready: { client: false, server: false },
      client: null, server: null, session: null, closed: false });
    await error(await f.call(keys[0], 'open', { client: f.statements.client, server: null }), 400, 'relay_wake_expired');
  });

  it('cold boot loses all declarations and refuses old boot proofs and declarations', async () => {
    const f = await fixture();
    const cold = new BbsGroup(f.ctx as any, {} as any);
    const read = await cold.fetch(signed(keys[0], 'poll').request);
    const page = await read.json() as PollPage;
    expect(page.boot).not.toBe(f.boot);
    expect(page.items.find((i) => i.device === keys[1].deviceId)?.handshake).toEqual({
      ready: { client: false, server: false }, client: null, server: null, session: null, closed: false,
    });
    await error(await cold.fetch(signed(keys[0], 'ready', f.ready, { boot: f.boot }).request), 409, 'relay_session_lost');
    for (const actor of keys)
      expect((await cold.fetch(signed(actor, 'ready', f.ready, { boot: page.boot }).request)).status).toBe(200);
    await error(await cold.fetch(signed(keys[0], 'open', { client: f.statements.client, server: null }, {
      boot: page.boot,
    }).request), 409, 'invalid_tls_statement');
    const fresh = declaration(wake(), page.boot);
    expect((await cold.fetch(signed(keys[0], 'open', { client: fresh.client, server: null }, { boot: page.boot }).request)).status).toBe(200);
    expect((await cold.fetch(signed(keys[1], 'open', fresh, { boot: page.boot }).request)).status).toBe(200);
  });

  it('rejects oversize declarations before retaining them and hides expired offers without renewing their TTL', async () => {
    const f = await fixture(false, false);
    const tooLarge = structuredClone(f.statements.client);
    tooLarge.signature = 'x'.repeat(MAX_DECLARATION_BYTES);
    await error(await f.call(keys[0], 'open', { client: tooLarge, server: null }), 413, 'invalid_tls_statement');
    expect((await poll(f)).items.find((i) => i.device === keys[1].deviceId)?.handshake?.client).toBeNull();
    const short = structuredClone(f.statements.client);
    short.statement.expiresAt = NOW + 1000;
    expect((await f.call(keys[0], 'open', { client: resign(short), server: null })).status).toBe(200);
    const bytes = structuredClone(f.ctx.data);
    vi.mocked(Date.now).mockReturnValue(NOW + 1001);
    const page = await poll(f);
    expect(page.items.find((i) => i.device === keys[1].deviceId)?.handshake?.client).toBeNull();
    expect(f.ctx.data).toEqual(bytes);
  });

  it('paginates whole declaration items within the encoded 64 KiB response without omissions', async () => {
    const ctx = new MemoryState();
    const actors = [keys[0], ...Array.from({ length: 16 }, (_, i) =>
      key((70 + i).toString(16).repeat(32), `membership-page-${String(i).padStart(4, '0')}`))];
    const owner = actors[0];
    const state = createGroup(GROUP, { deviceId: owner.deviceId, publicKey: owner.publicKey,
      membershipId: owner.membershipId, name: 'Page owner' }, NOW);
    for (const actor of actors.slice(1)) state.members[actor.deviceId] = {
      deviceId: actor.deviceId, publicKey: actor.publicKey, membershipId: actor.membershipId,
      role: 'member', lastSeenAt: NOW, name: 'Page peer',
    };
    await ctx.storage.put('state', state);
    const group = new BbsGroup(ctx as any, {} as any);
    const boot = (group as unknown as { relay: RelayMemory }).relay.boot;
    // An adversarial but canonical origin drives the encoded-byte boundary.
    // Request stays in this DO double; there is no DNS or external network.
    const origin = `https://${'x'.repeat(2800)}.example`;
    const call = (actor: typeof owner, route: string, body: unknown, query?: string) =>
      group.fetch(signed(actor, route, body, { boot, origin, query }).request);
    for (const [i, peer] of actors.slice(1).entries()) {
      const pair = [owner, peer].sort((a, b) => a.deviceId.localeCompare(b.deviceId));
      const intent = { ...wake(), id: digest(`page-wake-${i}`), client: pair[0].deviceId,
        server: pair[1].deviceId, clientMembership: pair[0].membershipId, serverMembership: pair[1].membershipId };
      await ctx.storage.put(pairKey(owner.deviceId, peer.deviceId), intent);
      const ready = { wake: intent.id, clientInstance: intent.clientInstance, serverInstance: intent.serverInstance };
      for (const actor of pair) expect((await call(actor, 'ready', ready)).status).toBe(200);
      const body = declaration(intent, boot, NOW, pair, origin);
      expect(Buffer.byteLength(JSON.stringify(body.client))).toBeLessThanOrEqual(MAX_DECLARATION_BYTES);
      const response = await call(pair[0], 'open', { client: body.client, server: null });
      expect(response.status, await response.clone().text()).toBe(200);
    }
    const persisted = structuredClone(ctx.data);
    const put = vi.spyOn(ctx.storage, 'put'), alarm = vi.spyOn(ctx.storage, 'setAlarm');
    const seen: string[] = [];
    let after: string | null = null, pageCount = 0, sawByteLimit = false;
    do {
      const response = await call(owner, 'poll', null, `?group=${GROUP}${after ? `&after=${after}` : ''}`);
      expect(response.status).toBe(200);
      const text = await response.text();
      expect(Buffer.byteLength(text)).toBeLessThanOrEqual(65536);
      const page = JSON.parse(text) as PollPage;
      expect(page.items.length).toBeLessThanOrEqual(16);
      expect(page.items.length).toBeGreaterThan(0);
      for (const item of page.items) {
        expect(item.device > (after ?? '')).toBe(true);
        if (item.device !== owner.deviceId) expect(item.handshake?.client?.signature).toMatch(/^[A-Za-z0-9+/]+={0,2}$/);
        seen.push(item.device);
      }
      if (page.next) {
        expect(page.next).toBe(page.items.at(-1)?.device);
        expect(page.next > (after ?? '')).toBe(true);
        sawByteLimit ||= page.items.length < 16;
      }
      after = page.next;
      expect(++pageCount).toBeLessThanOrEqual(3);
    } while (after);
    expect(sawByteLimit).toBe(true);
    expect(seen).toEqual(actors.map((k) => k.deviceId).sort());
    expect(ctx.data).toEqual(persisted);
    expect(put).not.toHaveBeenCalled();
    expect(alarm).not.toHaveBeenCalled();
  });
});

describe('relay DO verification and error boundaries', () => {
  it('dispatches binary send and body-free receive without parsing either as JSON', async () => {
    const f = await fixture();
    const ciphertext = new Uint8Array([0, 255, 128, 1]);
    const sent = await f.call(keys[0], 'send', ciphertext, { session: f.session });
    expect(sent.status, await sent.clone().text()).toBe(200);
    const received = await f.call(keys[1], 'receive', null, {
      query: receiveQuery(f.session, 0, 'data', f.boot),
    });
    expect(received.status, await received.clone().text()).toBe(200);
    const bytes = new Uint8Array(await received.arrayBuffer());
    expect(new TextDecoder().decode(bytes.subarray(0, 4))).toBe('KBR1');
    const length = new DataView(bytes.buffer).getUint32(4);
    const metadata = JSON.parse(new TextDecoder().decode(bytes.subarray(8, 8 + length)));
    expect(metadata).toMatchObject({ boot: f.boot, items: [{
      session: f.session, next: '1', consumed: '0', closed: false,
      batches: [{ sequence: '0', length: ciphertext.length }], ack: null,
    }] });
    expect(bytes.subarray(8 + length)).toEqual(ciphertext);
    expect(f.memory.stats().payloadBytes).toBe(ciphertext.length);
  });

  it('rechecks membership after authentication and cannot enqueue a revoked sender', async () => {
    const f = await fixture();
    const before = f.memory.stats();
    const gate = pauseVerification();
    const response = f.call(keys[0], 'send', new Uint8Array([1, 2, 3]), { session: f.session });
    await gate.entered.promise;
    try { await f.revoke(keys[0]); } finally { gate.release.resolve(); }
    const refused = await response;
    expect(refused.status).toBe(403);
    expect(await refused.json()).toMatchObject({ error: 'unauthorized' });
    expect(f.memory.stats()).toEqual(before);
  });

  it('rechecks membership before a signed ACK can release retained ciphertext', async () => {
    const f = await fixture();
    expect((await f.call(keys[0], 'send', new Uint8Array([7, 8, 9]), { session: f.session })).status).toBe(200);
    const before = f.memory.stats();
    const gate = pauseVerification();
    const response = f.call(keys[1], 'ack', { through: '1', final: false }, { session: f.session });
    await gate.entered.promise;
    try { await f.revoke(keys[1]); } finally { gate.release.resolve(); }
    const refused = await response;
    expect(refused.status).toBe(403);
    expect(await refused.json()).toMatchObject({ error: 'unauthorized' });
    expect(f.memory.stats().payloadBytes).toBe(3);
    expect(f.memory.stats()).toEqual(before);
  });

  it('rechecks after TLS declaration verification before creating a session', async () => {
    const f = await fixture(false);
    const before = f.memory.stats();
    // First verification authenticates the request; second is the TLS client declaration.
    const gate = pauseVerification(2);
    const response = f.call(keys[0], 'open', f.statements);
    await gate.entered.promise;
    try { await f.revoke(keys[0]); } finally { gate.release.resolve(); }
    const refused = await response;
    expect(refused.status).toBe(403);
    expect(await refused.json()).toMatchObject({ error: 'unauthorized' });
    expect(f.memory.stats().sessions).toBe(0);
    expect(f.memory.stats()).toEqual(before);
  });

  it('rechecks membership after asynchronous wake preparation before persisting an intent', async () => {
    const f = await fixture();
    await announcements(f);
    const entered = deferred(), release = deferred();
    const digest = crypto.subtle.digest.bind(crypto.subtle);
    vi.spyOn(crypto.subtle, 'digest').mockImplementation(async (algorithm, data) => {
      const bytes = ArrayBuffer.isView(data)
        ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
        : new Uint8Array(data);
      const value = await digest(algorithm, data);
      if (new TextDecoder().decode(bytes).startsWith('["kota-bbs-relay.wake-id.v1",')) {
        entered.resolve();
        await release.promise;
      }
      return value;
    });
    const response = f.call(keys[0], 'wake', newWake());
    await entered.promise;
    try { await f.revoke(keys[0]); } finally { release.resolve(); }
    const refused = await response;
    expect(refused.status).toBe(403);
    expect(await refused.json()).toMatchObject({ error: 'unauthorized' });
    const current = f.ctx.data.get(pairKey(keys[0].deviceId, keys[1].deviceId));
    expect(current).toEqual(wake());
    expect([...f.ctx.data.keys()].filter((key) => key.includes(':request:') || key.includes(':nonce:'))).toEqual([]);
  });

  it('commits metadata rows and receipts atomically and retires replaced wake memory only after commit', async () => {
    const f = await fixture();
    await announcements(f);
    expect((await f.call(keys[0], 'send', new Uint8Array([7]), { session: f.session })).status).toBe(200);
    const before = structuredClone(f.ctx.data);
    const memory = f.memory.stats();
    const put = f.ctx.storage.put;
    const fail = vi.spyOn(f.ctx.storage, 'put').mockImplementation(async (key, value) => {
      if (typeof key === 'string' && key.includes(':request:')) throw new Error('fixture storage failure');
      return put(key, value);
    });
    const rejected = await f.call(keys[0], 'wake', newWake());
    expect(rejected.status).toBe(503);
    expect(await rejected.json()).toEqual({ ok: false, error: 'relay_service_limit' });
    expect(f.ctx.data).toEqual(before);
    expect(f.memory.stats()).toEqual(memory);
    expect(f.memory.readyStatus(wake())).toEqual({ boot: f.boot, client: true, server: true });
    fail.mockRestore();

    const accepted = await f.call(keys[0], 'wake', newWake());
    expect(accepted.status).toBe(200);
    const result = await accepted.json() as { current: string };
    expect(result.current).not.toBe(wake().id);
    expect(f.memory.stats()).toMatchObject({ ready: 0, payloadBytes: 0 });
    expect(f.memory.readyStatus(wake())).toEqual({ boot: f.boot, client: false, server: false });
    const retry = await f.call(keys[0], 'wake', newWake());
    expect(retry.status).toBe(200);
    expect(await retry.json()).toMatchObject({ ok: true, current: result.current, changed: false });
    expect(f.memory.stats()).toMatchObject({ ready: 0, payloadBytes: 0 });
  });

  it('bad query, signature and backpressure preserve existing ready and session state', async () => {
    const f = await fixture();
    const blocks = vi.spyOn(f.ctx, 'blockConcurrencyWhile');
    for (let sequence = 0; sequence < 2; sequence++) {
      expect((await f.call(keys[0], 'send', new Uint8Array([sequence + 1]), {
        session: f.session, sequence,
      })).status).toBe(200);
    }
    const before = f.memory.stats();
    const badQuery = await f.call(keys[1], 'receive', null, {
      query: receiveQuery(f.session, 0, 'data', f.boot) + '&mode=data',
    });
    expect(badQuery.status).toBe(400);
    expect(f.memory.stats()).toEqual(before);
    expect(f.memory.readyStatus(wake())).toEqual({ boot: f.boot, client: true, server: true });
    const bad = signed(keys[0], 'send', new Uint8Array([3]), { boot: f.boot, session: f.session, sequence: 2 }).request;
    bad.headers.set('x-kota-relay-signature', btoa(String.fromCharCode(0).repeat(64)));
    const badSignature = await f.group.fetch(bad);
    expect(badSignature.status).toBe(403);
    expect(f.memory.stats()).toEqual(before);
    expect(f.memory.readyStatus(wake())).toEqual({ boot: f.boot, client: true, server: true });
    const pressure = await f.call(keys[0], 'send', new Uint8Array([3]), { session: f.session, sequence: 2 });
    expect(pressure.status).toBe(429);
    expect(await pressure.json()).toMatchObject({ error: 'relay_backpressure' });
    expect(f.memory.stats()).toEqual(before);
    expect(f.memory.readyStatus(wake())).toEqual({ boot: f.boot, client: true, server: true });
    expect((await f.call(keys[0], 'ready', f.ready)).status).toBe(200);
    const reopen = await f.call(keys[0], 'open', f.statements);
    expect(reopen.status).toBe(200);
    expect(await reopen.json()).toMatchObject({ boot: f.boot, session: f.session, closed: false });
    const received = await f.call(keys[1], 'receive', null, {
      query: receiveQuery(f.session, 0, 'data', f.boot),
    });
    expect(received.status).toBe(200);
    expect(received.headers.get('content-type')).toContain('application/octet-stream');
    expect(blocks).not.toHaveBeenCalled();
  });
});
