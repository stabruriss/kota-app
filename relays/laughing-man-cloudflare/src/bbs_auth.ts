import { BBS_PROTOCOL_VERSION } from './bbs_group_reducer';

export const AUTH_WINDOW_MS = 5 * 60_000;
export const MAX_REQUEST_BYTES = 96 * 1024;
export class ControlError extends Error {
  constructor(
    readonly code: string,
    readonly status = 400,
  ) {
    super(code);
  }
}
export function tokenId(value: unknown): string {
  if (typeof value !== 'string' || !/^[A-Za-z0-9_-]{16,80}$/.test(value))
    throw new ControlError('invalid_id');
  return value;
}
export function hashId(value: unknown): string {
  if (typeof value !== 'string' || !/^[a-f0-9]{64}$/.test(value))
    throw new ControlError('invalid_hash');
  return value;
}
export function displayName(value: unknown): string {
  if (
    typeof value !== 'string' ||
    !value.trim() ||
    value.length > 80 ||
    /[\u0000-\u001f\u007f]/.test(value)
  )
    throw new ControlError('invalid_name');
  return value.trim();
}
export function base64Bytes(value: string, length: number): Uint8Array<ArrayBuffer> {
  try {
    const result = Uint8Array.from(atob(value), (char) => char.charCodeAt(0));
    if (result.length !== length || btoa(String.fromCharCode(...result)) !== value)
      throw new Error();
    return result;
  } catch {
    throw new ControlError('invalid_key_or_signature', 401);
  }
}
export async function sha256(value: string | Uint8Array<ArrayBuffer>): Promise<string> {
  const bytes = typeof value === 'string' ? new TextEncoder().encode(value) : value;
  const hash = await crypto.subtle.digest('SHA-256', bytes);
  return Array.from(new Uint8Array(hash), (byte) => byte.toString(16).padStart(2, '0')).join('');
}
export function signingMessage(
  origin: string,
  method: string,
  path: string,
  groupId: string,
  deviceId: string,
  timestamp: number,
  nonce: string,
  bodyHash: string,
): string {
  return [
    'kota-bbs-control.v1',
    origin,
    method,
    path,
    groupId,
    deviceId,
    String(timestamp),
    nonce,
    bodyHash,
  ].join('\n');
}
export interface Proof {
  deviceId: string;
  publicKey: string;
  nonce: string;
  timestamp: number;
}
export async function verifyProof(
  request: Request,
  rawBody: string,
  groupId: string,
  now: number,
): Promise<Proof> {
  if (request.headers.get('x-kota-bbs-version') !== String(BBS_PROTOCOL_VERSION))
    throw new ControlError('protocol_mismatch', 409);
  const publicKey = request.headers.get('x-kota-bbs-public-key') ?? '';
  const key = base64Bytes(publicKey, 32);
  const deviceId = await sha256(key);
  if (deviceId !== request.headers.get('x-kota-bbs-device'))
    throw new ControlError('unauthorized', 401);
  const nonce = tokenId(request.headers.get('x-kota-bbs-nonce'));
  const timestampText = request.headers.get('x-kota-bbs-time') ?? '';
  const timestamp = Number(timestampText);
  if (
    !Number.isSafeInteger(timestamp) ||
    String(timestamp) !== timestampText ||
    Math.abs(now - timestamp) > AUTH_WINDOW_MS
  )
    throw new ControlError('stale_signature', 401);
  const signature = base64Bytes(request.headers.get('x-kota-bbs-signature') ?? '', 64);
  const message = signingMessage(
    new URL(request.url).origin,
    request.method,
    new URL(request.url).pathname,
    groupId,
    deviceId,
    timestamp,
    nonce,
    await sha256(rawBody),
  );
  const imported = await crypto.subtle.importKey('raw', key, { name: 'Ed25519' }, false, [
    'verify',
  ]);
  if (
    !(await crypto.subtle.verify(
      { name: 'Ed25519' },
      imported,
      signature,
      new TextEncoder().encode(message),
    ))
  )
    throw new ControlError('unauthorized', 401);
  return { publicKey, deviceId, nonce, timestamp };
}
export async function readBody(
  request: Request,
): Promise<{ raw: string; body: Record<string, unknown> }> {
  if (Number(request.headers.get('content-length') ?? '0') > MAX_REQUEST_BYTES)
    throw new ControlError('request_too_large', 413);
  const reader = request.body?.getReader();
  const parts: Uint8Array[] = [];
  let length = 0;
  if (reader) {
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        length += value.length;
        if (length > MAX_REQUEST_BYTES) {
          await reader.cancel();
          throw new ControlError('request_too_large', 413);
        }
        parts.push(value);
      }
    } finally {
      reader.releaseLock();
    }
  }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const part of parts) {
    bytes.set(part, offset);
    offset += part.length;
  }
  try {
    const raw = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes);
    const body: unknown = JSON.parse(raw);
    if (!body || typeof body !== 'object' || Array.isArray(body)) throw new Error();
    if ((body as Record<string, unknown>).protocolVersion !== BBS_PROTOCOL_VERSION)
      throw new ControlError('protocol_mismatch', 409);
    return { raw, body: body as Record<string, unknown> };
  } catch (error) {
    if (error instanceof ControlError) throw error;
    throw new ControlError('invalid_json');
  }
}

// Per-isolate nuisance protection; bounded even for a stream of distinct source IPs.
// Normal 60s member heartbeats are far below 180/min. No BBS content enters counters.
export class RateLimiter {
  private readonly counters = new Map<string, { count: number; until: number }>();
  allow(ip: string, route: string, now: number, limit = 180): boolean {
    for (const [key, counter] of this.counters) if (counter.until <= now) this.counters.delete(key);
    const key = `${ip.slice(0, 64)}:${route}`;
    const existing = this.counters.get(key);
    if (!existing && this.counters.size >= 1024) return false;
    const counter = existing ?? { count: 0, until: now + 60_000 };
    counter.count += 1;
    this.counters.set(key, counter);
    return counter.count <= limit;
  }
}
