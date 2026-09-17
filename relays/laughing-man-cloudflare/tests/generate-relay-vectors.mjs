// Offline fixture generator, independent of the production TypeScript encoder.
// RFC 8032 public test keys only. Run explicitly when a reviewed wire changes.
import { createHash, createPrivateKey, createPublicKey, sign } from 'node:crypto';
import { writeFileSync } from 'node:fs';
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const makeKey = (seed, membershipId) => {
  const privateKey = createPrivateKey({
    format: 'der',
    type: 'pkcs8',
    key: Buffer.from('302e020100300506032b657004220420' + seed, 'hex'),
  });
  const bytes = createPublicKey(privateKey).export({ format: 'der', type: 'spki' }).subarray(-32);
  return { privateKey, deviceId: hash(bytes), publicKey: bytes.toString('base64'), membershipId };
};
const keys = [
  makeKey(
    '9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60',
    'membership-alpha-0001',
  ),
  makeKey(
    '4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb',
    'membership-bravo-0002',
  ),
].sort((a, b) => a.deviceId.localeCompare(b.deviceId));
const now = 1789488000000,
  group = 'group-fixture-0001',
  boot = 'b'.repeat(64),
  session = 'e'.repeat(64);
const origin = 'https://worker.example',
  instance = ['instance-client-0001', 'instance-server-0001'];
const client = {
  relayVersion: 1,
  peerVersion: 4,
  origin,
  group,
  from: keys[0].deviceId,
  to: keys[1].deviceId,
  fromMembership: keys[0].membershipId,
  toMembership: keys[1].membershipId,
  fromInstance: instance[0],
  toInstance: instance[1],
  role: 'client',
  wake: 'a'.repeat(64),
  boot,
  nonce: 'session-nonce-fixture-0001',
  issuedAt: now,
  expiresAt: now + 120000,
  certificateSha256: 'c'.repeat(64),
  replyTo: null,
};
const tlsMessage = (s) =>
  JSON.stringify([
    'kota-bbs-relay.tls.v1',
    s.relayVersion,
    s.peerVersion,
    s.origin,
    s.group,
    s.from,
    s.to,
    s.fromMembership,
    s.toMembership,
    s.fromInstance,
    s.toInstance,
    s.role,
    s.wake,
    s.boot,
    s.nonce,
    String(s.issuedAt),
    String(s.expiresAt),
    s.certificateSha256,
    s.replyTo,
  ]);
const server = {
  ...client,
  from: client.to,
  to: client.from,
  fromMembership: client.toMembership,
  toMembership: client.fromMembership,
  fromInstance: client.toInstance,
  toInstance: client.fromInstance,
  role: 'server',
  certificateSha256: 'd'.repeat(64),
  replyTo: hash(tlsMessage(client)),
};
const signedTls = (statement, actor) => ({
  statement,
  signature: sign(null, Buffer.from(tlsMessage(statement)), actor.privateKey).toString('base64'),
});
const declarations = { client: signedTls(client, keys[0]), server: signedTls(server, keys[1]) };
const cases = [
  ['poll', 'read', 0, null, ''],
  ['receive', 'read', 1, null, `&boot=${boot}&mode=probe&cursors=${session}.0`],
  ['receive', 'read', 1, null, `&boot=${boot}&mode=data&cursors=${session}.0`],
  ['receive', 'read', 0, null, `&boot=${boot}&mode=receipts&cursors=${session}.0`],
  [
    'announce',
    'mutation',
    0,
    {
      requestId: 'announce-request-0001',
      previous: null,
      createdAt: now,
      instance: instance[0],
      revision: 'f'.repeat(64),
      peerVersion: 4,
    },
    '',
  ],
  [
    'wake',
    'mutation',
    0,
    {
      requestId: 'wake-request-0000001',
      previous: null,
      createdAt: now,
      instance: instance[0],
      peer: keys[1].deviceId,
      peerInstance: instance[1],
    },
    '',
  ],
  [
    'ready',
    'boot',
    0,
    { wake: client.wake, clientInstance: instance[0], serverInstance: instance[1] },
    '',
  ],
  ['open', 'boot', 0, declarations, ''],
  ['open', 'boot', 0, { client: declarations.client, server: null }, ''],
  ['send', 'frame', 0, Buffer.from('0011aaff7f8001020304', 'hex'), ''],
  ['ack', 'frame', 1, { through: '1', final: false }, ''],
  ['ack', 'frame', 1, { through: '1', final: true }, ''],
];
const requests = cases.map(([route, domain, k, body, query]) => {
  const actor = keys[k],
    read = domain === 'read',
    frame = domain === 'frame',
    method = read ? 'GET' : 'POST';
  const path = `/bbs/relay/${route}?group=${group}${query}`;
  const raw = read
    ? Buffer.alloc(0)
    : Buffer.isBuffer(body)
      ? body
      : Buffer.from(JSON.stringify(body));
  const nonce = domain === 'mutation' ? 'nonce-fixture-0001' : '',
    b = domain === 'boot' || frame ? boot : '';
  const s = frame ? session : '',
    direction = frame ? 'c2s' : '',
    sequence = frame && body.final ? 1 : 0,
    final = frame && body.final === true;
  const message = JSON.stringify([
    `kota-bbs-relay.${domain}.v1`,
    origin,
    method,
    path,
    group,
    actor.deviceId,
    actor.membershipId,
    String(now),
    nonce,
    b,
    s,
    direction,
    String(sequence),
    final,
    hash(raw),
  ]);
  const signature = sign(null, Buffer.from(message), actor.privateKey).toString('base64');
  const headers = {
    'x-kota-relay-version': '1',
    'x-kota-relay-device': actor.deviceId,
    'x-kota-relay-membership': actor.membershipId,
    'x-kota-relay-time': String(now),
    'x-kota-relay-signature': signature,
  };
  if (nonce) headers['x-kota-relay-nonce'] = nonce;
  if (b) headers['x-kota-relay-boot'] = b;
  if (frame)
    Object.assign(headers, {
      'x-kota-relay-session': s,
      'x-kota-relay-direction': direction,
      'x-kota-relay-sequence': String(sequence),
      'x-kota-relay-final': final ? '1' : '0',
    });
  return {
    name:
      route +
      (query.includes('mode=receipts') ? '-receipts' : query.includes('mode=data') ? '-data' : '') +
      (route === 'open' && body.server === null ? '-offer' : '') +
      (final ? '-final' : ''),
    origin,
    path,
    method,
    headers,
    bodyHex: raw.toString('hex'),
    message,
    signature,
  };
});
const members = {
  groupId: group,
  dissolved: false,
  members: Object.fromEntries(keys.map(({ privateKey, ...m }) => [m.deviceId, m])),
};
const receiptPath = `/bbs/relay/receive?group=${group}&boot=${boot}&mode=receipts&cursors=${session}.0`;
const receiveQueryRejections = [
  receiptPath.replace('mode=receipts', 'mode=Receipts'),
  receiptPath.replace('mode=receipts', 'mode=RECEIPTS'),
  receiptPath.replace('mode=receipts', 'mode=receipt'),
  receiptPath.replace('mode=receipts', 'mode=%72eceipts'),
  receiptPath.replace('mode=receipts', 'mode=receipts&mode=receipts'),
  receiptPath.replace(`boot=${boot}&mode=receipts`, `mode=receipts&boot=${boot}`),
  receiptPath.replace(`cursors=${session}.0`, `cursors=${session}.00`),
  receiptPath.replace(`cursors=${session}.0`, `cursors=${session}.0,${session}.1`),
  receiptPath + '&extra=1',
];
writeFileSync(
  new URL('./fixtures/relay-proof-v1.json', import.meta.url),
  JSON.stringify(
    {
      note: 'Public test keys; Node/OpenSSL-generated protocol vectors, not real platform error samples or a TLS handshake.',
      now,
      members,
      requests,
      receiveQueryRejections,
      tls: { declarations, clientMessage: tlsMessage(client), serverMessage: tlsMessage(server) },
    },
    null,
    2,
  ) + '\n',
);
