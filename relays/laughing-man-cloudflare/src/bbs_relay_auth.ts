// Relay v1 authentication. Not routed by the Worker until the H1 security gate.
// No storage, clock, timer, fetch or nonce consumption is hidden in this module.
import { AUTH_WINDOW_MS, base64Bytes, ControlError, hashId, sha256, tokenId } from './bbs_auth';

export const RELAY_WIRE_VERSION = 1;
export const PEER_VERSION = 4;
export const RELAY_JSON_BYTES = 64 * 1024;
export const RELAY_PAYLOAD_BYTES = 256 * 1024;
export const RELAY_HEADER_BYTES = 8 * 1024;
export type Direction = 'c2s' | 's2c';
export type ReceiveMode = 'data' | 'probe' | 'receipts';
export type RelayRoute =
  | 'poll'
  | 'announce'
  | 'wake'
  | 'ready'
  | 'open'
  | 'send'
  | 'receive'
  | 'ack';
export interface RelayMember {
  deviceId: string;
  publicKey: string;
  membershipId: string;
}
export interface RelayMembers {
  groupId: string;
  dissolved: boolean;
  members: Record<string, RelayMember>;
}
export interface ReceiveCursor {
  session: string;
  next: number;
}
export interface RelayTarget {
  origin: string;
  path: string;
  route: RelayRoute;
  group: string;
  boot: string;
  mode: ReceiveMode;
  after: string;
  cursors: ReceiveCursor[];
}
export function reject(code: string, status = 409): never {
  throw new ControlError(code, status);
}
export function decimal(value: unknown): number {
  if (typeof value !== 'string' || !/^(0|[1-9][0-9]*)$/.test(value))
    return reject('invalid_relay_integer', 400);
  const n = Number(value);
  if (!Number.isSafeInteger(n) || n < 0) return reject('invalid_relay_integer', 400);
  return n;
}
export function integer(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    return reject('invalid_relay_integer', 400);
  return value;
}
export function exactObject(value: unknown, keys: string[]): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    return reject('invalid_relay_json', 400);
  const row = value as Record<string, unknown>;
  if (Object.keys(row).sort().join(',') !== [...keys].sort().join(','))
    return reject('invalid_relay_fields', 400);
  return row;
}
export function direction(value: unknown): Direction {
  if (value !== 'c2s' && value !== 's2c') return reject('invalid_relay_direction', 400);
  return value;
}
export function canonicalOrigin(value: string): string {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return reject('invalid_relay_origin', 400);
  }
  if (url.protocol !== 'https:' || url.origin !== value || url.username || url.password)
    return reject('invalid_relay_origin', 400);
  return value;
}

// Query values use restricted ASCII alphabets, so no percent encoding is needed.
// Exact reconstruction rejects duplicates, reordered keys, escapes, empty keys,
// alternate integer spellings and query fields that would otherwise go unsigned.
export function relayTarget(request: Request): RelayTarget {
  const url = new URL(request.url);
  canonicalOrigin(url.origin);
  if (url.username || url.password || url.hash) return reject('invalid_relay_url', 400);
  const route = url.pathname.slice('/bbs/relay/'.length) as RelayRoute;
  if (
    url.pathname !== `/bbs/relay/${route}` ||
    !['poll', 'announce', 'wake', 'ready', 'open', 'send', 'receive', 'ack'].includes(route)
  )
    return reject('not_found', 404);
  if (request.method !== (route === 'poll' || route === 'receive' ? 'GET' : 'POST'))
    return reject('invalid_relay_method', 405);
  const group = tokenId(url.searchParams.get('group'));
  let query = `?group=${group}`;
  let boot = '',
    after = '';
  let mode: ReceiveMode = 'data';
  const cursors: ReceiveCursor[] = [];
  if (route === 'poll' && url.searchParams.has('after')) {
    after = hashId(url.searchParams.get('after'));
    query += `&after=${after}`;
  }
  if (route === 'receive') {
    boot = tokenId(url.searchParams.get('boot'));
    const rawMode = url.searchParams.get('mode');
    if (rawMode !== 'data' && rawMode !== 'probe' && rawMode !== 'receipts')
      return reject('invalid_relay_query', 400);
    mode = rawMode;
    const raw = url.searchParams.get('cursors') ?? '';
    for (const item of raw.split(',')) {
      const pair = item.split('.');
      if (pair.length !== 2) return reject('invalid_relay_cursor', 400);
      const session = hashId(pair[0]),
        next = decimal(pair[1]);
      if (cursors.length && cursors[cursors.length - 1].session >= session)
        return reject('invalid_relay_cursor', 400);
      cursors.push({ session, next });
    }
    if (!cursors.length || cursors.length > 4) return reject('invalid_relay_cursor', 400);
    query += `&boot=${boot}&mode=${mode}&cursors=${raw}`;
  }
  if (query !== url.search) return reject('invalid_relay_query', 400);
  return {
    origin: url.origin,
    path: url.pathname + query,
    route,
    group,
    boot,
    mode,
    after,
    cursors,
  };
}

export interface RelayProof {
  device: string;
  membership: string;
  time: number;
  nonce: string;
  boot: string;
  session: string;
  direction: Direction | '';
  sequence: number;
  final: boolean;
  signature: string;
}
export interface RelayInput {
  target: RelayTarget;
  method: string;
  proof: RelayProof;
  bytes: Uint8Array<ArrayBuffer>;
  bodyHash: string;
}
const authenticated = new WeakSet<object>();
export type VerifiedRelayRequest = RelayInput & { readonly authenticated: true };

export function memberFor(input: RelayInput, state: RelayMembers): RelayMember {
  const member = state.members[input.proof.device];
  if (
    state.dissolved ||
    state.groupId !== input.target.group ||
    !member ||
    member.membershipId !== input.proof.membership
  )
    return reject('unauthorized', 403);
  return member;
}
export function assertCurrent(input: VerifiedRelayRequest, state: RelayMembers): RelayMember {
  if (!authenticated.has(input)) return reject('unauthorized', 403);
  return memberFor(input, state);
}
function domain(route: RelayRoute): string {
  if (route === 'poll' || route === 'receive') return 'kota-bbs-relay.read.v1';
  if (route === 'announce' || route === 'wake') return 'kota-bbs-relay.mutation.v1';
  if (route === 'ready' || route === 'open') return 'kota-bbs-relay.boot.v1';
  return 'kota-bbs-relay.frame.v1';
}
export function relaySigningMessage(input: RelayInput): string {
  const { target: t, proof: p } = input;
  return JSON.stringify([
    domain(t.route),
    t.origin,
    input.method,
    t.path,
    t.group,
    p.device,
    p.membership,
    String(p.time),
    p.nonce,
    p.boot,
    p.session,
    p.direction,
    String(p.sequence),
    p.final,
    input.bodyHash,
  ]);
}
export async function verifyEd25519(
  publicKey: string,
  signature: string,
  message: string,
): Promise<void> {
  const key = await crypto.subtle.importKey('raw', base64Bytes(publicKey, 32), 'Ed25519', false, [
    'verify',
  ]);
  if (
    !(await crypto.subtle.verify(
      'Ed25519',
      key,
      base64Bytes(signature, 64),
      new TextEncoder().encode(message),
    ))
  )
    reject('unauthorized', 403);
}
export async function readRelayBytes(
  request: Request,
  max: number,
): Promise<Uint8Array<ArrayBuffer>> {
  const announced = request.headers.get('content-length');
  if (announced !== null && decimal(announced) > max) return reject('request_too_large', 413);
  const reader = request.body?.getReader();
  // A list of arbitrary stream chunks would also need a chunk-count budget:
  // one-byte chunks must not create hundreds of thousands of retained objects.
  const buffer = new Uint8Array(max);
  let len = 0;
  if (reader) {
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        if (value.length > max - len) {
          await reader.cancel();
          return reject('request_too_large', 413);
        }
        buffer.set(value, len);
        len += value.length;
      }
    } finally {
      reader.releaseLock();
    }
  }
  // Retained batches own only their charged byte length, not a 256 KiB backing
  // allocation for every tiny request. Temporary decode copy is bounded too.
  return buffer.slice(0, len);
}
export function relayJson(input: RelayInput): unknown {
  try {
    const raw = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(input.bytes);
    const value: unknown = JSON.parse(raw);
    // Exact signed bytes, one interpretation across Rust/JS (also rejects
    // duplicate JSON keys and ambiguous numeric spellings).
    if (JSON.stringify(value) !== raw) return reject('invalid_relay_json', 400);
    return value;
  } catch {
    return reject('invalid_relay_json', 400);
  }
}
export async function readRelayInput(request: Request, now: number): Promise<RelayInput> {
  let headerBytes = 0;
  for (const [key, value] of request.headers)
    headerBytes += new TextEncoder().encode(`${key}: ${value}\r\n`).length;
  if (headerBytes > RELAY_HEADER_BYTES) return reject('request_too_large', 413);
  const target = relayTarget(request);
  const h = request.headers;
  if (h.get('x-kota-relay-version') !== '1') return reject('protocol_mismatch');
  const proof: RelayProof = {
    device: hashId(h.get('x-kota-relay-device')),
    membership: tokenId(h.get('x-kota-relay-membership')),
    time: decimal(h.get('x-kota-relay-time')),
    nonce: '',
    boot: '',
    session: '',
    direction: '',
    sequence: 0,
    final: false,
    signature: h.get('x-kota-relay-signature') ?? '',
  };
  if (Math.abs(now - proof.time) > AUTH_WINDOW_MS) return reject('stale_signature', 401);
  const allowed = new Set(['version', 'device', 'membership', 'time', 'signature']);
  if (target.route === 'announce' || target.route === 'wake') {
    proof.nonce = tokenId(h.get('x-kota-relay-nonce'));
    allowed.add('nonce');
  }
  if (['ready', 'open', 'send', 'ack'].includes(target.route)) {
    proof.boot = tokenId(h.get('x-kota-relay-boot'));
    allowed.add('boot');
  }
  if (target.route === 'send' || target.route === 'ack') {
    proof.session = hashId(h.get('x-kota-relay-session'));
    proof.direction = direction(h.get('x-kota-relay-direction'));
    proof.sequence = decimal(h.get('x-kota-relay-sequence'));
    const final = h.get('x-kota-relay-final');
    if (final !== '0' && final !== '1') return reject('invalid_relay_final', 400);
    proof.final = final === '1';
    if (proof.final && target.route !== 'ack') return reject('invalid_relay_final', 400);
    for (const key of ['session', 'direction', 'sequence', 'final']) allowed.add(key);
  }
  for (const [key] of h)
    if (key.startsWith('x-kota-relay-') && !allowed.has(key.slice(13)))
      return reject('invalid_relay_header', 400);
  base64Bytes(proof.signature, 64);
  const max =
    request.method === 'GET' ? 0 : target.route === 'send' ? RELAY_PAYLOAD_BYTES : RELAY_JSON_BYTES;
  const bytes = await readRelayBytes(request, max);
  return { target, method: request.method, proof, bytes, bodyHash: await sha256(bytes) };
}
export async function authenticateRelay(
  request: Request,
  state: RelayMembers,
  now: number,
): Promise<VerifiedRelayRequest> {
  const input = await readRelayInput(request, now);
  const member = memberFor(input, state);
  await verifyEd25519(member.publicKey, input.proof.signature, relaySigningMessage(input));
  const result = input as VerifiedRelayRequest;
  authenticated.add(result);
  return result;
}

// Returned verbatim by receive. This is a peer-signed receipt, NOT a server ACK.
export interface SignedAck {
  proof: RelayProof;
  body: string;
}
export async function verifyRecipientAck(
  ack: SignedAck,
  target: RelayTarget,
  state: RelayMembers,
  now: number,
  expected: {
    recipient: string;
    boot: string;
    session: string;
    direction: Direction;
    sent: number;
  },
): Promise<{ through: number; sequence: number; final: boolean }> {
  const p = ack.proof;
  exactObject(p, [
    'device',
    'membership',
    'time',
    'nonce',
    'boot',
    'session',
    'direction',
    'sequence',
    'final',
    'signature',
  ]);
  integer(p.time);
  integer(p.sequence);
  if (typeof p.final !== 'boolean') return reject('invalid_relay_ack');
  if (
    !p ||
    p.device !== expected.recipient ||
    p.boot !== expected.boot ||
    p.session !== expected.session ||
    p.direction !== expected.direction ||
    p.nonce !== '' ||
    Math.abs(now - p.time) > AUTH_WINDOW_MS
  )
    return reject('invalid_relay_ack');
  const body = new Uint8Array(new TextEncoder().encode(ack.body));
  if (body.length > 1024) return reject('invalid_relay_ack');
  const input: RelayInput = {
    target: { ...target, path: `/bbs/relay/ack?group=${target.group}`, route: 'ack' },
    method: 'POST',
    proof: p,
    bytes: body,
    bodyHash: await sha256(body),
  };
  const member = memberFor(input, state);
  await verifyEd25519(member.publicKey, p.signature, relaySigningMessage(input));
  const row = exactObject(relayJson(input), ['through', 'final']);
  const through = decimal(row.through);
  if (typeof row.final !== 'boolean' || row.final !== p.final || through > expected.sent)
    return reject('invalid_relay_ack');
  return { through, sequence: integer(p.sequence), final: p.final };
}
