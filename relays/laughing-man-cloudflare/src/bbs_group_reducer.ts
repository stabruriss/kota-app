// Pure control-plane state transitions. Authentication and persistence live in bbs_group.ts.
export const BBS_PROTOCOL_VERSION = 1;
export const INVITE_TTL_MS = 15 * 60_000;
export const RECEIPT_TTL_MS = 24 * 60 * 60_000;
export const MAX_RECEIPTS = 200;
export const MAX_MEMBERS = 32;
export const ONLINE_MS = 90_000;

export interface Member {
  deviceId: string;
  publicKey: string;
  name: string;
  role: 'owner' | 'member';
  membershipId: string;
  lastSeenAt: number;
  joinFingerprint?: string;
}
export interface Invite {
  gen: number;
  hash: string | null;
  expiresAt: number;
}
export interface Receipt {
  publicKey: string;
  deviceId: string;
  membershipId: string;
  fingerprint: string;
  response: ControlResponse;
  at: number;
}
export interface GroupReducerState {
  groupId: string;
  owner: string;
  dissolved: boolean;
  memberVersion: number;
  members: Record<string, Member>;
  invite: Invite;
  oldHashes: Array<{ hash: string; result: 'used' | 'expired' | 'replaced' }>;
  receipts: Record<string, Receipt>;
  revokedJoins: Record<string, { publicKey: string; deviceId: string }>;
}
export type Actor =
  | { kind: 'owner' }
  | { kind: 'member'; deviceId: string; publicKey: string }
  | { kind: 'joiner'; deviceId: string; publicKey: string };
export type GroupRequest =
  | { kind: 'invite'; hash: string; gen: number; requestId: string }
  | { kind: 'redeem'; hash: string; name: string; requestId: string }
  | { kind: 'remove'; deviceId: string; requestId: string }
  | { kind: 'dissolve'; requestId: string }
  | { kind: 'leave'; requestId: string }
  | { kind: 'rename'; name: string; requestId: string }
  | { kind: 'heartbeat' }
  | { kind: 'status' };
export interface ControlResponse {
  protocolVersion: number;
  ok: boolean;
  error?: string;
  groupId?: string;
  role?: 'owner' | 'member';
  membershipId?: string;
  memberVersion?: number;
  members?: Array<Member & { online: boolean }>;
  invite?: { gen: number; hasCode: boolean; expiresAt: number };
  signals?: Array<{ id: string; from: string; to: string; payload: string; expiresAt: number }>;
}
export function failure(error: string): ControlResponse {
  return { protocolVersion: BBS_PROTOCOL_VERSION, ok: false, error };
}
export function createGroup(
  groupId: string,
  owner: Omit<Member, 'role' | 'lastSeenAt'>,
  now: number,
): GroupReducerState {
  return {
    groupId,
    owner: owner.deviceId,
    dissolved: false,
    memberVersion: 1,
    members: { [owner.deviceId]: { ...owner, role: 'owner', lastSeenAt: now } },
    invite: { gen: 0, hash: null, expiresAt: 0 },
    oldHashes: [],
    receipts: {},
    revokedJoins: {},
  };
}
export function activeMember(state: GroupReducerState, actor: Actor): Member | undefined {
  if (state.dissolved) return undefined;
  if (actor.kind === 'owner') return state.members[state.owner];
  const member = state.members[actor.deviceId];
  return member?.publicKey === actor.publicKey ? member : undefined;
}
export function status(state: GroupReducerState, owner: boolean, now: number): ControlResponse {
  return {
    protocolVersion: BBS_PROTOCOL_VERSION,
    ok: true,
    groupId: state.groupId,
    memberVersion: state.memberVersion,
    members: Object.values(state.members).map(({ joinFingerprint: _private, ...member }) => ({
      ...member,
      online: now - member.lastSeenAt < ONLINE_MS,
    })),
    ...(owner
      ? { invite: { gen: state.invite.gen, hasCode: state.invite.hash !== null && state.invite.expiresAt > now, expiresAt: state.invite.expiresAt } }
      : {}),
  };
}
function archiveHash(state: GroupReducerState, result: 'used' | 'expired' | 'replaced'): void {
  if (state.invite.hash) state.oldHashes.push({ hash: state.invite.hash, result });
  state.oldHashes = state.oldHashes.slice(-20);
}

export function reduce(
  state: GroupReducerState,
  actor: Actor,
  request: GroupRequest,
  now: number,
): { state: GroupReducerState; response: ControlResponse } {
  const reject = (error: string) => ({ state, response: failure(error) });
  // Revocation is checked before receipts, including when the same key joins again later.
  if (state.dissolved) return reject('removed');
  const member = activeMember(state, actor);
  const ownerOnly = ['invite', 'remove', 'dissolve'].includes(request.kind);
  if (ownerOnly && actor.kind !== 'owner') return reject('owner_required');
  if (request.kind === 'redeem' ? actor.kind !== 'joiner' : !member) return reject('unauthorized');
  const identity = actor.kind === 'owner' ? state.members[state.owner] : actor;
  const fingerprint = JSON.stringify(request);
  const requestId = 'requestId' in request ? request.requestId : undefined;
  if (request.kind === 'redeem') {
    const revoked = state.revokedJoins[request.requestId];
    if (revoked)
      return reject(
        revoked.publicKey === identity.publicKey && revoked.deviceId === identity.deviceId
          ? 'removed'
          : 'request_conflict',
      );
    // Active membership is durable even after the bounded general receipt cache expires.
    const joined = Object.values(state.members).find(
      (candidate) => candidate.membershipId === request.requestId,
    );
    if (joined) {
      if (
        joined.publicKey !== identity.publicKey ||
        joined.deviceId !== identity.deviceId ||
        joined.joinFingerprint !== fingerprint
      )
        return reject('request_conflict');
      return {
        state,
        response: {
          protocolVersion: BBS_PROTOCOL_VERSION,
          ok: true,
          groupId: state.groupId,
          role: 'member',
          membershipId: joined.membershipId,
        },
      };
    }
  }
  const receipt = requestId ? state.receipts[requestId] : undefined;
  if (receipt && now - receipt.at < RECEIPT_TTL_MS) {
    if (receipt.publicKey !== identity.publicKey || receipt.deviceId !== identity.deviceId)
      return reject('request_conflict');
    if (!member || receipt.membershipId !== member.membershipId) return reject('removed');
    if (receipt.fingerprint !== fingerprint) return reject('request_conflict');
    return { state, response: receipt.response };
  }
  const next = structuredClone(state);
  for (const [id, saved] of Object.entries(next.receipts)) {
    if (now - saved.at >= RECEIPT_TTL_MS) delete next.receipts[id];
  }
  let response: ControlResponse = {
    protocolVersion: BBS_PROTOCOL_VERSION,
    ok: true,
    groupId: state.groupId,
  };
  switch (request.kind) {
    case 'invite':
      if (request.gen !== state.invite.gen + 1) return reject('generation_conflict');
      archiveHash(next, state.invite.expiresAt <= now ? 'expired' : 'replaced');
      next.invite = { hash: request.hash, gen: request.gen, expiresAt: now + INVITE_TTL_MS };
      response.invite = { gen: next.invite.gen, hasCode: true, expiresAt: next.invite.expiresAt };
      break;
    case 'redeem': {
      if (state.invite.hash !== request.hash) {
        const old = state.oldHashes.find((item) => item.hash === request.hash);
        return reject(
          old?.result === 'used'
            ? 'invitation_used'
            : old?.result === 'expired'
              ? 'invitation_expired'
              : 'invalid_invitation',
        );
      }
      if (state.invite.expiresAt <= now) return reject('invitation_expired');
      if (member) return reject('already_member');
      if (Object.keys(state.members).length >= MAX_MEMBERS) return reject('group_full');
      const joiner = actor as Extract<Actor, { kind: 'joiner' }>;
      archiveHash(next, 'used');
      next.invite.hash = null;
      next.members[joiner.deviceId] = {
        deviceId: joiner.deviceId,
        publicKey: joiner.publicKey,
        name: request.name,
        role: 'member',
        membershipId: request.requestId,
        lastSeenAt: now,
        joinFingerprint: fingerprint,
      };
      next.memberVersion += 1;
      response.role = 'member';
      response.membershipId = request.requestId;
      break;
    }
    case 'remove':
      if (request.deviceId === state.owner) return reject('cannot_remove_owner');
      const removed = next.members[request.deviceId];
      if (removed)
        next.revokedJoins[removed.membershipId] = {
          publicKey: removed.publicKey,
          deviceId: removed.deviceId,
        };
      delete next.members[request.deviceId];
      next.memberVersion += 1;
      break;
    case 'dissolve':
      next.dissolved = true;
      next.invite = { gen: next.invite.gen, hash: null, expiresAt: 0 };
      next.members = {};
      next.receipts = {};
      next.oldHashes = [];
      break;
    case 'leave':
      if (member!.role === 'owner') return reject('owner_must_dissolve');
      next.revokedJoins[member!.membershipId] = {
        publicKey: member!.publicKey,
        deviceId: member!.deviceId,
      };
      delete next.members[member!.deviceId];
      next.memberVersion += 1;
      break;
    case 'rename':
      next.members[member!.deviceId].name = request.name;
      next.memberVersion += 1;
      break;
    case 'heartbeat':
      next.members[member!.deviceId].lastSeenAt = now;
      response = status(next, actor.kind === 'owner' || member!.role === 'owner', now);
      break;
    case 'status':
      return { state, response: status(state, actor.kind === 'owner' || member!.role === 'owner', now) };
  }
  const current = next.members[identity.deviceId];
  if (requestId && current && !next.dissolved) {
    next.receipts[requestId] = {
      publicKey: identity.publicKey,
      deviceId: identity.deviceId,
      membershipId: current.membershipId,
      fingerprint,
      response: structuredClone(response),
      at: now,
    };
    const oldest = Object.entries(next.receipts).sort((a, b) => a[1].at - b[1].at);
    for (const [id] of oldest.slice(0, Math.max(0, oldest.length - MAX_RECEIPTS)))
      delete next.receipts[id];
  }
  return { state: next, response };
}
