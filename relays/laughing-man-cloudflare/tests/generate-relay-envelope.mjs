// Offline independent wire encoder. No production TypeScript imports or keys.
// The existing RFC 8032 receipt is copied verbatim, not regenerated/re-signed.
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
const proof = JSON.parse(readFileSync(new URL('./fixtures/relay-proof-v1.json', import.meta.url)));
const a = proof.requests.find((r) => r.name === 'ack'), h = a.headers;
const ack = {
  proof: {
    device: h['x-kota-relay-device'], membership: h['x-kota-relay-membership'],
    time: Number(h['x-kota-relay-time']), nonce: '', boot: h['x-kota-relay-boot'],
    session: h['x-kota-relay-session'], direction: h['x-kota-relay-direction'],
    sequence: Number(h['x-kota-relay-sequence']), final: h['x-kota-relay-final'] === '1',
    signature: h['x-kota-relay-signature'],
  },
  body: Buffer.from(a.bodyHex, 'hex').toString('utf8'),
};
const boot = 'b'.repeat(64), session = 'e'.repeat(64);
const item = (session, next = 0, consumed = 0, closed = false, batches = [], receipt = null) =>
  ({ session, next, consumed, closed, batches, ack: receipt });
const batch = (sequence, hex) => ({ sequence, hex });
const cases = [
  { name: 'empty-data', mode: 'data', cursors: [[session, 0]], items: [item(session)] },
  { name: 'probe-high-water', mode: 'probe', cursors: [[session, Number.MAX_SAFE_INTEGER]],
    items: [item(session, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER, true)] },
  { name: 'receipts-only', mode: 'receipts', cursors: [[session, 4]], items: [item(session, 4, 3, false, [], ack)] },
  { name: 'two-record-fragments', mode: 'data', cursors: [[session, 0]],
    items: [item(session, 2, 0, false, [batch(0, '1703030011ff00'), batch(1, '01807f1122334455')], ack)] },
  { name: 'four-sessions', mode: 'data', cursors: ['1', '3', '5', 'e'].map((c) => [c.repeat(64), 0]),
    items: ['1', '3', '5', 'e'].map((c, i) => item(c.repeat(64), 2, 0, false,
      [batch(0, Buffer.from([i, 255, 0, 0, i]).toString('hex')), batch(1, Buffer.from([128, i]).toString('hex'))],
      c === 'e' ? ack : null)) },
];
for (const c of cases) {
  c.boot = boot;
  const chunks = [];
  const metadata = { boot, items: c.items.map((i) => ({
    session: i.session, next: String(i.next), consumed: String(i.consumed), closed: i.closed,
    batches: i.batches.map((b) => {
      const bytes = Buffer.from(b.hex, 'hex'); chunks.push(bytes);
      return { sequence: String(b.sequence), length: bytes.length };
    }), ack: i.ack,
  })) };
  const json = Buffer.from(JSON.stringify(metadata));
  const prefix = Buffer.alloc(8); prefix.write('KBR1', 'ascii'); prefix.writeUInt32BE(json.length, 4);
  const wire = Buffer.concat([prefix, json, ...chunks]);
  c.envelopeHex = wire.toString('hex');
  c.sha256 = createHash('sha256').update(wire).digest('hex');
}
writeFileSync(new URL('./fixtures/relay-envelope-v1.json', import.meta.url), JSON.stringify({
  note: 'Independent Node binary KBR1 encoder. ACK copied verbatim from relay-proof-v1.json; no private production keys.',
  cases,
}, null, 2) + '\n');
