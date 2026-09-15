import { sha256, signingMessage } from '../src/bbs_auth';
export async function signingKey() {
  const keys = await crypto.subtle.generateKey('Ed25519', true, ['sign', 'verify']);
  const publicBytes = new Uint8Array(await crypto.subtle.exportKey('raw', keys.publicKey));
  return {
    privateKey: keys.privateKey,
    publicKey: btoa(String.fromCharCode(...publicBytes)),
    deviceId: await sha256(publicBytes),
  };
}
export async function signedRequest(
  key: Awaited<ReturnType<typeof signingKey>>,
  path: string,
  body: object,
  groupId: string,
  now = Date.now(),
  nonce = crypto.randomUUID(),
) {
  const raw = JSON.stringify(body);
  const message = signingMessage(
    'https://worker.example',
    'POST',
    path,
    groupId,
    key.deviceId,
    now,
    nonce,
    await sha256(raw),
  );
  const signature = new Uint8Array(
    await crypto.subtle.sign('Ed25519', key.privateKey, new TextEncoder().encode(message)),
  );
  return new Request(`https://worker.example${path}`, {
    method: 'POST',
    body: raw,
    headers: {
      'x-kota-bbs-version': '1',
      'x-kota-bbs-device': key.deviceId,
      'x-kota-bbs-public-key': key.publicKey,
      'x-kota-bbs-time': String(now),
      'x-kota-bbs-nonce': nonce,
      'x-kota-bbs-signature': btoa(String.fromCharCode(...signature)),
      'content-type': 'application/json',
      'cf-connecting-ip': key.deviceId,
    },
  });
}
