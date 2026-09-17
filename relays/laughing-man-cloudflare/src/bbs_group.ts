import { DurableObject } from 'cloudflare:workers';
import type { Env } from './index';
import {
  activeMember,
  BBS_PROTOCOL_VERSION,
  createGroup,
  failure,
  reduce,
  type Actor,
  type ControlResponse,
  type GroupReducerState,
  type GroupRequest,
  type Member,
} from './bbs_group_reducer';
import {
  AUTH_WINDOW_MS,
  base64Bytes,
  ControlError,
  displayName,
  hashId,
  RateLimiter,
  readBody,
  sha256,
  tokenId,
  verifyProof,
  type Proof,
} from './bbs_auth';
import { assertCurrent, authenticateRelay, exactObject, RELAY_JSON_BYTES, relayJson, readRelayInput,
  relayTarget, type RelayMembers } from './bbs_relay_auth';
import { applyMutation, pairKey, pollMetadata, prepareMutation, type RelayStorage } from './bbs_relay_metadata';
import { RelayMemory } from './bbs_relay_memory';
import { verifyOffer, verifySession, type Wake } from './bbs_relay_session';
import { relayReceiveResponse } from './bbs_relay_envelope';

const SIGNAL_TTL_MS = 120_000;
const SIGNAL_LIMIT = 128;
const SIGNAL_PER_RECIPIENT = 32;
const NONCE_LIMIT = 4096;
const ROUTING_LIMIT = 200;
const ROUTING_TTL_MS = 24 * 60 * 60_000;
const limiter = new RateLimiter();
interface Signal {
  id: string;
  from: string;
  to: string;
  payload: string;
  expiresAt: number;
}
interface OwnerRegistry {
  current: string | null;
}
interface GroupRecord {
  requestId: string;
  fingerprint: string;
  phase: 'creating' | 'active' | 'revoking' | 'revoked';
}
interface JoinRoute {
  groupId: string;
  publicKey: string;
  fingerprint: string;
  at: number;
}
function response(value: unknown, code = 200): Response {
  return new Response(JSON.stringify(value), {
    status: code,
    headers: { 'content-type': 'application/json', 'cache-control': 'no-store' },
  });
}
function reply(value: ControlResponse): Response {
  return response(
    value,
    value.ok ? 200 : ['unauthorized', 'owner_required'].includes(value.error ?? '') ? 403 : 409,
  );
}
function ok(value: object = {}): Record<string, unknown> {
  return { protocolVersion: BBS_PROTOCOL_VERSION, ok: true, ...value };
}
function expectedGroup(body: Record<string, unknown>): string {
  return tokenId(body.expectedGroupId);
}

/** Only control metadata lives here. The fixed `owner` instance coordinates group lifetimes. */
export class BbsGroup extends DurableObject<Env> {
  private readonly relay = new RelayMemory(crypto.randomUUID().replaceAll('-', ''));
  private group(id: string) {
    return this.env.BBS_GROUP.get(this.env.BBS_GROUP.idFromName(id));
  }
  private async requireOwner(request: Request): Promise<void> {
    const secret = request.headers.get('x-kota-standby-secret');
    if (
      !secret ||
      !(await this.env.RELAY_STATE.get(
        this.env.RELAY_STATE.idFromName('kota-laughing-man-standby'),
      ).authorizeBbsOwner(secret))
    )
      throw new ControlError('unauthorized', 401);
  }
  // RPC only: never dispatch these methods from an HTTP path.
  async initialize(
    groupId: string,
    owner: Omit<Member, 'role' | 'lastSeenAt'>,
  ): Promise<ControlResponse> {
    return this.ctx.storage.transaction(async (storage) => {
      const previous = await storage.get<GroupReducerState>('state');
      if (previous) {
        if (previous.dissolved) return failure('removed');
        if (
          previous.groupId !== groupId ||
          previous.members[previous.owner]?.membershipId !== owner.membershipId ||
          previous.members[previous.owner]?.publicKey !== owner.publicKey
        )
          return failure('request_conflict');
        return {
          protocolVersion: BBS_PROTOCOL_VERSION,
          ok: true,
          groupId,
          role: 'owner',
          membershipId: owner.membershipId,
        };
      }
      await storage.put('state', createGroup(groupId, owner, Date.now()));
      return {
        protocolVersion: BBS_PROTOCOL_VERSION,
        ok: true,
        groupId,
        role: 'owner',
        membershipId: owner.membershipId,
      };
    });
  }
  async ownerAction(groupId: string, action: GroupRequest): Promise<ControlResponse> {
    return this.apply(groupId, { kind: 'owner' }, action);
  }
  async revokeGroup(groupId: string): Promise<ControlResponse> {
    // Prevent HTTP and RPC requests entering between deleteAll and the revocation marker.
    return this.ctx.blockConcurrencyWhile(async () => {
      const previous = await this.ctx.storage.get<GroupReducerState>('state');
      if (previous && previous.groupId !== groupId) return failure('group_mismatch');
      await this.ctx.storage.deleteAll();
      await this.ctx.storage.deleteAlarm();
      await this.ctx.storage.put('state', {
        groupId,
        owner: '',
        dissolved: true,
        memberVersion: 0,
        members: {},
        invite: { gen: 0, hash: null, expiresAt: 0 },
        oldHashes: [],
        receipts: {},
        revokedJoins: {},
      } satisfies GroupReducerState);
      return { protocolVersion: BBS_PROTOCOL_VERSION, ok: true, groupId };
    });
  }
  async redeem(
    groupId: string,
    proof: Proof,
    action: Extract<GroupRequest, { kind: 'redeem' }>,
  ): Promise<ControlResponse> {
    return this.apply(groupId, { kind: 'joiner', ...proof }, action, proof);
  }
  private async apply(
    groupId: string,
    actor: Actor,
    action: GroupRequest,
    proof?: Proof,
  ): Promise<ControlResponse> {
    const now = Date.now();
    return this.ctx.storage.transaction(async (storage) => {
      const state = await storage.get<GroupReducerState>('state');
      if (!state || state.groupId !== groupId || state.dissolved) return failure('removed');
      // Permanent revocations use individual keys: loading one request never scans history.
      if (action.kind === 'redeem') {
        const revoked = await storage.get<{ publicKey: string; deviceId: string }>(
          `revoked:${action.requestId}`,
        );
        if (revoked) state.revokedJoins[action.requestId] = revoked;
      }
      const result = reduce(state, actor, action, now);
      // Unauthenticated/nonmember attempts must not allocate nonce storage.
      if (result.response.ok && proof) await consumeNonce(storage, proof, now);
      if (result.response.ok && action.kind !== 'status') {
        for (const [id, revoked] of Object.entries(result.state.revokedJoins))
          await storage.put(`revoked:${id}`, revoked);
        result.state.revokedJoins = {};
        await storage.put('state', result.state);
      }
      if (result.response.ok && (action.kind === 'remove' || action.kind === 'leave')) {
        const device =
          action.kind === 'remove'
            ? action.deviceId
            : (actor as Extract<Actor, { kind: 'member' }>).deviceId;
        for (const [key, signal] of await storage.list<Signal>({ prefix: 'signal:' })) {
          if (signal.from === device || signal.to === device) await storage.delete(key);
        }
      }
      if (result.response.ok && action.kind === 'heartbeat' && actor.kind === 'member') {
        // One authenticated heartbeat: membership plus a bounded, consumed
        // signal batch. State, proof nonce and consumption share a transaction.
        const mine: Signal[] = [];
        for (const [key, signal] of await storage.list<Signal>({ prefix: 'signal:' })) {
          if (signal.expiresAt <= now) {
            await storage.delete(key);
          } else if (signal.to === actor.deviceId && mine.length < 8) {
            mine.push(signal);
            await storage.delete(key);
          }
        }
        result.response.signals = mine;
      }
      return result.response;
    });
  }
  private async relayMembers(storage: Pick<DurableObjectStorage, 'get'> = this.ctx.storage): Promise<RelayMembers> {
    const state = await storage.get<GroupReducerState>('state');
    if (!state) throw new ControlError('removed', 409);
    return {
      groupId: state.groupId,
      dissolved: state.dissolved,
      members: Object.fromEntries(Object.entries(state.members).map(([id, m]) => [id, {
        deviceId: m.deviceId, publicKey: m.publicKey, membershipId: m.membershipId,
      }])),
    };
  }
  private relayStorage(storage: Pick<DurableObjectStorage, 'get' | 'put' | 'getAlarm' | 'setAlarm'> = this.ctx.storage): RelayStorage {
    return {
      get: <T>(key: string) => storage.get<T>(key),
      put: <T>(key: string, value: T) => storage.put(key, value),
      getAlarm: () => storage.getAlarm(),
      setAlarm: (at: number) => storage.setAlarm(at),
    };
  }
  private async relayWakeMembers(wake: Wake): Promise<RelayMembers> {
    // Crypto verification may overlap a newer wake. These storage reads share
    // the DO input gate; no non-storage await separates them from the reducer.
    const current = await this.ctx.storage.get<Wake>(pairKey(wake.client, wake.server));
    const members = await this.relayMembers();
    if (!current || JSON.stringify(current) !== JSON.stringify(wake))
      throw new ControlError('relay_wake_changed', 409);
    return members;
  }
  private async relayRequest(request: Request): Promise<Response> {
    const now = Date.now();
    const target = relayTarget(request);
    const initialMembers = await this.relayMembers();
    const input = await authenticateRelay(request, initialMembers, now);
    if (!limiter.allow(input.proof.device, target.route, now, 1500))
      throw new ControlError('rate_limited', 429);
    // Ed25519 verification yields to the DO. Re-read membership after that
    // await and serialize the final check with the relay mutation/read.
    const verifiedMembers = await this.relayMembers();
    const members = verifiedMembers;
    if (target.route === 'poll') {
      const page = await pollMetadata(input, members, this.relayStorage(), now);
      const current = await this.relayMembers();
      assertCurrent(input, current);
      type Item = (typeof page.items)[number] & { handshake: ReturnType<RelayMemory['handshake']> | null };
      const items: Item[] = [];
      let next = page.next;
      for (const row of page.items) {
        const peer = current.members[row.device];
        if (!peer) continue;
        const unchanged = members.members[row.device]?.membershipId === peer.membershipId;
        const wake = unchanged ? row.wake : null;
        const item = { ...row,
          announcement: row.announcement?.membership === peer.membershipId ? row.announcement : null,
          currentWake: unchanged ? row.currentWake : null,
          wake,
          handshake: wake ? this.relay.handshake(input, current, wake, now) : null,
        };
        // Include the largest possible next cursor while checking encoded size.
        // Never split or silently omit an otherwise valid declaration item.
        const bytes = new TextEncoder().encode(JSON.stringify({ boot: this.relay.boot,
          items: [...items, item], next: row.device }));
        if (bytes.length > RELAY_JSON_BYTES) {
          if (!items.length) throw new ControlError('invalid_relay_metadata', 413);
          next = items[items.length - 1].device;
          break;
        }
        items.push(item);
      }
      return response({ boot: this.relay.boot, items, next });
    }
    if (target.route === 'announce' || target.route === 'wake') {
      const prepared = await prepareMutation(input);
      // Wake ID hashing is asynchronous too: a removal may have completed
      // after authentication. The member read and all receipt/nonce/row writes
      // share a storage transaction with membership changes; no crypto await
      // or boot-local side effect occurs in a retriable transaction callback.
      const result = await this.ctx.storage.transaction(async (storage) =>
        applyMutation(prepared, await this.relayMembers(storage), this.relayStorage(storage), now));
      if (result.replacedWake) this.relay.replaceWake(result.replacedWake);
      return response(result);
    }
    if (target.route === 'ready') {
      const body = relayJson(input);
      const wakeId = typeof (body as any).wake === 'string' ? (body as any).wake : '';
      const rows = await this.ctx.storage.list<Wake>({ prefix: 'relay:wake:' });
      const wake = [...rows.values()].find((w) => w.id === wakeId);
      if (!wake) throw new ControlError('relay_wake_expired');
      return response(this.relay.ready(input, await this.relayWakeMembers(wake), wake, now));
    }
    if (target.route === 'open') {
      const body = exactObject(relayJson(input), ['client', 'server']);
      const value = body as { client?: { statement?: { wake?: string } }; server?: { statement?: { wake?: string } } };
      const wakeId = value.client?.statement?.wake ?? value.server?.statement?.wake ?? '';
      const rows = await this.ctx.storage.list<Wake>({ prefix: 'relay:wake:' });
      const wake = [...rows.values()].find((w) => w.id === wakeId);
      if (!wake) throw new ControlError('relay_wake_expired');
      if (body.server === null) {
        const offer = await verifyOffer(body.client, target.origin, wake, input.proof.boot, members, now);
        return response(this.relay.offer(input, await this.relayWakeMembers(wake), wake, offer, now));
      }
      const session = await verifySession(body, target.origin, wake, input.proof.boot, members, now);
      return response(this.relay.open(input, await this.relayWakeMembers(wake), wake, session, now));
    }
    if (target.route === 'send') return response(this.relay.send(input, members, now));
    if (target.route === 'ack') return response(this.relay.ack(input, members, now));
    if (target.route === 'receive') return relayReceiveResponse(input, this.relay.receive(input, members, now));
    throw new ControlError('not_found', 404);
  }
  async fetch(request: Request): Promise<Response> {
    try {
      const path = new URL(request.url).pathname;
      if (path.startsWith('/bbs/relay/')) {
        if (!limiter.allow(request.headers.get('cf-connecting-ip') ?? 'local', '/bbs/relay', Date.now(), 3000))
          throw new ControlError('rate_limited', 429);
        return await this.relayRequest(request);
      }
      const route = path.replace(/\/groups\/[^/]+\//, '/groups/:id/');
      if (
        !limiter.allow(
          request.headers.get('cf-connecting-ip') ?? 'local',
          route,
          Date.now(),
          path === '/bbs/join' ? 30 : 180,
        )
      )
        throw new ControlError('rate_limited', 429);
      if (!['POST', 'PUT'].includes(request.method)) throw new ControlError('not_found', 404);
      const { raw, body } = await readBody(request);
      if (path.startsWith('/bbs/owner/')) {
        await this.requireOwner(request);
        return await this.ownerRequest(path, body);
      }
      if (path === '/bbs/join') {
        const proof = await verifyProof(request, raw, '', Date.now());
        return await this.joinRequest(body, proof);
      }
      const match = path.match(
        /^\/bbs\/groups\/([A-Za-z0-9_-]{16,80})\/(status|heartbeat|rename|leave|signal|signal\/pull)$/,
      );
      if (!match) throw new ControlError('not_found', 404);
      const groupId = match[1];
      const proof = await verifyProof(request, raw, groupId, Date.now());
      if (body.expectedGroupId !== groupId) throw new ControlError('group_mismatch', 409);
      if (match[2].startsWith('signal'))
        return await this.signalRequest(groupId, match[2], body, proof);
      const action: GroupRequest =
        match[2] === 'rename'
          ? { kind: 'rename', requestId: tokenId(body.requestId), name: displayName(body.name) }
          : match[2] === 'leave'
            ? { kind: 'leave', requestId: tokenId(body.requestId) }
            : { kind: match[2] as 'status' | 'heartbeat' };
      return reply(await this.apply(groupId, { kind: 'member', ...proof }, action, proof));
    } catch (error) {
      if (new URL(request.url).pathname.startsWith('/bbs/relay/')) {
        // Platform exceptions are not evidence of a daily quota or reset time.
        return response({ ok: false, error: error instanceof ControlError ? error.code : 'relay_service_limit' },
          error instanceof ControlError ? error.status : 503);
      }
      return replyError(error);
    }
  }
  private async ownerRequest(path: string, body: Record<string, unknown>): Promise<Response> {
    // Cross-DO lifecycle calls are intentionally serialized by this fixed owner instance.
    return this.ctx.blockConcurrencyWhile(async () => {
      const registry = (await this.ctx.storage.get<OwnerRegistry>('registry')) ?? { current: null };
      if (path === '/bbs/owner/create') {
        const groupId = tokenId(body.groupId);
        const requestId = tokenId(body.requestId);
        const publicKey = typeof body.publicKey === 'string' ? body.publicKey : '';
        const deviceId = await sha256(base64Bytes(publicKey, 32));
        if (body.deviceId !== deviceId || body.expectedGroupId !== null)
          throw new ControlError('group_mismatch', 409);
        const name = displayName(body.name);
        const fingerprint = await sha256(JSON.stringify({ requestId, deviceId, publicKey, name }));
        const key = `group:${groupId}`;
        let record = await this.ctx.storage.get<GroupRecord>(key);
        if (record?.phase === 'revoked' || record?.phase === 'revoking')
          return reply(failure('removed'));
        if (record && record.fingerprint !== fingerprint) return reply(failure('request_conflict'));
        if (registry.current && registry.current !== groupId)
          return reply(failure('group_mismatch'));
        record ??= { requestId, fingerprint, phase: 'creating' };
        // Durable intent precedes the RPC; replay uses exactly this group and identity.
        await this.ctx.storage.put({ registry: { current: groupId }, [key]: record });
        const result = await this.group(groupId).initialize(groupId, {
          deviceId,
          publicKey,
          name,
          membershipId: requestId,
        });
        if (!result.ok) return reply(result);
        await this.ctx.storage.put(key, { ...record, phase: 'active' });
        return reply(result);
      }
      const groupId = expectedGroup(body);
      const record = await this.ctx.storage.get<GroupRecord>(`group:${groupId}`);
      if (path === '/bbs/owner/dissolve' && record?.phase === 'revoked')
        return response(ok({ groupId }));
      if (registry.current !== groupId || !record) return reply(failure('group_mismatch'));
      if (path === '/bbs/owner/dissolve') {
        tokenId(body.requestId);
        await this.ctx.storage.put(`group:${groupId}`, { ...record, phase: 'revoking' });
        const result = await this.group(groupId).revokeGroup(groupId);
        if (!result.ok) return reply(result);
        await this.ctx.storage.put({
          registry: { current: null },
          [`group:${groupId}`]: { ...record, phase: 'revoked' },
        });
        return reply(result);
      }
      if (record.phase !== 'active') return reply(failure('removed'));
      let action: GroupRequest;
      switch (path) {
        case '/bbs/owner/status':
          action = { kind: 'status' };
          break;
        case '/bbs/owner/invitation': {
          const gen = body.gen;
          if (!Number.isSafeInteger(gen) || Number(gen) < 1)
            throw new ControlError('invalid_generation');
          action = {
            kind: 'invite',
            gen: Number(gen),
            hash: hashId(body.hash),
            requestId: tokenId(body.requestId),
          };
          break;
        }
        case '/bbs/owner/remove':
          action = {
            kind: 'remove',
            deviceId: hashId(body.deviceId),
            requestId: tokenId(body.requestId),
          };
          break;
        default:
          throw new ControlError('not_found', 404);
      }
      return reply(await this.group(groupId).ownerAction(groupId, action));
    });
  }
  private async joinRequest(body: Record<string, unknown>, proof: Proof): Promise<Response> {
    const requestId = tokenId(body.requestId);
    const name = displayName(body.name);
    // Invitation bearer is used only for this hash comparison and never stored or logged.
    if (typeof body.token !== 'string' || !/^[a-f0-9]{64}$/.test(body.token))
      throw new ControlError('invalid_invitation');
    const hash = await sha256(body.token);
    const fingerprint = await sha256(JSON.stringify({ hash, name, deviceId: proof.deviceId }));
    return this.ctx.blockConcurrencyWhile(async () => {
      const now = Date.now();
      const routes = await this.ctx.storage.list<JoinRoute>({ prefix: 'join:' });
      for (const [key, route] of routes)
        if (now - route.at >= ROUTING_TTL_MS) {
          await this.ctx.storage.delete(key);
          routes.delete(key);
        }
      let route = routes.get(`join:${requestId}`);
      if (route && (route.publicKey !== proof.publicKey || route.fingerprint !== fingerprint))
        return reply(failure('request_conflict'));
      const registry = await this.ctx.storage.get<OwnerRegistry>('registry');
      if (!route) {
        if (!registry?.current) return reply(failure('invalid_invitation'));
        route = { groupId: registry.current, publicKey: proof.publicKey, fingerprint, at: now };
        for (const [key] of [...routes.entries()]
          .sort((a, b) => a[1].at - b[1].at)
          .slice(0, Math.max(0, routes.size - ROUTING_LIMIT + 1)))
          await this.ctx.storage.delete(key);
        await this.ctx.storage.put(`join:${requestId}`, route);
      }
      const record = await this.ctx.storage.get<GroupRecord>(`group:${route.groupId}`);
      if (record?.phase !== 'active') return reply(failure('removed'));
      return reply(
        await this.group(route.groupId).redeem(route.groupId, proof, {
          kind: 'redeem',
          hash,
          name,
          requestId,
        }),
      );
    });
  }
  private async signalRequest(
    groupId: string,
    action: string,
    body: Record<string, unknown>,
    proof: Proof,
  ): Promise<Response> {
    const now = Date.now();
    const value = await this.ctx.storage.transaction(async (storage) => {
      const state = await storage.get<GroupReducerState>('state');
      if (!state || state.groupId !== groupId || !activeMember(state, { kind: 'member', ...proof }))
        throw new ControlError('unauthorized', 403);
      await consumeNonce(storage, proof, now);
      const signals = await storage.list<Signal>({ prefix: 'signal:' });
      for (const [key, signal] of signals)
        if (signal.expiresAt <= now) {
          await storage.delete(key);
          signals.delete(key);
        }
      if (action === 'signal/pull') {
        const mine = [...signals.entries()]
          .filter(([, signal]) => signal.to === proof.deviceId)
          .slice(0, 8);
        for (const [key] of mine) await storage.delete(key);
        return ok({ signals: mine.map(([, signal]) => signal) });
      }
      const to = hashId(body.to);
      if (!state.members[to] || to === proof.deviceId) throw new ControlError('invalid_peer');
      if (
        typeof body.payload !== 'string' ||
        !body.payload ||
        new TextEncoder().encode(body.payload).length > 64 * 1024
      )
        throw new ControlError('invalid_signal');
      if (
        signals.size >= SIGNAL_LIMIT ||
        [...signals.values()].filter((signal) => signal.to === to).length >= SIGNAL_PER_RECIPIENT
      )
        throw new ControlError('signal_queue_full', 429);
      const id = tokenId(body.requestId);
      const key = `signal:${to}:${id}`;
      const existing = signals.get(key);
      if (existing && (existing.from !== proof.deviceId || existing.payload !== body.payload))
        throw new ControlError('request_conflict', 409);
      await storage.put(key, {
        id,
        from: proof.deviceId,
        to,
        payload: body.payload,
        expiresAt: now + SIGNAL_TTL_MS,
      } satisfies Signal);
      return ok();
    });
    await this.ctx.storage.setAlarm(now + SIGNAL_TTL_MS);
    return response(value);
  }
  async alarm(): Promise<void> {
    const now = Date.now();
    // Relay ready/session state is intentionally boot-local, but its bounded
    // TTL still needs the DO alarm to retire idle or revoked entries. The
    // durable signal/nonce cleanup below remains independent.
    const members = await this.relayMembers().catch(() => null);
    if (members) this.relay.sweep(members, now);
    let remaining = false;
    let earliest = Number.POSITIVE_INFINITY;
    for (const [key, value] of await this.ctx.storage.list<unknown>({ prefix: 'relay:' })) {
      const expiresAt = typeof value === 'number'
        ? value
        : value && typeof value === 'object' && typeof (value as { expiresAt?: unknown }).expiresAt === 'number'
          ? (value as { expiresAt: number }).expiresAt
          : null;
      if (expiresAt !== null && expiresAt <= now) await this.ctx.storage.delete(key);
      else if (expiresAt !== null) {
        remaining = true;
        earliest = Math.min(earliest, expiresAt);
      }
    }
    for (const [key, signal] of await this.ctx.storage.list<Signal>({ prefix: 'signal:' })) {
      if (signal.expiresAt <= now) await this.ctx.storage.delete(key);
      else remaining = true;
    }
    for (const [key, expiresAt] of await this.ctx.storage.list<number>({ prefix: 'nonce:' })) {
      if (expiresAt <= now) await this.ctx.storage.delete(key);
      else remaining = true;
    }
    if (remaining) await this.ctx.storage.setAlarm(
      Number.isFinite(earliest) ? earliest : now + SIGNAL_TTL_MS,
    );
  }
}
async function consumeNonce(
  storage: DurableObjectTransaction,
  proof: Proof,
  now: number,
): Promise<void> {
  if (Math.abs(now - proof.timestamp) > AUTH_WINDOW_MS)
    throw new ControlError('stale_signature', 401);
  const key = `nonce:${proof.deviceId}:${proof.nonce}`;
  const nonces = await storage.list<number>({ prefix: 'nonce:' });
  for (const [id, expiresAt] of nonces)
    if (expiresAt <= now) {
      await storage.delete(id);
      nonces.delete(id);
    }
  if (nonces.has(key)) throw new ControlError('replayed_signature', 409);
  if (nonces.size >= NONCE_LIMIT) throw new ControlError('authentication_busy', 429);
  await storage.put(key, proof.timestamp + AUTH_WINDOW_MS + 1);
}
function replyError(error: unknown): Response {
  return response(
    failure(error instanceof ControlError ? error.code : 'control_unavailable'),
    error instanceof ControlError ? error.status : 503,
  );
}
export async function routeBbs(request: Request, env: Env): Promise<Response> {
  const url = new URL(request.url);
  if (url.pathname.startsWith('/bbs/relay/')) {
    const groupId = url.searchParams.get('group');
    if (!groupId || !/^[A-Za-z0-9_-]{16,80}$/.test(groupId))
      return response(failure('invalid_relay_group'), 400);
    return env.BBS_GROUP.get(env.BBS_GROUP.idFromName(groupId)).fetch(request);
  }
  if (url.search) return response(failure('invalid_url'), 400);
  const group = url.pathname.match(/^\/bbs\/groups\/([A-Za-z0-9_-]{16,80})\//);
  if (
    !group &&
    ![
      '/bbs/join',
      '/bbs/owner/create',
      '/bbs/owner/status',
      '/bbs/owner/invitation',
      '/bbs/owner/remove',
      '/bbs/owner/dissolve',
    ].includes(url.pathname)
  )
    return response(failure('not_found'), 404);
  return env.BBS_GROUP.get(env.BBS_GROUP.idFromName(group?.[1] ?? 'owner')).fetch(request);
}
