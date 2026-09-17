// Only these small rows are durable. The DO caller supplies a transaction and
// a member state read within it, after async proof verification. No payload,
// ready, TLS declaration, accepted-session or ACK state enters this store.
import { AUTH_WINDOW_MS, hashId, sha256, tokenId } from './bbs_auth';
import {
  assertCurrent,
  exactObject,
  integer,
  PEER_VERSION,
  relayJson,
  reject,
  type RelayMembers,
  type VerifiedRelayRequest,
} from './bbs_relay_auth';
import { type Wake } from './bbs_relay_session';

export const WAKE_MS = 120_000;
export const META_PREFIX = 'relay:';
export interface RelayStorage {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  getAlarm(): Promise<number | null>;
  setAlarm(at: number): Promise<void>;
}
export interface Announcement {
  device: string;
  membership: string;
  requestId: string;
  fingerprint: string;
  instance: string;
  revision: string;
  peerVersion: 4;
}
interface MutationReceipt {
  fingerprint: string;
  expiresAt: number;
  response: MutationResponse;
}
export interface MutationResponse {
  ok: boolean;
  current: string | null;
  changed: boolean;
  replacedWake?: string;
}
export interface PreparedMutation {
  input: VerifiedRelayRequest;
  requestId: string;
  previous: string | null;
  createdAt: number;
  instance: string;
  revision?: string;
  peer?: string;
  peerInstance?: string;
  wakeId?: string;
}
export async function prepareMutation(input: VerifiedRelayRequest): Promise<PreparedMutation> {
  const announce = input.target.route === 'announce';
  if (!announce && input.target.route !== 'wake') return reject('invalid_relay_route');
  const body = exactObject(
    relayJson(input),
    announce
      ? ['requestId', 'previous', 'createdAt', 'instance', 'revision', 'peerVersion']
      : ['requestId', 'previous', 'createdAt', 'instance', 'peer', 'peerInstance'],
  );
  const requestId = tokenId(body.requestId);
  const previous = body.previous === null ? null : tokenId(body.previous);
  const result: PreparedMutation = {
    input,
    requestId,
    previous,
    createdAt: integer(body.createdAt),
    instance: tokenId(body.instance),
  };
  if (announce) {
    if (body.peerVersion !== PEER_VERSION) return reject('protocol_mismatch');
    result.revision = hashId(body.revision);
  } else {
    result.peer = hashId(body.peer);
    result.peerInstance = tokenId(body.peerInstance);
    result.wakeId = await sha256(
      JSON.stringify([
        'kota-bbs-relay.wake-id.v1',
        input.target.group,
        input.proof.device,
        input.proof.membership,
        requestId,
      ]),
    );
  }
  return result;
}
export function pairKey(a: string, b: string): string {
  return `${META_PREFIX}wake:${[a, b].sort().join(':')}`;
}
function currentWake(
  wake: Wake | undefined,
  members: RelayMembers,
  a: string,
  b: string,
): wake is Wake {
  const [client, server] = [a, b].sort();
  return (
    !!wake &&
    wake.group === members.groupId &&
    wake.client === client &&
    wake.server === server &&
    members.members[client]?.membershipId === wake.clientMembership &&
    members.members[server]?.membershipId === wake.serverMembership
  );
}
export async function applyMutation(
  prepared: PreparedMutation,
  members: RelayMembers,
  storage: RelayStorage,
  now: number,
): Promise<MutationResponse> {
  const { input, requestId, previous, createdAt, instance } = prepared;
  assertCurrent(input, members);
  if (Math.abs(now - input.proof.time) > AUTH_WINDOW_MS || createdAt - now > AUTH_WINDOW_MS)
    return reject('stale_signature', 401);
  const actor = input.proof.device;
  const prefix = `${META_PREFIX}${actor}:${input.proof.membership}:`;
  const receiptKey = `${prefix}request:${requestId}`;
  const old = await storage.get<MutationReceipt>(receiptKey);
  if (old && old.expiresAt > now) {
    if (old.fingerprint !== input.bodyHash) return reject('request_conflict');
    return { ...old.response, changed: false, replacedWake: undefined };
  }
  const announceKey = `${META_PREFIX}announcement:${actor}`;
  const announcement =
    input.target.route === 'announce' ? await storage.get<Announcement>(announceKey) : undefined;
  // A durable pending announcement can outlive a network outage or quota day.
  // The latest row is itself its permanent receipt. A freshly signed retry of
  // the same body must succeed after short-lived replay records are collected.
  if (announcement?.membership === input.proof.membership && announcement.requestId === requestId) {
    if (announcement.fingerprint !== input.bodyHash) return reject('request_conflict');
    return { ok: true, current: requestId, changed: false };
  }
  const nonceKey = `${prefix}nonce:${input.proof.nonce}`;
  const nonce = await storage.get<number>(nonceKey);
  if (nonce !== undefined && nonce > now) return reject('replayed_signature');
  let response: MutationResponse;
  if (input.target.route === 'announce') {
    const current =
      announcement?.membership === input.proof.membership ? announcement.requestId : null;
    if (current !== previous) response = { ok: false, current, changed: false };
    else {
      await storage.put<Announcement>(announceKey, {
        device: actor,
        membership: input.proof.membership,
        requestId,
        fingerprint: input.bodyHash,
        instance,
        revision: prepared.revision!,
        peerVersion: 4,
      });
      response = { ok: true, current: requestId, changed: true };
    }
  } else {
    // A signed creation time has the same clock-skew allowance as the proof.
    // A receipt outlives this allowance, so collecting it cannot revive an old
    // intent under a new HTTP proof. The actual wake TTL uses server time.
    if (now - createdAt > AUTH_WINDOW_MS) return reject('relay_wake_expired');
    const peer = prepared.peer!;
    const member = members.members[peer];
    if (!member || actor === peer) return reject('unauthorized', 403);
    const mine = await storage.get<Announcement>(`${META_PREFIX}announcement:${actor}`);
    const theirs = await storage.get<Announcement>(`${META_PREFIX}announcement:${peer}`);
    if (
      mine?.membership !== input.proof.membership ||
      theirs?.membership !== member.membershipId ||
      mine.instance !== instance ||
      theirs.instance !== prepared.peerInstance
    )
      return reject('relay_instance_changed');
    const key = pairKey(actor, peer),
      oldWake = await storage.get<Wake>(key);
    // An expired current ID is still the CAS value. poll returns it as such,
    // without presenting it as a live wake or writing/renewing its TTL.
    const current = currentWake(oldWake, members, actor, peer) ? oldWake.id : null;
    if (current !== previous) response = { ok: false, current, changed: false };
    else {
      const client = actor < peer;
      await storage.put<Wake>(key, {
        id: prepared.wakeId!,
        group: members.groupId,
        client: client ? actor : peer,
        server: client ? peer : actor,
        clientMembership: client ? input.proof.membership : member.membershipId,
        serverMembership: client ? member.membershipId : input.proof.membership,
        clientInstance: client ? instance : prepared.peerInstance!,
        serverInstance: client ? prepared.peerInstance! : instance,
        createdAt: now,
        expiresAt: now + WAKE_MS,
      });
      response = {
        ok: true,
        current: prepared.wakeId!,
        changed: true,
        ...(current ? { replacedWake: current } : {}),
      };
    }
  }
  const expiresAt = Math.max(now, createdAt) + AUTH_WINDOW_MS + 1;
  await storage.put<MutationReceipt>(receiptKey, {
    fingerprint: input.bodyHash,
    expiresAt,
    response,
  });
  await storage.put(nonceKey, input.proof.time + AUTH_WINDOW_MS + 1);
  // H1 DO wiring coalesces this with the existing signal alarm. No data route
  // calls this; cleanup deletes only expired relay nonce/receipt/wake rows.
  const alarm = await storage.getAlarm();
  const earliest = Math.min(expiresAt, input.target.route === 'wake' ? now + WAKE_MS : expiresAt);
  if (alarm === null || earliest < alarm) await storage.setAlarm(earliest);
  return response;
}
export async function pollMetadata(
  input: VerifiedRelayRequest,
  members: RelayMembers,
  storage: RelayStorage,
  now: number,
) {
  assertCurrent(input, members);
  if (input.target.route !== 'poll') return reject('invalid_relay_route');
  if (Math.abs(now - input.proof.time) > AUTH_WINDOW_MS) return reject('stale_signature', 401);
  const ids = Object.keys(members.members)
    .sort()
    .filter((id) => id > input.target.after);
  const page = ids.slice(0, 16),
    items = [];
  for (const id of page) {
    const announcement = await storage.get<Announcement>(`${META_PREFIX}announcement:${id}`);
    const wake =
      id === input.proof.device
        ? undefined
        : await storage.get<Wake>(pairKey(id, input.proof.device));
    const active =
      announcement?.membership === members.members[id].membershipId ? announcement : null;
    const validWake = currentWake(wake, members, input.proof.device, id);
    items.push({
      device: id,
      announcement: active,
      currentWake: validWake ? wake.id : null,
      wake: validWake && wake.expiresAt > now ? wake : null,
    });
  }
  return { items, next: ids.length > page.length ? page[page.length - 1] : null };
}
