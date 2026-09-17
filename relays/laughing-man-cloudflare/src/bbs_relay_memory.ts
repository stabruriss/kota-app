// Synchronous boot-local relay reducer: no peer wait, timers or persistence.
// Worker routing is deliberately absent until the H1 security gate passes.
import { AUTH_WINDOW_MS } from './bbs_auth';
import {
  assertCurrent,
  decimal,
  exactObject,
  relayJson,
  reject,
  RELAY_PAYLOAD_BYTES,
  type Direction,
  type RelayMembers,
  type SignedAck,
  type VerifiedRelayRequest,
} from './bbs_relay_auth';
import { checkOffer, checkSession, checkWake, type SignedTlsStatement, type VerifiedOffer,
  type VerifiedSession, type Wake } from './bbs_relay_session';

export const GROUP_PAYLOAD_BYTES = 16 * 1024 * 1024;
export const MAX_BOOT_RECORDS = 256;
export const MAX_LIVE_SESSIONS = 64;
export const MAX_DEVICE_SESSIONS = 4;
export const DIRECTION_BATCHES = 2;
interface Ready {
  wake: Wake;
  client: boolean;
  server: boolean;
  offer: VerifiedOffer | null;
  answer: SignedTlsStatement | null;
}
interface Batch {
  sequence: number;
  hash: string;
  bytes: Uint8Array<ArrayBuffer>;
}
interface Lane {
  next: number;
  consumed: number;
  batches: Batch[];
  ack: SignedAck | null;
  ackSequence: number;
  ackHash: string;
  closed: boolean;
}
interface Session {
  // Replay/context record only. TLS declarations live in readyRows and are
  // erased on close; this small binding remains through the acceptance window.
  wake: Wake;
  acceptUntil: number;
  c2s: Lane;
  s2c: Lane;
  touchedAt: number;
}
function lane(): Lane {
  return {
    next: 0,
    consumed: 0,
    batches: [],
    ack: null,
    ackSequence: -1,
    ackHash: '',
    closed: false,
  };
}
export class RelayMemory {
  private readonly readyRows = new Map<string, Ready>();
  private readonly sessions = new Map<string, Session>();
  // Boot-local service order, at most four live sessions per current device.
  // Omitted/deferred sessions retain priority; reads still consume no bytes.
  private readonly receiveOrder = new Map<string, string[]>();
  private payloadBytes = 0;
  constructor(readonly boot: string) {}

  private member(input: VerifiedRelayRequest, members: RelayMembers, now: number): void {
    assertCurrent(input, members);
    if (Math.abs(input.proof.time - now) > AUTH_WINDOW_MS) reject('stale_signature', 401);
    if ((input.proof.boot || input.target.boot) !== this.boot) reject('relay_session_lost');
  }
  // Called inside the DO's serialized final-member-check section; never await
  // between this point and enqueuing/returning ciphertext or accepting an ACK.
  sweep(members: RelayMembers, now: number): void {
    for (const [id, row] of this.readyRows) {
      const w = row.wake;
      if (
        w.expiresAt <= now ||
        members.dissolved ||
        w.group !== members.groupId ||
        members.members[w.client]?.membershipId !== w.clientMembership ||
        members.members[w.server]?.membershipId !== w.serverMembership
      )
        this.readyRows.delete(id);
    }
    for (const [id, session] of this.sessions) {
      const w = session.wake;
      const revoked =
        members.dissolved ||
        w.group !== members.groupId ||
        members.members[w.client]?.membershipId !== w.clientMembership ||
        members.members[w.server]?.membershipId !== w.serverMembership;
      // Server memory lease, not content progress. A legitimate probe keeps
      // memory allocated during file_work; the endpoint still enforces its
      // independent no-progress deadline. No timer is scheduled here.
      if (revoked || now - session.touchedAt > 20_000) this.close(session);
      if (session.c2s.closed && session.s2c.closed && session.acceptUntil <= now)
        this.sessions.delete(id);
    }
    for (const [device, order] of this.receiveOrder) {
      const live =
        members.dissolved || !members.members[device]
          ? []
          : order.filter((id) => {
              const session = this.sessions.get(id);
              return session && (!session.c2s.closed || !session.s2c.closed);
            });
      if (live.length) this.receiveOrder.set(device, live);
      else this.receiveOrder.delete(device);
    }
  }
  private close(session: Session): void {
    this.readyRows.delete(session.wake.id);
    for (const direction of ['c2s', 's2c'] as const) {
      const row = session[direction];
      for (const batch of row.batches) this.payloadBytes -= batch.bytes.length;
      row.batches = [];
      row.closed = true;
    }
  }
  ready(input: VerifiedRelayRequest, members: RelayMembers, wake: Wake, now: number) {
    this.member(input, members, now);
    if (input.target.route !== 'ready') return reject('invalid_relay_route');
    this.sweep(members, now);
    checkWake(wake, members, now);
    const body = exactObject(relayJson(input), ['wake', 'clientInstance', 'serverInstance']);
    if (
      body.wake !== wake.id ||
      body.clientInstance !== wake.clientInstance ||
      body.serverInstance !== wake.serverInstance ||
      ![wake.client, wake.server].includes(input.proof.device)
    )
      return reject('relay_wake_changed');
    let row = this.readyRows.get(wake.id);
    if (row && JSON.stringify(row.wake) !== JSON.stringify(wake))
      return reject('relay_wake_changed');
    if (!row) {
      if (this.readyRows.size + this.sessions.size >= MAX_BOOT_RECORDS)
        return reject('relay_backpressure', 429);
      row = { wake: structuredClone(wake), client: false, server: false, offer: null, answer: null };
      this.readyRows.set(wake.id, row);
    }
    if (input.proof.device === wake.client) row.client = true;
    else row.server = true;
    return { boot: this.boot, client: row.client, server: row.server };
  }
  // Replacing a current wake retires all old ready/session states. Closed
  // declaration records remain until expiry, so replay cannot reopen them.
  replaceWake(oldId: string): void {
    this.readyRows.delete(oldId);
    for (const session of this.sessions.values())
      if (session.wake.id === oldId) this.close(session);
  }
  offer(input: VerifiedRelayRequest, members: RelayMembers, wake: Wake, offer: VerifiedOffer, now: number) {
    this.member(input, members, now);
    if (input.target.route !== 'open' || input.proof.device !== wake.client)
      return reject('unauthorized', 403);
    checkOffer(offer, input.target.origin, wake, this.boot, members, now);
    const body = exactObject(relayJson(input), ['client', 'server']);
    if (body.server !== null || JSON.stringify(body.client) !== offer.bytes)
      return reject('invalid_tls_statement');
    this.sweep(members, now);
    for (const session of this.sessions.values())
      if (session.wake.id === wake.id) return reject('relay_wake_used');
    const row = this.readyRows.get(wake.id);
    if (!row?.client || !row.server || JSON.stringify(row.wake) !== JSON.stringify(wake))
      return reject('relay_not_ready');
    if (row.offer && row.offer.bytes !== offer.bytes) return reject('relay_wake_used');
    row.offer ??= offer;
    return { boot: this.boot, offered: true };
  }
  open(
    input: VerifiedRelayRequest,
    members: RelayMembers,
    wake: Wake,
    declaration: VerifiedSession,
    now: number,
  ) {
    this.member(input, members, now);
    if (input.target.route !== 'open') return reject('invalid_relay_route');
    checkSession(declaration, input.target.origin, wake, this.boot, members, now);
    // Ensures the verified declarations are exactly the signed request body.
    const body = exactObject(relayJson(input), ['client', 'server']);
    if (
      JSON.stringify(body.client) !== JSON.stringify(declaration.client) ||
      JSON.stringify(body.server) !== JSON.stringify(declaration.server) ||
      ![wake.client, wake.server].includes(input.proof.device)
    )
      return reject('invalid_tls_statement');
    this.sweep(members, now);
    const old = this.sessions.get(declaration.id);
    if (old && (old.c2s.closed || old.s2c.closed))
      return { boot: this.boot, session: declaration.id, closed: true };
    if (!old) for (const session of this.sessions.values())
      if (session.wake.id === wake.id) return reject('relay_wake_used');
    const ready = this.readyRows.get(wake.id);
    if (!ready?.client || !ready.server || JSON.stringify(ready.wake) !== JSON.stringify(wake))
      return reject('relay_not_ready');
    if (!ready.offer) return reject('relay_not_ready');
    if (ready.offer.bytes !== JSON.stringify(body.client)) return reject('invalid_tls_statement');
    if (old) return { boot: this.boot, session: declaration.id, closed: false };
    const live = [...this.sessions.values()].filter((s) => !s.c2s.closed || !s.s2c.closed);
    const deviceCount = (device: string) =>
      live.filter((s) => {
        return s.wake.client === device || s.wake.server === device;
      }).length;
    if (
      live.length >= MAX_LIVE_SESSIONS ||
      this.sessions.size + this.readyRows.size >= MAX_BOOT_RECORDS ||
      deviceCount(wake.client) >= MAX_DEVICE_SESSIONS ||
      deviceCount(wake.server) >= MAX_DEVICE_SESSIONS
    )
      return reject('relay_backpressure', 429);
    // One current session per wake. A second declaration cannot bypass closure;
    // it needs a new create-new wake/current-id exchange instead.
    ready.answer = declaration.server;
    this.sessions.set(declaration.id, { wake: structuredClone(wake), acceptUntil: declaration.acceptUntil,
      c2s: lane(), s2c: lane(), touchedAt: now });
    return { boot: this.boot, session: declaration.id, closed: false };
  }
  private session(
    input: VerifiedRelayRequest,
    members: RelayMembers,
    id: string,
    now: number,
  ): Session {
    this.member(input, members, now);
    this.sweep(members, now);
    const session = this.sessions.get(id);
    if (!session) return reject('relay_session_lost');
    const w = session.wake;
    if (
      w.group !== members.groupId ||
      ![w.client, w.server].includes(input.proof.device) ||
      members.members[w.client]?.membershipId !== w.clientMembership ||
      members.members[w.server]?.membershipId !== w.serverMembership
    )
      return reject('unauthorized', 403);
    session.touchedAt = now;
    return session;
  }
  send(input: VerifiedRelayRequest, members: RelayMembers, now: number) {
    if (input.target.route !== 'send') return reject('invalid_relay_route');
    const session = this.session(input, members, input.proof.session, now);
    const dir = input.proof.direction as Direction,
      row = session[dir];
    const sender =
      dir === 'c2s'
        ? session.wake.client
        : session.wake.server;
    if (input.proof.device !== sender) return reject('unauthorized', 403);
    if (row.closed) return reject('relay_session_lost');
    const seq = input.proof.sequence;
    if (seq < row.consumed) return { accepted: false, consumed: true };
    if (seq < row.next) {
      const old = row.batches.find((b) => b.sequence === seq);
      if (!old || old.hash !== input.bodyHash) return reject('relay_sequence_conflict');
      return { accepted: true, consumed: false };
    }
    if (seq !== row.next || seq >= Number.MAX_SAFE_INTEGER) return reject('relay_sequence_gap');
    if (!input.bytes.length) return reject('invalid_relay_payload', 400);
    if (
      row.batches.length >= DIRECTION_BATCHES ||
      this.payloadBytes + input.bytes.length > GROUP_PAYLOAD_BYTES
    )
      return reject('relay_backpressure', 429);
    row.batches.push({ sequence: seq, hash: input.bodyHash, bytes: input.bytes });
    row.next += 1;
    this.payloadBytes += input.bytes.length;
    return { accepted: true, consumed: false };
  }
  ack(input: VerifiedRelayRequest, members: RelayMembers, now: number) {
    if (input.target.route !== 'ack') return reject('invalid_relay_route');
    const session = this.session(input, members, input.proof.session, now);
    const dir = input.proof.direction as Direction,
      row = session[dir];
    const recipient =
      dir === 'c2s'
        ? session.wake.server
        : session.wake.client;
    if (input.proof.device !== recipient) return reject('unauthorized', 403);
    const body = exactObject(relayJson(input), ['through', 'final']);
    const through = decimal(body.through),
      sequence = input.proof.sequence;
    if (
      typeof body.final !== 'boolean' ||
      body.final !== input.proof.final ||
      through > row.next ||
      through < row.consumed
    )
      return reject('invalid_relay_ack');
    if (sequence < row.ackSequence) return { accepted: false };
    if (sequence === row.ackSequence) {
      if (input.bodyHash !== row.ackHash) return reject('relay_sequence_conflict');
      return { accepted: true };
    }
    if (sequence !== row.ackSequence + 1 || row.closed) return reject('relay_sequence_gap');
    row.ack = { proof: { ...input.proof }, body: new TextDecoder().decode(input.bytes) };
    row.ackSequence = sequence;
    row.ackHash = input.bodyHash;
    row.consumed = through;
    row.batches = row.batches.filter((batch) => {
      if (batch.sequence >= through) return true;
      this.payloadBytes -= batch.bytes.length;
      return false;
    });
    if (body.final) {
      this.readyRows.delete(session.wake.id);
      // Cancel may close with an unchanged high-water mark. Unconsumed bytes
      // are discarded, but are never reported as consumed or completed.
      for (const batch of row.batches) this.payloadBytes -= batch.bytes.length;
      row.batches = [];
      row.closed = true;
    }
    return { accepted: true };
  }
  receive(input: VerifiedRelayRequest, members: RelayMembers, now: number) {
    if (input.target.route !== 'receive') return reject('invalid_relay_route');
    // Validate the complete request before changing its data scheduling order.
    const rows = input.target.cursors.map((cursor) => {
      const session = this.session(input, members, cursor.session, now);
      const client = input.proof.device === session.wake.client;
      const incoming = session[client ? 's2c' : 'c2s'];
      const outgoing = session[client ? 'c2s' : 's2c'];
      if (cursor.next < incoming.consumed || cursor.next > incoming.next)
        return reject('invalid_relay_cursor');
      return { cursor, incoming, outgoing, batches: [] as Batch[] };
    });
    let bytes = 0;
    if (input.target.mode === 'data') {
      const order = this.receiveOrder.get(input.proof.device) ?? [];
      for (const row of rows)
        if ((!row.incoming.closed || !row.outgoing.closed) && !order.includes(row.cursor.session))
          order.push(row.cursor.session);
      // Rotate only sessions actually served. A smaller batch from a later
      // session may use spare room, but never skip a batch within one session.
      for (const id of [...order]) {
        const row = rows.find((row) => row.cursor.session === id);
        if (!row) continue;
        for (const batch of row.incoming.batches) {
          if (batch.sequence < row.cursor.next) continue;
          if (bytes + batch.bytes.length > RELAY_PAYLOAD_BYTES) break;
          row.batches.push(batch);
          bytes += batch.bytes.length;
        }
        if (row.batches.length) {
          order.splice(order.indexOf(id), 1);
          order.push(id);
        }
      }
      if (order.length) this.receiveOrder.set(input.proof.device, order);
    }
    // Keep wire items in canonical cursor order, independent of service order.
    const items = rows.map(({ cursor, incoming, outgoing, batches }) => ({
      session: cursor.session,
      next: incoming.next,
      consumed: incoming.consumed,
      closed: incoming.closed || outgoing.closed,
      batches,
      // Probe remains status-only. Receipts forwards the exact peer ACK
      // without fetching payload or consuming/altering any receipt.
      ack: input.target.mode === 'probe' ? null : outgoing.ack,
    }));
    return { boot: this.boot, items, payloadBytes: bytes };
  }
  readyStatus(wake: Wake) {
    const row = this.readyRows.get(wake.id);
    return { boot: this.boot, client: row?.client ?? false, server: row?.server ?? false };
  }
  handshake(input: VerifiedRelayRequest, members: RelayMembers, wake: Wake, now: number) {
    assertCurrent(input, members);
    if (input.target.route !== 'poll' || ![wake.client, wake.server].includes(input.proof.device))
      return reject('unauthorized', 403);
    checkWake(wake, members, now);
    const row = this.readyRows.get(wake.id);
    const pair = [...this.sessions].find(([, session]) => session.wake.id === wake.id);
    const closed = !!pair && (pair[1].c2s.closed || pair[1].s2c.closed || now - pair[1].touchedAt > 20_000);
    // Read-only projection: no sweep, touchedAt update or TTL extension.
    const current = row && JSON.stringify(row.wake) === JSON.stringify(wake) && !closed;
    const client = current && row.offer && row.offer.client.statement.expiresAt > now
      ? row.offer.client : null;
    const answer = row?.answer;
    const server = client && answer && answer.statement.expiresAt > now ? answer : null;
    return { ready: { client: !!current && row.client, server: !!current && row.server },
      client, server, session: pair?.[0] ?? null, closed };
  }
  stats() {
    return {
      payloadBytes: this.payloadBytes,
      ready: this.readyRows.size,
      sessions: this.sessions.size,
    };
  }
}
