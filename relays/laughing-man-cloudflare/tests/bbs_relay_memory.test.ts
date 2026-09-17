import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  RelayMemory,
  DIRECTION_BATCHES,
  GROUP_PAYLOAD_BYTES,
  MAX_BOOT_RECORDS,
} from '../src/bbs_relay_memory';
import { RELAY_HEADER_BYTES, verifyRecipientAck } from '../src/bbs_relay_auth';
import { verifyOffer, verifySession } from '../src/bbs_relay_session';
import {
  BOOT,
  GROUP,
  keys,
  key,
  digest,
  NOW,
  members,
  opened,
  verified,
  receiveQuery,
  wake,
  declaration,
} from './relay_support';

afterEach(() => vi.restoreAllMocks());
describe('immediate boot-local relay transitions', () => {
  it('empty data/probe/receipts return synchronously without timers, waiting or SQL', async () => {
    const timer = vi.spyOn(globalThis, 'setTimeout');
    const interval = vi.spyOn(globalThis, 'setInterval');
    // Includes both ready and open; neither can schedule peer waiting.
    const { memory, state, session } = await opened();
    for (const mode of ['data', 'probe', 'receipts'] as const) {
      const input = await verified(keys[1], 'receive', null, {
        query: receiveQuery(session.id, 0, mode),
      });
      const out = memory.receive(input, state, NOW);
      expect(out).not.toBeInstanceOf(Promise);
      expect(out.payloadBytes).toBe(0);
      expect(out.items[0].batches).toEqual([]);
    }
    const data = await verified(keys[0], 'send', new Uint8Array([1]), { session: session.id });
    expect(memory.send(data, state, NOW)).not.toBeInstanceOf(Promise);
    const ack = await verified(keys[1], 'ack', { through: '1', final: false }, { session: session.id });
    expect(memory.ack(ack, state, NOW)).not.toBeInstanceOf(Promise);
    expect(timer).not.toHaveBeenCalled();
    expect(interval).not.toHaveBeenCalled();
  });
  it('requires both ready, pins all statement context and rejects role reflection or wrong signatures', async () => {
    const mem = new RelayMemory(BOOT),
      state = members(),
      w = wake(),
      body = declaration(w);
    const d = await verifySession(body, 'https://worker.example', w, BOOT, state, NOW);
    const input = await verified(keys[0], 'open', body);
    expect(() => mem.open(input, state, w, d, NOW)).toThrow('relay_not_ready');
    for (const mutate of [
      (b: typeof body) => (b.client.statement.role = 'server' as const),
      (b: typeof body) => (b.server.statement.replyTo = '0'.repeat(64)),
      (b: typeof body) => (b.server.statement.certificateSha256 = '0'.repeat(64)),
      (b: typeof body) => (b.client.statement.fromMembership = 'old-membership-0001'),
      (b: typeof body) => (b.server.signature = b.client.signature),
    ]) {
      const changed = structuredClone(body);
      mutate(changed);
      await expect(
        verifySession(changed, 'https://worker.example', w, BOOT, state, NOW),
      ).rejects.toThrow();
    }
    await expect(
      verifySession(body, 'https://other.example', w, BOOT, state, NOW),
    ).rejects.toThrow();
    await expect(
      verifySession(body, 'https://worker.example', w, 'e'.repeat(64), state, NOW),
    ).rejects.toThrow();
    await expect(
      verifySession(body, 'https://worker.example', w, BOOT, state, NOW + 120_000),
    ).rejects.toThrow();
  });
  it('retains exactly two same-ciphertext batches; cursor reads do not consume or release credit', async () => {
    const { memory, state, session } = await opened(),
      data = new Uint8Array(256 * 1024).fill(42);
    for (let seq = 0; seq < DIRECTION_BATCHES; seq++) {
      const input = await verified(keys[0], 'send', data, { session: session.id, sequence: seq });
      expect(memory.send(input, state, NOW).accepted).toBe(true);
      expect(memory.send(input, state, NOW).accepted).toBe(true);
    }
    expect(memory.stats().payloadBytes).toBe(512 * 1024);
    const third = await verified(keys[0], 'send', data, { session: session.id, sequence: 2 });
    expect(() => memory.send(third, state, NOW)).toThrow('relay_backpressure');
    const input = await verified(keys[1], 'receive', null, { query: receiveQuery(session.id) });
    const first = memory.receive(input, state, NOW);
    const repeat = memory.receive(input, state, NOW);
    expect(repeat).toEqual(first);
    expect(first.items[0].batches[0].bytes).toEqual(data);
    expect(first.payloadBytes).toBe(256 * 1024);
    expect(memory.stats().payloadBytes).toBe(512 * 1024);
    const changed = await verified(keys[0], 'send', new Uint8Array([43]), { session: session.id });
    expect(() => memory.send(changed, state, NOW)).toThrow('relay_sequence_conflict');
    const gap = await verified(keys[0], 'send', data, { session: session.id, sequence: 4 });
    expect(() => memory.send(gap, state, NOW)).toThrow('relay_sequence_gap');
    const reflection = await verified(keys[1], 'send', data, { session: session.id });
    expect(() => memory.send(reflection, state, NOW)).toThrow('unauthorized');
  });
  it('only signed recipient ACK releases a sent range; sender verifies original receipt', async () => {
    const { memory, state, session } = await opened();
    memory.send(
      await verified(keys[0], 'send', new Uint8Array([1, 2, 3]), { session: session.id }),
      state,
      NOW,
    );
    const over = await verified(
      keys[1],
      'ack',
      { through: '2', final: false },
      { session: session.id },
    );
    expect(() => memory.ack(over, state, NOW)).toThrow('invalid_relay_ack');
    const wrong = await verified(
      keys[0],
      'ack',
      { through: '1', final: false },
      { session: session.id },
    );
    expect(() => memory.ack(wrong, state, NOW)).toThrow('unauthorized');
    const ack = await verified(
      keys[1],
      'ack',
      { through: '1', final: false },
      { session: session.id },
    );
    memory.ack(ack, state, NOW);
    memory.ack(ack, state, NOW);
    expect(memory.stats().payloadBytes).toBe(0);
    const recv = await verified(keys[0], 'receive', null, { query: receiveQuery(session.id) });
    const receipt = memory.receive(recv, state, NOW).items[0].ack!;
    expect(receipt.proof.signature).toBe(ack.proof.signature);
    const expected = {
      recipient: keys[1].deviceId,
      boot: BOOT,
      session: session.id,
      direction: 'c2s' as const,
      sent: 1,
    };
    expect((await verifyRecipientAck(receipt, recv.target, state, NOW, expected)).through).toBe(1);
    await expect(
      verifyRecipientAck(
        { ...receipt, body: '{"through":"2","final":false}' },
        recv.target,
        state,
        NOW,
        { ...expected, sent: 2 },
      ),
    ).rejects.toThrow('unauthorized');
    await expect(
      verifyRecipientAck(receipt, recv.target, state, NOW, { ...expected, direction: 's2c' }),
    ).rejects.toThrow('invalid_relay_ack');
    await expect(
      verifyRecipientAck(
        { proof: {} as never, body: '{"through":"1","final":false}' },
        recv.target,
        state,
        NOW,
        expected,
      ),
    ).rejects.toThrow();
  });
  it('receipts forwards four original ACKs without payload, consumption, timers or extra state', async () => {
    const timer = vi.spyOn(globalThis, 'setTimeout');
    const interval = vi.spyOn(globalThis, 'setInterval');
    const memory = new RelayMemory(BOOT),
      state = members();
    const expected = new Map<string, unknown>();
    for (let n = 1; n <= 4; n++) {
      const w = { ...wake(), id: n.toString(16).padStart(64, '0') };
      const { session } = await opened(memory, w, state);
      for (const [actor, direction] of [
        [keys[0], 'c2s'],
        [keys[1], 's2c'],
      ] as const) {
        memory.send(
          await verified(actor, 'send', new Uint8Array(256 * 1024).fill(n), {
            session: session.id,
            direction,
          }),
          state,
          NOW,
        );
      }
      const ack = await verified(
        keys[1],
        'ack',
        { through: '1', final: false },
        { session: session.id },
      );
      memory.ack(ack, state, NOW);
      expected.set(session.id, { proof: ack.proof, body: new TextDecoder().decode(ack.bytes) });
    }
    const cursors = [...expected.keys()]
      .sort()
      .map((id) => `${id}.0`)
      .join(',');
    const before = memory.stats();
    expect(before.payloadBytes).toBe(4 * 256 * 1024); // Opposite-direction data remains pending.
    for (const mode of ['receipts', 'receipts', 'probe', 'data'] as const) {
      const input = await verified(keys[0], 'receive', null, {
        query: `?group=${GROUP}&boot=${BOOT}&mode=${mode}&cursors=${cursors}`,
      });
      const out = memory.receive(input, state, NOW);
      expect(out).not.toBeInstanceOf(Promise);
      expect(out.items).toHaveLength(4);
      expect(out.payloadBytes).toBe(mode === 'data' ? 256 * 1024 : 0);
      for (const item of out.items) {
        expect(item.next).toBe(1);
        expect(item.consumed).toBe(0);
        expect(item.closed).toBe(false);
        if (mode !== 'data') expect(item.batches).toEqual([]);
        if (mode === 'probe') expect(item.ack).toBeNull();
        else {
          expect(item.ack).toEqual(expected.get(item.session));
          expect(
            (await verifyRecipientAck(item.ack!, input.target, state, NOW, {
              recipient: keys[1].deviceId,
              boot: BOOT,
              session: item.session,
              direction: 'c2s',
              sent: 1,
            })).through,
          ).toBe(1);
        }
      }
      if (mode !== 'data')
        expect(new TextEncoder().encode(JSON.stringify(out)).length).toBeLessThanOrEqual(
          RELAY_HEADER_BYTES,
        );
      expect(memory.stats()).toEqual(before);
    }
    expect(timer).not.toHaveBeenCalled();
    expect(interval).not.toHaveBeenCalled();
  });
  it('final loss is idempotent, does not invent consumption, and cannot reopen a closed declaration', async () => {
    const { memory, state, session, wake: w, input } = await opened();
    memory.send(
      await verified(keys[0], 'send', new Uint8Array([1]), { session: session.id }),
      state,
      NOW,
    );
    const ack = await verified(
      keys[1],
      'ack',
      { through: '0', final: true },
      { session: session.id, final: true },
    );
    memory.ack(ack, state, NOW);
    expect(memory.ack(ack, state, NOW)).toEqual({ accepted: true });
    expect(memory.stats().payloadBytes).toBe(0);
    expect(memory.open(input, state, w, session, NOW).closed).toBe(true);
    const recv = await verified(keys[0], 'receive', null, { query: receiveQuery(session.id) });
    expect(JSON.parse(memory.receive(recv, state, NOW).items[0].ack!.body).through).toBe('0');
  });
  it('wake replacement closes old records and rejects a second declaration for the same wake', async () => {
    const { memory, state, session, wake: oldWake, input } = await opened();
    memory.send(
      await verified(keys[0], 'send', new Uint8Array([1]), { session: session.id }),
      state,
      NOW,
    );
    memory.replaceWake(oldWake.id);
    expect(memory.stats().payloadBytes).toBe(0);
    expect(memory.open(input, state, oldWake, session, NOW).closed).toBe(true);
    // Replaying ready cannot erase the closed declaration's protection.
    const ready = {
      wake: oldWake.id,
      clientInstance: oldWake.clientInstance,
      serverInstance: oldWake.serverInstance,
    };
    for (const actor of keys)
      memory.ready(await verified(actor, 'ready', ready), state, oldWake, NOW);
    const changed = declaration(oldWake, BOOT, NOW + 1);
    const changedSession = await verifySession(
      changed,
      'https://worker.example',
      oldWake,
      BOOT,
      state,
      NOW + 1,
    );
    const changedInput = await verified(keys[0], 'open', changed, { now: NOW + 1 });
    expect(() => memory.open(changedInput, state, oldWake, changedSession, NOW + 1)).toThrow(
      'relay_wake_used',
    );
    const fresh = await opened(memory, { ...wake(), id: 'd'.repeat(64) }, state);
    expect(fresh.session.id).not.toBe(session.id);
    const lateReady = await verified(keys[0], 'ready', ready);
    expect(() => memory.ready(lateReady, state, fresh.wake, NOW)).toThrow('relay_wake_changed');
  });
  it('full boot record table backpressures without evicting still-valid closed declarations', async () => {
    const memory = new RelayMemory(BOOT),
      state = members();
    const first = await opened(memory);
    memory.replaceWake(first.wake.id);
    for (let i = 1; i < MAX_BOOT_RECORDS - 1; i++) {
      const w = { ...wake(), id: digest(`closed-wake-${i}`) };
      await opened(memory, w, state);
      memory.replaceWake(w.id);
    }
    const w = { ...wake(), id: digest('next-wake-at-capacity') };
    const body = { wake: w.id, clientInstance: w.clientInstance, serverInstance: w.serverInstance };
    for (const actor of keys) memory.ready(await verified(actor, 'ready', body), state, w, NOW);
    const declarationBody = declaration(w);
    memory.offer(await verified(keys[0], 'open', { client: declarationBody.client, server: null }),
      state, w, await verifyOffer(declarationBody.client, 'https://worker.example', w, BOOT, state, NOW), NOW);
    const session = await verifySession(
      declarationBody,
      'https://worker.example',
      w,
      BOOT,
      state,
      NOW,
    );
    const input = await verified(keys[0], 'open', declarationBody);
    expect(() => memory.open(input, state, w, session, NOW)).toThrow('relay_backpressure');
    expect(memory.stats().sessions + memory.stats().ready).toBe(MAX_BOOT_RECORDS);
    expect(memory.open(first.input, state, first.wake, first.session, NOW).closed).toBe(true);
    memory.sweep(state, NOW + 120_001);
    expect(memory.stats()).toEqual({ payloadBytes: 0, ready: 0, sessions: 0 });
    expect(() => memory.open(first.input, state, first.wake, first.session, NOW + 120_001)).toThrow(
      'relay_wake_expired',
    );
  });
  it('a fifth live session cannot bypass the per-device connection budget', async () => {
    const memory = new RelayMemory(BOOT),
      state = members();
    for (let i = 0; i < 4; i++)
      await opened(memory, { ...wake(), id: digest(`live-wake-${i}`) }, state);
    await expect(
      opened(memory, { ...wake(), id: digest('fifth-live-wake') }, state),
    ).rejects.toThrow('relay_backpressure');
    expect(memory.stats().sessions).toBe(4);
  });
  it('enforces the shared 16 MiB payload window across 32 members without dropping old batches', async () => {
    const actors = Array.from({ length: 32 }, (_, i) =>
      key(digest(`relay-capacity-public-test-${i}`), `membership-capacity-${i}`),
    ).sort((a, b) => a.deviceId.localeCompare(b.deviceId));
    const state = members(actors),
      memory = new RelayMemory(BOOT);
    const data = new Uint8Array(256 * 1024).fill(71);
    const wakeFor = (a: number, b: number) => ({
      ...wake(),
      id: digest(`pair-${a}-${b}`),
      client: actors[a].deviceId,
      server: actors[b].deviceId,
      clientMembership: actors[a].membershipId,
      serverMembership: actors[b].membershipId,
    });
    const sessions = [];
    for (let pair = 0; pair < 16; pair++) {
      const a = pair * 2,
        b = a + 1;
      const openedSession = await opened(memory, wakeFor(a, b), state, actors);
      sessions.push(openedSession.session);
      for (const [sender, direction] of [
        [actors[a], 'c2s'],
        [actors[b], 's2c'],
      ] as const)
        for (let sequence = 0; sequence < 2; sequence++)
          memory.send(
            await verified(
              sender,
              'send',
              data,
              { session: openedSession.session.id, direction, sequence },
              state,
            ),
            state,
            NOW,
          );
    }
    expect(memory.stats().payloadBytes).toBe(GROUP_PAYLOAD_BYTES);
    const extra = await opened(memory, wakeFor(0, 2), state, actors);
    const pending = await verified(actors[0], 'send', data, { session: extra.session.id }, state);
    expect(() => memory.send(pending, state, NOW)).toThrow('relay_backpressure');
    const read = await verified(
      actors[1],
      'receive',
      null,
      { query: receiveQuery(sessions[0].id) },
      state,
    );
    expect(memory.receive(read, state, NOW).items[0].batches[0].bytes).toEqual(data);
    const ack = await verified(
      actors[1],
      'ack',
      { through: '1', final: false },
      { session: sessions[0].id },
      state,
    );
    memory.ack(ack, state, NOW);
    expect(memory.send(pending, state, NOW).accepted).toBe(true);
    expect(memory.stats().payloadBytes).toBe(GROUP_PAYLOAD_BYTES);
  });
  it.each(['handshake', 'half-record', 'half-file', 'before-ack', 'after-ack'])(
    'cold boot at %s never resurrects old session',
    async (stage) => {
      const { memory, state, session, input, wake: w } = await opened();
      if (stage !== 'handshake')
        memory.send(
          await verified(keys[0], 'send', new Uint8Array(stage === 'half-file' ? 16000 : 7), {
            session: session.id,
          }),
          state,
          NOW,
        );
      if (stage === 'after-ack')
        memory.ack(
          await verified(keys[1], 'ack', { through: '1', final: false }, { session: session.id }),
          state,
          NOW,
        );
      const cold = new RelayMemory('e'.repeat(64));
      expect(() => cold.open(input, state, w, session, NOW)).toThrow('relay_session_lost');
      for (const mode of ['data', 'probe', 'receipts'] as const) {
        const oldGet = await verified(keys[1], 'receive', null, {
          query: receiveQuery(session.id, 0, mode),
        });
        expect(() => cold.receive(oldGet, state, NOW)).toThrow('relay_session_lost');
        const guessedNewBoot = await verified(keys[1], 'receive', null, {
          query: receiveQuery(session.id, 0, mode, 'e'.repeat(64)),
        });
        expect(() => cold.receive(guessedNewBoot, state, NOW)).toThrow('relay_session_lost');
      }
      expect(cold.stats()).toEqual({ payloadBytes: 0, ready: 0, sessions: 0 });
    },
  );
  it('membership revoked during signature work is rechecked before reading/sending/acking', async () => {
    const { memory, state, session } = await opened();
    const send = await verified(keys[0], 'send', new Uint8Array([1]), { session: session.id });
    const recv = await verified(keys[1], 'receive', null, { query: receiveQuery(session.id) });
    const receipts = await verified(keys[1], 'receive', null, {
      query: receiveQuery(session.id, 0, 'receipts'),
    });
    const ack = await verified(
      keys[1],
      'ack',
      { through: '0', final: true },
      { session: session.id, final: true },
    );
    delete state.members[keys[0].deviceId];
    expect(() => memory.send(send, state, NOW)).toThrow('unauthorized');
    expect(() => memory.receive(recv, state, NOW)).toThrow('unauthorized');
    expect(() => memory.receive(receipts, state, NOW)).toThrow('unauthorized');
    expect(() => memory.ack(ack, state, NOW)).toThrow('unauthorized');
    expect(memory.stats().payloadBytes).toBe(0);
  });
  it.each(['probe', 'receipts'] as const)(
    '%s preserves memory without consumption; silence retires it without extending expiry',
    async (mode) => {
      const { memory, state, session } = await opened();
      memory.send(
        await verified(keys[0], 'send', new Uint8Array([1]), { session: session.id }),
        state,
        NOW,
      );
      for (let ms = 4_000; ms <= 28_000; ms += 4_000) {
        const probe = await verified(keys[1], 'receive', null, {
          now: NOW + ms,
          query: receiveQuery(session.id, 0, mode),
        });
        const out = memory.receive(probe, state, NOW + ms);
        expect(out.items[0].consumed).toBe(0);
        expect(out.payloadBytes).toBe(0);
        expect(out.items[0].ack).toBeNull();
        expect(out.items[0].closed).toBe(false);
      }
      memory.sweep(state, NOW + 49_000);
      expect(memory.stats().payloadBytes).toBe(0);
      // Receipt protection remains through declaration expiry, not forever.
      expect(memory.stats().sessions).toBe(1);
      memory.sweep(state, NOW + 120_001);
      expect(memory.stats().sessions).toBe(0);
    },
  );
});
