import { hashId, sha256, tokenId } from './bbs_auth';
import {
  canonicalOrigin,
  exactObject,
  integer,
  PEER_VERSION,
  reject,
  verifyEd25519,
  type RelayMembers,
} from './bbs_relay_auth';

export const DECLARATION_MS = 120_000;
export const MAX_DECLARATION_BYTES = 4096;
export interface Wake {
  id: string;
  group: string;
  client: string;
  server: string;
  clientMembership: string;
  serverMembership: string;
  clientInstance: string;
  serverInstance: string;
  createdAt: number;
  expiresAt: number;
}
export interface TlsStatement {
  relayVersion: 1;
  peerVersion: 4;
  origin: string;
  group: string;
  from: string;
  to: string;
  fromMembership: string;
  toMembership: string;
  fromInstance: string;
  toInstance: string;
  role: 'client' | 'server';
  wake: string;
  boot: string;
  nonce: string;
  issuedAt: number;
  expiresAt: number;
  certificateSha256: string;
  replyTo: string | null;
}
export interface SignedTlsStatement {
  statement: TlsStatement;
  signature: string;
}
export interface VerifiedSession {
  id: string;
  client: SignedTlsStatement;
  server: SignedTlsStatement;
  acceptUntil: number;
}
const verifiedSessions = new WeakSet<object>();
export interface VerifiedOffer {
  client: SignedTlsStatement;
  bytes: string;
}
const verifiedOffers = new WeakSet<object>();
export function tlsSigningMessage(value: TlsStatement): string {
  return JSON.stringify([
    'kota-bbs-relay.tls.v1',
    value.relayVersion,
    value.peerVersion,
    value.origin,
    value.group,
    value.from,
    value.to,
    value.fromMembership,
    value.toMembership,
    value.fromInstance,
    value.toInstance,
    value.role,
    value.wake,
    value.boot,
    value.nonce,
    String(value.issuedAt),
    String(value.expiresAt),
    value.certificateSha256,
    value.replyTo,
  ]);
}
function parseStatement(value: unknown): SignedTlsStatement {
  if (new TextEncoder().encode(JSON.stringify(value)).length > MAX_DECLARATION_BYTES)
    return reject('invalid_tls_statement', 413);
  const row = exactObject(value, ['statement', 'signature']);
  const s = exactObject(row.statement, [
    'relayVersion',
    'peerVersion',
    'origin',
    'group',
    'from',
    'to',
    'fromMembership',
    'toMembership',
    'fromInstance',
    'toInstance',
    'role',
    'wake',
    'boot',
    'nonce',
    'issuedAt',
    'expiresAt',
    'certificateSha256',
    'replyTo',
  ]);
  if (s.relayVersion !== 1 || s.peerVersion !== PEER_VERSION) return reject('protocol_mismatch');
  if (s.role !== 'client' && s.role !== 'server') return reject('invalid_tls_statement');
  if (typeof s.origin !== 'string' || typeof row.signature !== 'string')
    return reject('invalid_tls_statement');
  canonicalOrigin(s.origin);
  for (const field of ['from', 'to', 'certificateSha256']) hashId(s[field]);
  for (const field of [
    'group',
    'fromMembership',
    'toMembership',
    'fromInstance',
    'toInstance',
    'wake',
    'boot',
    'nonce',
  ])
    tokenId(s[field]);
  integer(s.issuedAt);
  integer(s.expiresAt);
  if (s.replyTo !== null) hashId(s.replyTo);
  return row as unknown as SignedTlsStatement;
}
export function checkWake(wake: Wake, members: RelayMembers, now: number): void {
  if (
    members.dissolved ||
    members.groupId !== wake.group ||
    wake.client >= wake.server ||
    members.members[wake.client]?.membershipId !== wake.clientMembership ||
    members.members[wake.server]?.membershipId !== wake.serverMembership
  )
    reject('unauthorized', 403);
  if (wake.expiresAt <= now || wake.createdAt > now) reject('relay_wake_expired');
}
export function checkSession(
  session: VerifiedSession,
  origin: string,
  wake: Wake,
  boot: string,
  members: RelayMembers,
  now: number,
): void {
  if (!verifiedSessions.has(session)) reject('unauthorized', 403);
  checkStatements([session.client, session.server], origin, wake, boot, members, now);
}
function checkStatements(
  statements: SignedTlsStatement[], origin: string, wake: Wake, boot: string,
  members: RelayMembers, now: number,
): void {
  checkWake(wake, members, now);
  for (const signed of statements) {
    const s = signed.statement;
    const client = s.role === 'client';
    if (
      s.origin !== origin ||
      s.group !== wake.group ||
      s.wake !== wake.id ||
      s.boot !== boot ||
      s.from !== (client ? wake.client : wake.server) ||
      s.to !== (client ? wake.server : wake.client) ||
      s.fromMembership !== (client ? wake.clientMembership : wake.serverMembership) ||
      s.toMembership !== (client ? wake.serverMembership : wake.clientMembership) ||
      s.fromInstance !== (client ? wake.clientInstance : wake.serverInstance) ||
      s.toInstance !== (client ? wake.serverInstance : wake.clientInstance) ||
      s.issuedAt > now ||
      s.expiresAt <= now ||
      s.expiresAt - s.issuedAt > DECLARATION_MS ||
      s.expiresAt <= s.issuedAt
    )
      reject('invalid_tls_statement');
  }
}
export function checkOffer(
  offer: VerifiedOffer, origin: string, wake: Wake, boot: string,
  members: RelayMembers, now: number,
): void {
  if (!verifiedOffers.has(offer) || JSON.stringify(offer.client) !== offer.bytes)
    reject('unauthorized', 403);
  checkStatements([offer.client], origin, wake, boot, members, now);
}
export async function verifyOffer(
  value: unknown, origin: string, wake: Wake, boot: string,
  members: RelayMembers, now: number,
): Promise<VerifiedOffer> {
  const client = parseStatement(value);
  if (client.statement.role !== 'client' || client.statement.replyTo !== null)
    return reject('invalid_tls_statement');
  checkStatements([client], origin, wake, boot, members, now);
  await verifyEd25519(members.members[wake.client].publicKey, client.signature,
    tlsSigningMessage(client.statement));
  const offer = { client, bytes: JSON.stringify(client) };
  verifiedOffers.add(offer);
  return offer;
}
export async function verifySession(
  value: unknown,
  origin: string,
  wake: Wake,
  boot: string,
  members: RelayMembers,
  now: number,
): Promise<VerifiedSession> {
  const row = exactObject(value, ['client', 'server']);
  const client = parseStatement(row.client),
    server = parseStatement(row.server);
  if (
    client.statement.role !== 'client' ||
    server.statement.role !== 'server' ||
    client.statement.replyTo !== null ||
    client.statement.nonce !== server.statement.nonce
  )
    return reject('invalid_tls_statement');
  const clientHash = await sha256(tlsSigningMessage(client.statement));
  if (server.statement.replyTo !== clientHash) return reject('invalid_tls_statement');
  const session = {
    id: await sha256(
      JSON.stringify([
        'kota-bbs-relay.session-id.v1',
        clientHash,
        await sha256(tlsSigningMessage(server.statement)),
      ]),
    ),
    client,
    server,
    acceptUntil: Math.min(client.statement.expiresAt, server.statement.expiresAt),
  };
  checkStatements([client, server], origin, wake, boot, members, now);
  for (const signed of [client, server]) {
    const member = members.members[signed.statement.from];
    await verifyEd25519(member.publicKey, signed.signature, tlsSigningMessage(signed.statement));
  }
  verifiedSessions.add(session);
  // The caller MUST recheck current membership/wake after these async verifies.
  return session;
}
