import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  BbsSyncStatus, BbsSyncMember, BbsSyncCommand, BbsSyncInvitationRequest, BbsSyncInvitationResult,
  BbsSyncJoinRequest, BbsSyncRenameRequest, BbsSyncRemoveRequest,
} from './types/bbs-sync';
import type { BbsSyncView } from './bbs-sync-view';
import { BBS_SYNC_ERROR_DETAILS } from './bbs-sync-errors';

const errorMessages = {
  unavailable: 'BBS device sync requires the Kota runtime.',
  protocol: 'Kota returned an incompatible device sync response.',
  read: 'Could not refresh device sync status.',
  listen: 'Could not listen for device sync updates.',
  invitation: 'Could not prepare an invitation. Please retry.',
  join: 'Could not join this group. Check the invitation and try again.',
  disconnect: 'Could not disconnect from this group. Please retry.',
  rename: 'Could not update the device name. Please retry.',
  remove: 'Could not remove this device. Please retry.',
  start: BBS_SYNC_ERROR_DETAILS.unknown,
  cancel: BBS_SYNC_ERROR_DETAILS.unknown,
  stale_signature: BBS_SYNC_ERROR_DETAILS.stale_signature,
  worker_update_required: BBS_SYNC_ERROR_DETAILS.worker_update_required,
  sync_busy: BBS_SYNC_ERROR_DETAILS.sync_busy,
} as const;

export class BbsSyncClientError extends Error {
  constructor(public readonly code: keyof typeof errorMessages) {
    super(errorMessages[code]);
    this.name = 'BbsSyncClientError';
  }
}

const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === 'string';
const nullableText = (value: unknown): value is string | null => value === null || text(value);
const role = (value: unknown): value is 'owner' | 'member' => value === 'owner' || value === 'member';
const count = (value: unknown): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
const generation = (value: unknown): value is string => text(value) && /^(0|[1-9][0-9]*)$/.test(value);
const invitationState = (value: unknown): value is BbsSyncStatus['invitation'] =>
  value === 'none' || value === 'preparing' || value === 'ready' || value === 'error';
const syncPhase = (value: unknown): value is BbsSyncStatus['sync']['phase'] =>
  value === 'idle' || value === 'connecting' || value === 'syncing' || value === 'partial' || value === 'failed';

/** Validate without coercion and project only public fields. Bad state must not
 * fabricate a connected group or turn malformed counts into successful sync. */
export function parseBbsSyncStatus(value: unknown): BbsSyncStatus {
  const invalid = () => new BbsSyncClientError('protocol');
  if (!record(value) || value.protocolVersion !== 1 || !record(value.device)
    || !record(value.worker) || !record(value.group) || !record(value.sync)) throw invalid();
  const { device, worker, group, sync } = value;
  if (!text(device.id) || !text(device.name) || typeof worker.configured !== 'boolean'
    || typeof worker.canCreateGroup !== 'boolean' || !nullableText(group.id) || !nullableText(group.name)
    || !(group.role === null || role(group.role)) || !Array.isArray(group.members)
    || !invitationState(value.invitation) || !syncPhase(sync.phase)
    || !(value.invitationGeneration === null || generation(value.invitationGeneration))
    || !nullableText(sync.lastSuccessfulAt) || !nullableText(sync.error)
    || !(sync.controlRecoverable === undefined || typeof sync.controlRecoverable === 'boolean')) throw invalid();
  if ((group.id === null && (group.role !== null || group.name !== null || group.members.length !== 0))
    || (group.id !== null && (!group.id || !role(group.role) || !device.id))) throw invalid();
  if (sync.controlRecoverable === true && group.id === null) throw invalid();
  if ((group.role !== 'owner' && value.invitationGeneration !== null)
    || (group.role === 'owner' && value.invitation === 'ready' && value.invitationGeneration === null)) throw invalid();
  if (!((sync.completed === null && sync.total === null)
    || (count(sync.completed) && count(sync.total) && sync.completed <= sync.total))) throw invalid();
  if (sync.lastSuccessfulAt !== null && !Number.isFinite(Date.parse(sync.lastSuccessfulAt))) throw invalid();
  const members: BbsSyncMember[] = [];
  const seen = new Set<string>();
  for (const item of group.members) {
    if (!record(item) || !text(item.id) || !item.id || seen.has(item.id) || !text(item.name)
      || !role(item.role) || typeof item.online !== 'boolean' || !text(item.publicKey)) throw invalid();
    seen.add(item.id);
    members.push({ id: item.id, name: item.name, role: item.role, online: item.online, publicKey: item.publicKey });
  }
  return {
    protocolVersion: 1,
    device: { id: device.id, name: device.name },
    worker: { configured: worker.configured, canCreateGroup: worker.canCreateGroup },
    group: { id: group.id, name: group.name, role: group.role, members },
    invitation: value.invitation,
    invitationGeneration: value.invitationGeneration,
    sync: {
      phase: sync.phase,
      completed: sync.completed as number | null, total: sync.total as number | null,
      lastSuccessfulAt: sync.lastSuccessfulAt, error: sync.error,
      // Old protocol-1 snapshots cannot grant the new recovery capability.
      controlRecoverable: sync.controlRecoverable === true,
    },
  };
}

/** Only bbs_sync_status is invoked here. The backend contract is a memory read,
 * never Worker refresh, scan or start; those are separate explicit actions. */
export async function bbsSyncStatus(): Promise<BbsSyncStatus> {
  if (!isTauri()) throw new BbsSyncClientError('unavailable');
  let payload: unknown;
  try { payload = await invoke('bbs_sync_status'); }
  catch { throw new BbsSyncClientError('read'); }
  return parseBbsSyncStatus(payload);
}

export async function onBbsSyncChanged(changed: () => void): Promise<() => void> {
  if (!isTauri()) throw new BbsSyncClientError('unavailable');
  try {
    // Ignore the hint payload, even if it happens to resemble a status object.
    return await listen('bbs-sync://changed', () => changed());
  } catch { throw new BbsSyncClientError('listen'); }
}

type Mutation = 'join' | 'disconnect' | 'rename' | 'remove' | 'start' | 'cancel';

async function managementRequest<T extends BbsSyncCommand>(action: Mutation | 'invitation', request: T): Promise<unknown> {
  if (!isTauri()) throw new BbsSyncClientError('unavailable');
  try {
    return await invoke(`bbs_sync_${action}`, { request });
  } catch (error) {
    // Do not attach raw errors/cause: a failed join may echo the invitation.
    // Durable recovery and expectedGroupId checks belong to the backend. This
    // client never silently retries a mutation or derives state from its result.
    if (record(error) && Object.keys(error).length === 1 && Object.hasOwn(error, 'code')
      && (error.code === 'stale_signature' || error.code === 'worker_update_required' || error.code === 'sync_busy')) {
      throw new BbsSyncClientError(error.code);
    }
    throw new BbsSyncClientError(action);
  }
}

async function mutate<T extends BbsSyncCommand>(action: Mutation, request: T): Promise<void> {
  const result = await managementRequest(action, request);
  // Tauri Result<(), _> resolves to unit. Do not mistake a structured failure
  // object from an incompatible backend for success.
  if (result !== null && result !== undefined) throw new BbsSyncClientError('protocol');
}

export async function bbsSyncInvitation({ expectedGroupId, refresh }: BbsSyncInvitationRequest): Promise<BbsSyncInvitationResult> {
  const result = await managementRequest('invitation', { expectedGroupId, refresh });
  if (!record(result) || !text(result.groupId) || !result.groupId
    || !generation(result.generation)
    || !text(result.invitation) || !result.invitation.trim() || result.invitation.length > 4096
    || (expectedGroupId !== null && result.groupId !== expectedGroupId)) throw new BbsSyncClientError('protocol');
  return { groupId: result.groupId, generation: result.generation, invitation: result.invitation };
}

export function bbsSyncJoin({ expectedGroupId, invitation }: BbsSyncJoinRequest): Promise<void> {
  return mutate('join', { expectedGroupId, invitation });
}
export function bbsSyncDisconnect({ expectedGroupId }: BbsSyncCommand): Promise<void> {
  return mutate('disconnect', { expectedGroupId });
}
export function bbsSyncRename({ expectedGroupId, name }: BbsSyncRenameRequest): Promise<void> {
  return mutate('rename', { expectedGroupId, name });
}
export function bbsSyncRemove({ expectedGroupId, deviceId }: BbsSyncRemoveRequest): Promise<void> {
  return mutate('remove', { expectedGroupId, deviceId });
}
export function bbsSyncStart({ expectedGroupId }: BbsSyncCommand): Promise<void> {
  return mutate('start', { expectedGroupId });
}
export function bbsSyncCancel({ expectedGroupId }: BbsSyncCommand): Promise<void> {
  return mutate('cancel', { expectedGroupId });
}

export function bbsSyncStatusView(status: BbsSyncStatus): BbsSyncView {
  const group = status.group.id === null || status.group.role === null ? null : {
    id: status.group.id, name: status.group.name || 'BBS group', role: status.group.role,
    members: status.group.members.map(({ id, name, role: memberRole, online }) => ({ id, name, role: memberRole, online })),
  };
  return {
    deviceId: status.device.id, deviceName: status.device.name,
    workerAvailable: status.worker.configured && (status.worker.canCreateGroup || group?.role === 'owner'),
    group,
    invitationGeneration: status.invitationGeneration,
    // A public "ready" flag is not the code itself. Management fetches the
    // full invitation separately, and may overlay it only while open.
    invitation: status.invitation === 'preparing' || status.invitation === 'ready' ? { state: 'preparing' }
      : status.invitation === 'error' ? { state: 'error', message: 'Could not prepare an invitation.' } : { state: 'none' },
    phase: status.sync.phase,
    progress: status.sync.completed === null || status.sync.total === null ? null : { completed: status.sync.completed, total: status.sync.total },
    lastSuccessfulAt: status.sync.lastSuccessfulAt, error: status.sync.error,
    controlRecoverable: status.sync.controlRecoverable,
  };
}

/** Stable source for useBbsSyncView. Importing it performs no registration, read,
 * network request, timer or storage write. */
export const bbsSyncViewSource = Object.freeze({
  read: async () => bbsSyncStatusView(await bbsSyncStatus()),
  listen: onBbsSyncChanged,
});
