// Finite binary receive response. No stream, timer, fetch, storage or new route.
import { base64Bytes, hashId, tokenId } from './bbs_auth';
import {
  exactObject,
  decimal,
  integer,
  RELAY_HEADER_BYTES,
  RELAY_PAYLOAD_BYTES,
  reject,
  type RelayTarget,
  type SignedAck,
  type VerifiedRelayRequest,
} from './bbs_relay_auth';
import type { RelayMemory } from './bbs_relay_memory';

export const RELAY_RECEIVE_CONTENT_TYPE = 'application/octet-stream';
const PREFIX_BYTES = 8;
const MAGIC = [0x4b, 0x42, 0x52, 0x31]; // KBR1
type ReceiveResult = ReturnType<RelayMemory['receive']>;

function checkedAck(ack: SignedAck | null, boot: string, session: string): SignedAck | null {
  if (ack === null) return null;
  exactObject(ack, ['proof', 'body']);
  const p = exactObject(ack.proof, [
    'device', 'membership', 'time', 'nonce', 'boot', 'session', 'direction',
    'sequence', 'final', 'signature',
  ]);
  hashId(p.device);
  tokenId(p.membership);
  integer(p.time);
  tokenId(p.boot);
  hashId(p.session);
  integer(p.sequence);
  if (
    p.nonce !== '' ||
    p.boot !== boot ||
    p.session !== session ||
    (p.direction !== 'c2s' && p.direction !== 's2c') ||
    typeof p.final !== 'boolean' ||
    typeof p.signature !== 'string' ||
    typeof ack.body !== 'string' ||
    ack.body.length > 1024 ||
    new TextEncoder().encode(ack.body).length > 1024
  ) {
    return reject('invalid_relay_response');
  }
  base64Bytes(p.signature, 64);
  let body: unknown;
  try {
    body = JSON.parse(ack.body);
  } catch {
    return reject('invalid_relay_response');
  }
  if (JSON.stringify(body) !== ack.body) return reject('invalid_relay_response');
  const fields = exactObject(body, ['through', 'final']);
  decimal(fields.through);
  if (fields.final !== p.final) return reject('invalid_relay_response');
  // Do not reconstruct/re-sign the receipt or treat it as Worker authority.
  return ack;
}

export function encodeReceive(target: RelayTarget, result: ReceiveResult): Uint8Array<ArrayBuffer> {
  if (target.route !== 'receive' || target.cursors.length < 1 || target.cursors.length > 4) {
    return reject('invalid_relay_response');
  }
  tokenId(result.boot);
  if (result.boot !== target.boot || result.items.length !== target.cursors.length) {
    return reject('invalid_relay_response');
  }
  let payloadBytes = 0;
  const bodies: Uint8Array<ArrayBuffer>[] = [];
  const items = result.items.map((item, i) => {
    const cursor = target.cursors[i];
    hashId(item.session);
    integer(item.next);
    integer(item.consumed);
    if (
      item.session !== cursor.session ||
      item.consumed > cursor.next ||
      cursor.next > item.next ||
      typeof item.closed !== 'boolean' ||
      item.batches.length > 2 ||
      (target.mode !== 'data' && item.batches.length !== 0) ||
      (target.mode === 'probe' && item.ack !== null)
    ) {
      return reject('invalid_relay_response');
    }
    const batches = item.batches.map((batch, index) => {
      integer(batch.sequence);
      if (
        batch.sequence !== cursor.next + index ||
        batch.sequence >= item.next ||
        !(batch.bytes instanceof Uint8Array) ||
        batch.bytes.length < 1 ||
        batch.bytes.length > RELAY_PAYLOAD_BYTES
      ) {
        return reject('invalid_relay_response');
      }
      payloadBytes += batch.bytes.length;
      if (payloadBytes > RELAY_PAYLOAD_BYTES) return reject('relay_response_too_large');
      bodies.push(batch.bytes);
      return { sequence: String(batch.sequence), length: batch.bytes.length };
    });
    return {
      session: item.session,
      next: String(item.next),
      consumed: String(item.consumed),
      closed: item.closed,
      batches,
      ack: checkedAck(item.ack, result.boot, item.session),
    };
  });
  if (integer(result.payloadBytes) !== payloadBytes) return reject('invalid_relay_response');
  const metadata = new TextEncoder().encode(JSON.stringify({ boot: result.boot, items }));
  if (PREFIX_BYTES + metadata.length > RELAY_HEADER_BYTES) return reject('relay_response_too_large');
  const bytes = new Uint8Array(PREFIX_BYTES + metadata.length + payloadBytes);
  bytes.set(MAGIC);
  new DataView(bytes.buffer).setUint32(4, metadata.length, false);
  bytes.set(metadata, PREFIX_BYTES);
  let at = PREFIX_BYTES + metadata.length;
  for (const body of bodies) {
    bytes.set(body, at);
    at += body.length;
  }
  return bytes;
}

export function relayReceiveResponse(input: VerifiedRelayRequest, result: ReceiveResult): Response {
  const bytes = encodeReceive(input.target, result);
  return new Response(bytes, {
    status: 200,
    headers: {
      'content-type': RELAY_RECEIVE_CONTENT_TYPE,
      'content-length': String(bytes.length),
      'cache-control': 'no-store',
    },
  });
}
