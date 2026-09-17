import { describe, expect, it } from 'vitest';
import { RELAY_PAYLOAD_BYTES } from '../src/bbs_relay_auth';
import { RelayMemory, MAX_DEVICE_SESSIONS } from '../src/bbs_relay_memory';
import { BOOT, GROUP, NOW, digest, key, members, opened, verified, wake } from './relay_support';

// One authenticated recipient and distinct peers, all with real signed ready,
// declaration, send, read and ACK requests. Sort only by the canonical cursor.
async function connected(count: number) {
  const actors = Array.from({ length: count + 1 }, (_, i) =>
    key(digest(`receive-fairness-public-test-${i}`), `membership-receive-${i}`),
  ).sort((a, b) => a.deviceId.localeCompare(b.deviceId));
  const recipient = actors[0],
    state = members(actors),
    memory = new RelayMemory(BOOT);
  const rows = [];
  for (let i = 1; i <= count; i++) {
    const peer = actors[i];
    const w = {
      ...wake(),
      id: digest(`receive-wake-${i}`),
      client: recipient.deviceId,
      server: peer.deviceId,
      clientMembership: recipient.membershipId,
      serverMembership: peer.membershipId,
    };
    const { session } = await opened(memory, w, state, actors);
    rows.push({ id: session.id, peer, wake: w, sent: 0, next: 0, ackSequence: 0 });
  }
  rows.sort((a, b) => a.id.localeCompare(b.id));
  type Row = (typeof rows)[number];
  async function send(row: Row, size: number) {
    const sequence = row.sent++;
    const bytes = new Uint8Array(size).fill(sequence + 1);
    memory.send(
      await verified(
        row.peer,
        'send',
        bytes,
        { session: row.id, direction: 's2c', sequence },
        state,
      ),
      state,
      NOW,
    );
  }
  async function read(selected = rows, mode: 'data' | 'probe' | 'receipts' = 'data') {
    const cursors = [...selected]
      .sort((a, b) => a.id.localeCompare(b.id))
      .map((row) => `${row.id}.${row.next}`)
      .join(',');
    const request = await verified(
      recipient,
      'receive',
      null,
      { query: `?group=${GROUP}&boot=${BOOT}&mode=${mode}&cursors=${cursors}` },
      state,
    );
    return memory.receive(request, state, NOW);
  }
  async function ack(row: Row, through: number) {
    memory.ack(
      await verified(
        recipient,
        'ack',
        { through: String(through), final: false },
        { session: row.id, direction: 's2c', sequence: row.ackSequence++ },
        state,
      ),
      state,
      NOW,
    );
    row.next = through;
  }
  return { memory, state, rows, recipient, send, read, ack };
}

describe('merged receive contiguous selection and bounded fairness', () => {
  it('never skips an oversized next batch to return a later smaller batch', async () => {
    const { memory, rows, send, read } = await connected(2);
    await send(rows[0], 16 * 1024);
    await send(rows[1], RELAY_PAYLOAD_BYTES);
    await send(rows[1], 16 * 1024);
    const held = memory.stats().payloadBytes;
    const first = await read();
    expect(first.items.map((item) => item.batches.map((batch) => batch.sequence))).toEqual([
      [0],
      [],
    ]);
    // A read is not consumption. The deferred session gets the next opportunity
    // even if this is an exact request replay and the first data remains unacked.
    const second = await read();
    expect(second.items.map((item) => item.batches.map((batch) => batch.sequence))).toEqual([
      [],
      [0],
    ]);
    expect(second.payloadBytes).toBe(RELAY_PAYLOAD_BYTES);
    expect(memory.stats().payloadBytes).toBe(held);
    const afterPrefix = await read([{ ...rows[1], next: 1 }]);
    expect(afterPrefix.items[0].batches.map((batch) => batch.sequence)).toEqual([1]);
    expect(memory.stats().payloadBytes).toBe(held);
  });

  it('serves four continuously replenished peers within four reads with canonical output order', async () => {
    const { memory, rows, send, read, ack } = await connected(MAX_DEVICE_SESSIONS);
    for (const row of rows) await send(row, RELAY_PAYLOAD_BYTES);
    const observed: string[] = [];
    for (let round = 0; round < 12; round++) {
      const reply = await read();
      expect(reply.items.map((item) => item.session)).toEqual(rows.map((row) => row.id));
      expect(reply.payloadBytes).toBe(RELAY_PAYLOAD_BYTES);
      expect(memory.stats().payloadBytes).toBe(MAX_DEVICE_SESSIONS * RELAY_PAYLOAD_BYTES);
      const delivered = reply.items.filter((item) => item.batches.length);
      expect(delivered).toHaveLength(1);
      const item = delivered[0],
        row = rows.find((row) => row.id === item.session)!;
      expect(item.batches.map((batch) => batch.sequence)).toEqual([row.next]);
      expect(item.batches[0].bytes[0]).toBe(row.next + 1);
      observed.push(item.session);
      await ack(row, row.next + 1);
      expect(memory.stats().payloadBytes).toBe((MAX_DEVICE_SESSIONS - 1) * RELAY_PAYLOAD_BYTES);
      await send(row, RELAY_PAYLOAD_BYTES);
    }
    expect(observed).toEqual(Array.from({ length: 3 }, () => rows.map((row) => row.id)).flat());
  });

  it('does not let an earlier small stream starve a full batch', async () => {
    const { rows, send, read, ack } = await connected(2);
    await send(rows[0], 16 * 1024);
    await send(rows[1], RELAY_PAYLOAD_BYTES);
    for (let round = 0; round < 8; round++) {
      const reply = await read();
      const selected = reply.items.filter((item) => item.batches.length);
      expect(selected).toHaveLength(1);
      const row = rows[round % 2];
      expect(selected[0].session).toBe(row.id);
      expect(selected[0].batches[0].sequence).toBe(row.next);
      await ack(row, row.next + 1);
      await send(row, round % 2 ? RELAY_PAYLOAD_BYTES : 16 * 1024);
    }
  });

  it('preserves deferred priority across subset reads and status-only reads', async () => {
    const { rows, send, read, ack } = await connected(2);
    for (const row of rows) await send(row, RELAY_PAYLOAD_BYTES);
    expect((await read()).items[0].batches).toHaveLength(1);
    await ack(rows[0], 1);
    // Omitting a deferred session must not erase its place in the next merged
    // read. Nor may ACK/probe polling change the data service order.
    for (let i = 0; i < 3; i++) {
      await send(rows[0], RELAY_PAYLOAD_BYTES);
      expect((await read([rows[0]])).items[0].batches[0].sequence).toBe(rows[0].next);
      await ack(rows[0], rows[0].next + 1);
      for (const mode of ['probe', 'receipts'] as const) {
        const out = await read(rows, mode);
        expect(out.payloadBytes).toBe(0);
        expect(out.items.every((item) => item.batches.length === 0)).toBe(true);
      }
    }
    await send(rows[0], RELAY_PAYLOAD_BYTES);
    expect((await read()).items.map((item) => item.batches.map((batch) => batch.sequence))).toEqual([
      [],
      [0],
    ]);
  });

  it('packs remaining space but leaves every wholly unserved session ahead of served ones', async () => {
    const { rows, send, read } = await connected(4);
    await send(rows[0], 16 * 1024);
    await send(rows[1], RELAY_PAYLOAD_BYTES);
    await send(rows[2], 16 * 1024);
    await send(rows[3], RELAY_PAYLOAD_BYTES);
    const selected = (reply: Awaited<ReturnType<typeof read>>) =>
      reply.items.filter((item) => item.batches.length).map((item) => item.session);
    expect(selected(await read())).toEqual([rows[0].id, rows[2].id]);
    expect(selected(await read())).toEqual([rows[1].id]);
    expect(selected(await read())).toEqual([rows[3].id]);
    expect(selected(await read())).toEqual([rows[0].id, rows[2].id]);
  });

  it('keeps scheduling metadata bounded to current live sessions and drops it at cold boot', async () => {
    const { memory, state, rows, recipient, send, read } = await connected(4);
    // Inspect the resource bound only; fairness above is tested through the real
    // signed API. Do not add a product debug endpoint for boot-local metadata.
    const orders = (memory as unknown as { receiveOrder: Map<string, string[]> }).receiveOrder;
    for (const row of rows) await send(row, RELAY_PAYLOAD_BYTES);
    await read();
    expect(orders.get(recipient.deviceId)).toHaveLength(MAX_DEVICE_SESSIONS);
    memory.replaceWake(rows[0].wake.id);
    memory.sweep(state, NOW);
    expect(orders.get(recipient.deviceId)).toHaveLength(MAX_DEVICE_SESSIONS - 1);
    delete state.members[rows[1].peer.deviceId];
    memory.sweep(state, NOW);
    expect(orders.get(recipient.deviceId)).toHaveLength(MAX_DEVICE_SESSIONS - 2);
    delete state.members[recipient.deviceId];
    memory.sweep(state, NOW);
    expect(orders.size).toBe(0);
    const cold = new RelayMemory('e'.repeat(64));
    expect((cold as unknown as { receiveOrder: Map<string, string[]> }).receiveOrder.size).toBe(0);
  });
});
