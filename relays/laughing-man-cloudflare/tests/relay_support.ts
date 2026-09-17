import { createHash, createPrivateKey, createPublicKey, sign } from 'node:crypto';
import { authenticateRelay, type RelayMembers } from '../src/bbs_relay_auth';
import {
  tlsSigningMessage,
  verifyOffer,
  verifySession,
  type TlsStatement,
  type Wake,
} from '../src/bbs_relay_session';
import { RelayMemory } from '../src/bbs_relay_memory';

// Public RFC 8032 test seeds. Never used by production; stable independent
// Node/OpenSSL signer for golden vectors and adversarial mutation tests.
export function key(seed: string, membershipId: string) {
  const privateKey = createPrivateKey({
    format: 'der',
    type: 'pkcs8',
    key: Buffer.from('302e020100300506032b657004220420' + seed, 'hex'),
  });
  const publicBytes = createPublicKey(privateKey)
    .export({ format: 'der', type: 'spki' })
    .subarray(-32);
  return {
    privateKey,
    publicKey: publicBytes.toString('base64'),
    deviceId: createHash('sha256').update(publicBytes).digest('hex'),
    membershipId,
  };
}
export const keys = [
  key('9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60', 'membership-alpha-0001'),
  key('4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb', 'membership-bravo-0002'),
].sort((a, b) => a.deviceId.localeCompare(b.deviceId));
export const NOW = 1_789_488_000_000;
export const GROUP = 'group-fixture-0001';
export const BOOT = 'b'.repeat(64);
export const INSTANCE = ['instance-client-0001', 'instance-server-0001'];
export function members(actors = keys): RelayMembers {
  return {
    groupId: GROUP,
    dissolved: false,
    members: Object.fromEntries(
      actors.map((k) => [
        k.deviceId,
        { deviceId: k.deviceId, publicKey: k.publicKey, membershipId: k.membershipId },
      ]),
    ),
  };
}
export function digest(value: string | Uint8Array) {
  return createHash('sha256').update(value).digest('hex');
}
export interface RequestOptions {
  now?: number;
  origin?: string;
  group?: string;
  membership?: string;
  boot?: string;
  session?: string;
  direction?: 'c2s' | 's2c';
  sequence?: number;
  final?: boolean;
  nonce?: string;
  query?: string;
}
export function signed(
  actor: (typeof keys)[number],
  route: string,
  body: unknown = null,
  opts: RequestOptions = {},
) {
  const read = route === 'poll' || route === 'receive',
    method = read ? 'GET' : 'POST';
  const group = opts.group ?? GROUP,
    origin = opts.origin ?? 'https://worker.example';
  const path = `/bbs/relay/${route}${opts.query ?? `?group=${group}`}`;
  const raw =
    body instanceof Uint8Array
      ? Buffer.from(body)
      : read
        ? Buffer.alloc(0)
        : Buffer.from(JSON.stringify(body));
  const mutation = route === 'announce' || route === 'wake',
    frame = route === 'send' || route === 'ack';
  const time = opts.now ?? NOW,
    nonce = mutation ? (opts.nonce ?? 'nonce-fixture-0001') : '';
  const boot = !read && !mutation ? (opts.boot ?? BOOT) : '';
  const session = frame ? (opts.session ?? 'e'.repeat(64)) : '';
  const direction = frame ? (opts.direction ?? 'c2s') : '';
  const sequence = frame ? (opts.sequence ?? 0) : 0,
    final = frame ? (opts.final ?? false) : false;
  const domain = read ? 'read' : mutation ? 'mutation' : frame ? 'frame' : 'boot';
  // Intentionally independent of the production signing encoder.
  const message = JSON.stringify([
    `kota-bbs-relay.${domain}.v1`,
    origin,
    method,
    path,
    group,
    actor.deviceId,
    opts.membership ?? actor.membershipId,
    String(time),
    nonce,
    boot,
    session,
    direction,
    String(sequence),
    final,
    digest(raw),
  ]);
  const signature = sign(null, Buffer.from(message), actor.privateKey).toString('base64');
  const headers: Record<string, string> = {
    'x-kota-relay-version': '1',
    'x-kota-relay-device': actor.deviceId,
    'x-kota-relay-membership': opts.membership ?? actor.membershipId,
    'x-kota-relay-time': String(time),
    'x-kota-relay-signature': signature,
  };
  if (mutation) headers['x-kota-relay-nonce'] = nonce;
  if (boot) headers['x-kota-relay-boot'] = boot;
  if (frame) {
    headers['x-kota-relay-session'] = session;
    headers['x-kota-relay-direction'] = direction;
    headers['x-kota-relay-sequence'] = String(sequence);
    headers['x-kota-relay-final'] = final ? '1' : '0';
  }
  return {
    request: new Request(origin + path, { method, headers, ...(read ? {} : { body: raw }) }),
    message,
    signature,
    bodyHex: raw.toString('hex'),
    headers,
    method,
    path,
    origin,
  };
}
export async function verified(
  actor: (typeof keys)[number],
  route: string,
  body: unknown,
  opts: RequestOptions = {},
  state = members(),
) {
  return authenticateRelay(signed(actor, route, body, opts).request, state, opts.now ?? NOW);
}
export function wake(now = NOW): Wake {
  return {
    id: 'a'.repeat(64),
    group: GROUP,
    client: keys[0].deviceId,
    server: keys[1].deviceId,
    clientMembership: keys[0].membershipId,
    serverMembership: keys[1].membershipId,
    clientInstance: INSTANCE[0],
    serverInstance: INSTANCE[1],
    createdAt: now,
    expiresAt: now + 120_000,
  };
}
export function declaration(w = wake(), boot = BOOT, now = NOW, actors = keys, origin = 'https://worker.example') {
  const clientKey = actors.find((actor) => actor.deviceId === w.client)!;
  const serverKey = actors.find((actor) => actor.deviceId === w.server)!;
  const client: TlsStatement = {
    relayVersion: 1,
    peerVersion: 4,
    origin,
    group: GROUP,
    from: clientKey.deviceId,
    to: serverKey.deviceId,
    fromMembership: clientKey.membershipId,
    toMembership: serverKey.membershipId,
    fromInstance: w.clientInstance,
    toInstance: w.serverInstance,
    role: 'client',
    wake: w.id,
    boot,
    nonce: 'session-nonce-fixture-0001',
    issuedAt: now,
    expiresAt: now + 120_000,
    certificateSha256: 'c'.repeat(64),
    replyTo: null,
  };
  const server: TlsStatement = {
    ...client,
    from: client.to,
    to: client.from,
    fromMembership: client.toMembership,
    toMembership: client.fromMembership,
    fromInstance: client.toInstance,
    toInstance: client.fromInstance,
    role: 'server',
    certificateSha256: 'd'.repeat(64),
    replyTo: digest(tlsSigningMessage(client)),
  };
  const s = (statement: TlsStatement, actor: (typeof keys)[number]) => ({
    statement,
    signature: sign(null, Buffer.from(tlsSigningMessage(statement)), actor.privateKey).toString(
      'base64',
    ),
  });
  return { client: s(client, clientKey), server: s(server, serverKey) };
}
export async function opened(
  memory = new RelayMemory(BOOT),
  w = wake(),
  state = members(),
  actors = keys,
) {
  const ready = { wake: w.id, clientInstance: w.clientInstance, serverInstance: w.serverInstance };
  const participants = actors.filter((actor) => [w.client, w.server].includes(actor.deviceId));
  for (const actor of participants)
    memory.ready(await verified(actor, 'ready', ready, {}, state), state, w, NOW);
  const body = declaration(w, BOOT, NOW, actors);
  memory.offer(await verified(participants[0], 'open', { client: body.client, server: null }, {}, state),
    state, w, await verifyOffer(body.client, 'https://worker.example', w, BOOT, state, NOW), NOW);
  const session = await verifySession(body, 'https://worker.example', w, BOOT, state, NOW);
  const input = await verified(participants[0], 'open', body, {}, state);
  memory.open(input, state, w, session, NOW);
  return { memory, wake: w, state, session, input };
}
export function receiveQuery(
  session: string,
  next = 0,
  mode: 'data' | 'probe' | 'receipts' = 'data',
  boot = BOOT,
) {
  return `?group=${GROUP}&boot=${boot}&mode=${mode}&cursors=${session}.${next}`;
}
export class MetaStore {
  rows = new Map<string, unknown>();
  reads: string[] = [];
  writes: string[] = [];
  alarm: number | null = null;
  async get<T>(key: string): Promise<T | undefined> {
    this.reads.push(key);
    return structuredClone(this.rows.get(key)) as T | undefined;
  }
  async put<T>(key: string, value: T) {
    this.writes.push(key);
    this.rows.set(key, structuredClone(value));
  }
  async getAlarm() {
    this.reads.push('alarm');
    return this.alarm;
  }
  async setAlarm(at: number) {
    this.writes.push('alarm');
    this.alarm = at;
  }
  resetCounts() {
    this.reads = [];
    this.writes = [];
  }
}
