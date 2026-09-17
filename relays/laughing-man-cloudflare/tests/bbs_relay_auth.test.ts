import { describe, expect, it } from 'vitest';
import {
  authenticateRelay,
  assertCurrent,
  readRelayBytes,
  relayJson,
  relaySigningMessage,
  RELAY_HEADER_BYTES,
  RELAY_JSON_BYTES,
  RELAY_PAYLOAD_BYTES,
} from '../src/bbs_relay_auth';
import { keys, NOW, GROUP, members, signed, receiveQuery } from './relay_support';
import {
  tlsSigningMessage,
  verifyOffer,
  verifySession,
  type SignedTlsStatement,
} from '../src/bbs_relay_session';
import { BOOT, digest, wake } from './relay_support';

describe('relay signature security gate', () => {
  it('accepts all independently generated fixed vectors byte for byte', async () => {
    const { default: vectors } = await import('./fixtures/relay-proof-v1.json');
    for (const v of vectors.requests) {
      const request = new Request(v.origin + v.path, {
        method: v.method,
        headers: v.headers,
        ...(v.method === 'GET' ? {} : { body: Buffer.from(v.bodyHex, 'hex') }),
      });
      const input = await authenticateRelay(request, vectors.members, vectors.now);
      expect(relaySigningMessage(input)).toBe(v.message);
      expect(input.proof.signature).toBe(v.signature);
    }
    const open = vectors.requests.find((v) => v.name === 'open')!;
    const body = JSON.parse(Buffer.from(open.bodyHex, 'hex').toString()) as {
      client: SignedTlsStatement;
      server: SignedTlsStatement;
    };
    const session = await verifySession(
      body,
      open.origin,
      wake(),
      BOOT,
      vectors.members,
      vectors.now,
    );
    expect(session.client.statement.replyTo).toBeNull();
    expect(session.server.statement.replyTo).toBe(digest(tlsSigningMessage(body.client.statement)));
    expect(session.id).toMatch(/^[0-9a-f]{64}$/);
    const first = vectors.requests.find((v) => v.name === 'open-offer')!;
    const staged = JSON.parse(Buffer.from(first.bodyHex, 'hex').toString());
    const offer = await verifyOffer(staged.client, first.origin, wake(), BOOT, vectors.members, vectors.now);
    expect(staged.server).toBeNull();
    expect(offer.bytes).toBe(JSON.stringify(body.client));
  });
  it('binds origin, method, full query, group, device, membership and time', async () => {
    const v = signed(keys[0], 'poll');
    const changed: Array<[string, Headers]> = [
      ['https://other.example' + v.path, new Headers(v.headers)],
      [v.origin + v.path + '&after=' + keys[1].deviceId, new Headers(v.headers)],
      [v.origin + '/bbs/relay/poll?group=other-group-0001', new Headers(v.headers)],
    ];
    for (const header of ['x-kota-relay-device', 'x-kota-relay-membership', 'x-kota-relay-time']) {
      const h = new Headers(v.headers);
      h.set(
        header,
        header.endsWith('device')
          ? keys[1].deviceId
          : header.endsWith('time')
            ? String(NOW + 1)
            : 'old-membership-0001',
      );
      changed.push([v.origin + v.path, h]);
    }
    for (const [url, headers] of changed)
      await expect(
        authenticateRelay(new Request(url, { headers }), members(), NOW),
      ).rejects.toThrow();
    await expect(authenticateRelay(v.request.clone(), members(), NOW + 300_001)).rejects.toThrow(
      'stale_signature',
    );
    await expect(
      authenticateRelay(
        new Request(v.origin + v.path, { method: 'POST', headers: v.headers }),
        members(),
        NOW,
      ),
    ).rejects.toThrow('invalid_relay_method');
  });
  it('accepts only three canonical receive modes, and signatures cannot switch modes', async () => {
    const { default: vectors } = await import('./fixtures/relay-proof-v1.json');
    for (const path of vectors.receiveQueryRejections) {
      // Sign the malformed spelling too: signature validity cannot bypass
      // canonical query/mode/cursor validation.
      const request = signed(keys[0], 'receive', null, { query: path.slice(path.indexOf('?')) });
      await expect(authenticateRelay(request.request, members(), NOW)).rejects.toThrow();
    }
    for (const mode of ['data', 'probe', 'receipts'] as const) {
      const query = receiveQuery('e'.repeat(64), 0, mode);
      const base = signed(keys[0], 'receive', null, { query });
      const input = await authenticateRelay(base.request, members(), NOW);
      expect(input.target.mode).toBe(mode);
      expect(input.proof.nonce).toBe('');
      for (const other of ['data', 'probe', 'receipts']) {
        if (mode === other) continue;
        await expect(
          authenticateRelay(
            new Request(base.origin + base.path.replace(`mode=${mode}`, `mode=${other}`), {
              headers: base.headers,
            }),
            members(),
            NOW,
          ),
        ).rejects.toThrow('unauthorized');
      }
    }
  });
  it('rejects ambiguous queries, integer encodings, extra domains and revoked proofs', async () => {
    for (const query of [
      `?group=${GROUP}&group=${GROUP}`,
      `?group=${GROUP}&`,
      `?group=%67roup-fixture-0001`,
      `?group=${GROUP}&unknown=x`,
    ]) {
      await expect(
        authenticateRelay(signed(keys[0], 'poll', null, { query }).request, members(), NOW),
      ).rejects.toThrow('invalid_relay_query');
    }
    const v = signed(keys[0], 'poll');
    v.request.headers.set('x-kota-relay-time', '0' + NOW);
    await expect(authenticateRelay(v.request, members(), NOW)).rejects.toThrow(
      'invalid_relay_integer',
    );
    const other = signed(keys[0], 'poll');
    other.request.headers.set('x-kota-relay-nonce', 'nonce-fixture-0001');
    await expect(authenticateRelay(other.request, members(), NOW)).rejects.toThrow(
      'invalid_relay_header',
    );
    const input = await authenticateRelay(signed(keys[0], 'poll').request, members(), NOW);
    const state = members();
    delete state.members[keys[0].deviceId];
    expect(() => assertCurrent(input, state)).toThrow('unauthorized');
    const newMembership = members();
    newMembership.members[keys[0].deviceId].membershipId = 'replacement-membership-0001';
    expect(() => assertCurrent(input, newMembership)).toThrow('unauthorized');
  });
  it('binds every frame field and the exact bytes, without accepting control-domain proofs', async () => {
    const base = signed(keys[0], 'send', new Uint8Array([1, 2, 3]), { session: 'e'.repeat(64) });
    for (const [header, value] of [
      ['x-kota-relay-boot', 'f'.repeat(64)],
      ['x-kota-relay-session', 'f'.repeat(64)],
      ['x-kota-relay-direction', 's2c'],
      ['x-kota-relay-sequence', '1'],
    ]) {
      const h = new Headers(base.headers);
      h.set(header, value);
      await expect(
        authenticateRelay(
          new Request(base.origin + base.path, {
            method: 'POST',
            headers: h,
            body: Buffer.from(base.bodyHex, 'hex'),
          }),
          members(),
          NOW,
        ),
      ).rejects.toThrow('unauthorized');
    }
    await expect(
      authenticateRelay(
        new Request(base.origin + base.path, {
          method: 'POST',
          headers: base.headers,
          body: new Uint8Array([1, 2, 4]),
        }),
        members(),
        NOW,
      ),
    ).rejects.toThrow('unauthorized');
    const wrong = signed(
      keys[0],
      'ack',
      { through: '0', final: false },
      { session: 'e'.repeat(64) },
    );
    wrong.request.headers.set('x-kota-relay-final', '1');
    await expect(authenticateRelay(wrong.request, members(), NOW)).rejects.toThrow('unauthorized');
    const old = await import('./fixtures/control-proof-v1.json');
    base.request.headers.set('x-kota-relay-signature', old.default.signature);
    await expect(authenticateRelay(base.request, members(), NOW)).rejects.toThrow('unauthorized');
  });
  it('enforces byte limits without trusting content length; rejects duplicate JSON keys', async () => {
    for (const n of [RELAY_JSON_BYTES, RELAY_PAYLOAD_BYTES]) {
      const request = new Request('https://worker.example/', {
        method: 'POST',
        body: new Uint8Array(n + 1),
      });
      await expect(readRelayBytes(request, n)).rejects.toThrow('request_too_large');
    }
    const h = signed(keys[0], 'poll');
    h.request.headers.set('x-padding', 'x'.repeat(RELAY_HEADER_BYTES));
    await expect(authenticateRelay(h.request, members(), NOW)).rejects.toThrow('request_too_large');
    const v = signed(keys[0], 'announce', new TextEncoder().encode('{"x":1,"x":2}'));
    const input = await authenticateRelay(v.request, members(), NOW);
    expect(() => relayJson(input)).toThrow('invalid_relay_json');
  });
});
