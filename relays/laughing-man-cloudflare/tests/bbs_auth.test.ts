import { describe, expect, it } from 'vitest';
import {
  RateLimiter,
  readBody,
  sha256,
  signingMessage,
  verifyProof,
  AUTH_WINDOW_MS,
} from '../src/bbs_auth';
import { signingKey, signedRequest } from './support';

describe('BBS request authentication', () => {
  it('verifies Ed25519 proof bound to method, path, group, body, and key', async () => {
    const key = await signingKey();
    const body = { protocolVersion: 1, expectedGroupId: 'g' };
    const request = await signedRequest(key, '/bbs/groups/g/status', body, 'g', 1000);
    const raw = JSON.stringify(body);
    expect((await verifyProof(request, raw, 'g', 1000)).publicKey).toBe(key.publicKey);
    for (const [path, group, payload] of [
      ['/bbs/groups/g/leave', 'g', raw],
      ['/bbs/groups/g/status', 'h', raw],
      ['/bbs/groups/g/status', 'g', '{}'],
    ]) {
      const tampered = new Request(`https://worker.example${path}`, {
        method: request.method,
        headers: request.headers,
      });
      await expect(verifyProof(tampered, payload, group, 1000)).rejects.toThrow('unauthorized');
    }
    const otherWorker = new Request('https://other.example/bbs/groups/g/status', {
      method: request.method,
      headers: request.headers,
    });
    await expect(verifyProof(otherWorker, raw, 'g', 1000)).rejects.toThrow('unauthorized');
    await expect(verifyProof(request, raw, 'g', 1001 + AUTH_WINDOW_MS)).rejects.toThrow(
      'stale_signature',
    );
  });
  it('rejects malformed or mismatched keys and incompatible protocol', async () => {
    const key = await signingKey();
    const body = { protocolVersion: 1 };
    const request = await signedRequest(key, '/bbs/join', body, '', 1000);
    request.headers.set('x-kota-bbs-device', 'wrong');
    await expect(verifyProof(request, JSON.stringify(body), '', 1000)).rejects.toThrow(
      'unauthorized',
    );
    request.headers.set('x-kota-bbs-version', '2');
    await expect(verifyProof(request, JSON.stringify(body), '', 1000)).rejects.toThrow(
      'protocol_mismatch',
    );
  });
  it('limits requests by bytes without trusting content-length and never echoes bad input', async () => {
    await expect(
      readBody(new Request('https://x/', { method: 'POST', body: 's'.repeat(100 * 1024) })),
    ).rejects.toThrow('request_too_large');
    await expect(
      readBody(new Request('https://x/', { method: 'POST', body: 'secret-bad-json' })),
    ).rejects.toThrow('invalid_json');
  });
  it('bounds source/route rate counters and expires them', () => {
    const limiter = new RateLimiter();
    expect(limiter.allow('ip', 'join', 0, 1)).toBe(true);
    expect(limiter.allow('ip', 'join', 0, 1)).toBe(false);
    for (let n = 0; n < 1023; n++) expect(limiter.allow(`${n}`, 'join', 0)).toBe(true);
    expect(limiter.allow('last', 'join', 0)).toBe(false);
    expect(limiter.allow('last', 'join', 60_000)).toBe(true);
  });
});

it('accepts the fixed Ed25519 vector shared with the Rust client', async () => {
  const { default: vector } = await import('./fixtures/control-proof-v1.json');
  const request = new Request(`https://worker.example${vector.path}`, {
    method: vector.method,
    headers: {
      'x-kota-bbs-version': '1',
      'x-kota-bbs-device': vector.deviceId,
      'x-kota-bbs-public-key': vector.publicKey,
      'x-kota-bbs-time': String(vector.timestamp),
      'x-kota-bbs-nonce': vector.nonce,
      'x-kota-bbs-signature': vector.signature,
    },
  });
  expect((await verifyProof(request, vector.body, vector.groupId, vector.timestamp)).deviceId).toBe(
    vector.deviceId,
  );
});
