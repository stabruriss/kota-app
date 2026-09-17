import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { encodeReceive, relayReceiveResponse } from '../src/bbs_relay_envelope';
import { RELAY_HEADER_BYTES, RELAY_PAYLOAD_BYTES, type SignedAck } from '../src/bbs_relay_auth';
import type { RelayMemory } from '../src/bbs_relay_memory';
import { GROUP, NOW, digest, keys, opened, receiveQuery, verified } from './relay_support';

const golden = JSON.parse(readFileSync(new URL('./fixtures/relay-envelope-v1.json', import.meta.url), 'utf8'));
type Case = {
  name: string;
  mode: 'data' | 'probe' | 'receipts';
  boot: string;
  cursors: [string, number][];
  envelopeHex: string;
  sha256: string;
  items: {
    session: string;
    next: number;
    consumed: number;
    closed: boolean;
    batches: { sequence: number; hex: string }[];
    ack: SignedAck | null;
  }[];
};
type ReceiveResult = ReturnType<RelayMemory['receive']>;
function encodeInput(c: Case): ReceiveResult {
  let payloadBytes = 0;
  const items = c.items.map((i) => ({
    ...i,
    ack: structuredClone(i.ack),
    batches: i.batches.map((b) => {
      const bytes = new Uint8Array(Buffer.from(b.hex, 'hex'));
      payloadBytes += bytes.length;
      return { sequence: b.sequence, hash: digest(bytes), bytes };
    }),
  }));
  return { boot: c.boot, items, payloadBytes };
}
async function input(c: Case) {
  return verified(keys[0], 'receive', null, {
    query: `?group=${GROUP}&boot=${c.boot}&mode=${c.mode}&cursors=${c.cursors.map(([s, n]) => `${s}.${n}`).join(',')}`,
  });
}
afterEach(() => vi.restoreAllMocks());
describe('finite binary receive envelope', () => {
  it('matches the independent Node golden bytes including KBR1 version and length', async () => {
    for (const c of golden.cases as Case[]) {
      const request = await input(c);
      const out = encodeReceive(request.target, encodeInput(c));
      expect(Buffer.from(out).toString('hex')).toBe(c.envelopeHex);
      expect(digest(out)).toBe(c.sha256);
      expect(Buffer.from(out.subarray(0, 4)).toString('ascii')).toBe('KBR1');
      const length = new DataView(out.buffer).getUint32(4, false);
      expect(length + 8).toBeLessThanOrEqual(RELAY_HEADER_BYTES);
      const decoded = JSON.parse(new TextDecoder().decode(out.subarray(8, 8 + length)));
      for (let i = 0; i < c.items.length; i++) {
        expect(decoded.items[i].ack).toEqual(c.items[i].ack);
      }
    }
  });
  it('encodes real authenticated reducer output as a finite no-store binary Response without timers', async () => {
    const { memory, state, session } = await opened();
    const bytes = new Uint8Array(RELAY_PAYLOAD_BYTES).map((_, i) => i % 251);
    memory.send(await verified(keys[0], 'send', bytes, { session: session.id }), state, NOW);
    const request = await verified(keys[1], 'receive', null, { query: receiveQuery(session.id) });
    const timeout = vi.spyOn(globalThis, 'setTimeout');
    const interval = vi.spyOn(globalThis, 'setInterval');
    const response = relayReceiveResponse(request, memory.receive(request, state, NOW));
    expect(response).not.toBeInstanceOf(Promise);
    expect(response.headers.get('content-type')).toBe('application/octet-stream');
    expect(response.headers.get('cache-control')).toBe('no-store');
    expect(timeout).not.toHaveBeenCalled();
    expect(interval).not.toHaveBeenCalled();
    const wire = new Uint8Array(await response.arrayBuffer());
    const offset = 8 + new DataView(wire.buffer).getUint32(4, false);
    expect(wire.length).toBeLessThanOrEqual(RELAY_PAYLOAD_BYTES + RELAY_HEADER_BYTES);
    expect(Number(response.headers.get('content-length'))).toBe(wire.length);
    expect(wire.subarray(offset)).toEqual(bytes);
    expect(memory.stats().payloadBytes).toBe(RELAY_PAYLOAD_BYTES); // encoding is not ACK
  });
  it('refuses non-prefix batches, wrong sessions/boot/ranges and payload length disagreements', async () => {
    const c = golden.cases.find((c: Case) => c.name === 'two-record-fragments') as Case;
    const request = await input(c);
    const mutations: ((r: ReceiveResult) => void)[] = [
      (r) => { r.boot = 'c'.repeat(64); },
      (r) => { r.items[0].session = 'd'.repeat(64); },
      (r) => { r.items = []; },
      (r) => { r.items[0].batches[0].sequence = 1; },
      (r) => { r.items[0].batches[1].sequence = 0; },
      (r) => { r.items[0].next = 1; },
      (r) => { r.items[0].consumed = 1; },
      (r) => { r.items[0].closed = 'false' as unknown as boolean; },
      (r) => { r.items[0].batches[0].bytes = new Uint8Array(0); },
      (r) => { r.items[0].batches.push(r.items[0].batches[1]); },
      (r) => { r.payloadBytes += 1; },
      (r) => { Object.assign(r.items[0].ack!.proof, { extra: 'not on wire' }); },
    ];
    for (const change of mutations) {
      const result = encodeInput(c);
      change(result);
      expect(() => encodeReceive(request.target, result)).toThrow();
    }
  });
  it('rejects oversized batches/envelope and never smuggles data or ACK through probe', async () => {
    const c = golden.cases.find((c: Case) => c.name === 'two-record-fragments') as Case;
    const request = await input(c);
    const large = encodeInput(c);
    large.items[0].batches[0].bytes = new Uint8Array(RELAY_PAYLOAD_BYTES + 1);
    expect(() => encodeReceive(request.target, large)).toThrow();
    const total = encodeInput(c);
    total.items[0].batches[0].bytes = new Uint8Array(RELAY_PAYLOAD_BYTES);
    expect(() => encodeReceive(request.target, total)).toThrow('relay_response_too_large');
    for (const mode of ['probe', 'receipts'] as const) {
      const req = await input({ ...c, mode });
      expect(() => encodeReceive(req.target, encodeInput(c))).toThrow();
    }
    const receipts = golden.cases.find((c: Case) => c.name === 'receipts-only') as Case;
    const probe = await input({ ...receipts, mode: 'probe' });
    expect(() => encodeReceive(probe.target, encodeInput(receipts))).toThrow();
    const badAck = encodeInput(receipts);
    badAck.items[0].ack!.body = 'x'.repeat(RELAY_HEADER_BYTES);
    const receiptInput = await input(receipts);
    expect(() => encodeReceive(receiptInput.target, badAck)).toThrow();
  });
});
