import type { BbsMentionSelection, BbsMentionTarget, BbsRosterAgent, BbsRosterDevice, BbsRosterProject } from './types/bbs-roster';

const mentionErrors = {
  mention_requires_group: 'This agent is on another device, but this device is not in a sync group. No thread was created.',
  mention_thread_not_shared: 'This agent is on another device, but this thread is not shared. No reply was posted. Start a new thread to @ this agent.',
  agent_roster_not_synced: 'The agent list for this device has not synced yet. Try again after syncing.',
} as const;
export class BbsMentionClientError extends Error {
  constructor(public readonly code: keyof typeof mentionErrors) { super(mentionErrors[code]); this.name = 'BbsMentionClientError'; }
}
/** Only an exact, single-field structured rejection can claim a mention error.
 * Legacy attachment failures stay on their existing path, never guessed as codes. */
export function bbsMentionError(value: unknown): BbsMentionClientError | null {
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).length !== 1 || !Object.hasOwn(value, 'code')) return null;
  const { code } = value as { code: unknown };
  return typeof code === 'string' && Object.hasOwn(mentionErrors, code) ? new BbsMentionClientError(code as keyof typeof mentionErrors) : null;
}

export const bbsMentionKey = ({ deviceId, projectId, agentId }: BbsMentionTarget) => JSON.stringify([deviceId, projectId, agentId]);
export const bbsMentionDeviceKey = (device: BbsRosterDevice) => device.local ? 'local' : device.deviceId!;

export function bbsMentionSelection(device: BbsRosterDevice, project: BbsRosterProject, agent: BbsRosterAgent): BbsMentionSelection {
  return { deviceId: bbsMentionDeviceKey(device), projectId: project.projectId, agentId: agent.agentId,
    deviceName: device.name, projectName: project.name, agentName: agent.name };
}

export function bbsUniqueMentions(values: readonly BbsMentionSelection[]): BbsMentionSelection[] {
  return [...new Map(values.map(value => [bbsMentionKey(value), value])).values()];
}

/** Follow current display names, but never silently drop or re-address a selection. */
export function bbsCurrentMentionNames(values: readonly BbsMentionSelection[], devices: readonly BbsRosterDevice[]): BbsMentionSelection[] {
  const byDevice = new Map(devices.map(device => [bbsMentionDeviceKey(device), device]));
  return bbsUniqueMentions(values).map(value => {
    const device = byDevice.get(value.deviceId);
    const project = device?.projects?.find(item => item.projectId === value.projectId);
    const agent = project?.agents.find(item => item.agentId === value.agentId);
    return { ...value, deviceName: device?.name ?? value.deviceName,
      projectName: project?.name ?? value.projectName, agentName: agent?.name ?? value.agentName };
  });
}

export function bbsMentionGroups(values: readonly BbsMentionSelection[]) {
  const groups = new Map<string, { key: string; deviceName: string; projectName: string; targets: BbsMentionSelection[] }>();
  for (const value of bbsUniqueMentions(values)) {
    const key = JSON.stringify([value.deviceId, value.projectId]);
    if (!groups.has(key)) groups.set(key, { key, deviceName: value.deviceName, projectName: value.projectName, targets: [] });
    groups.get(key)!.targets.push(value);
  }
  return [...groups.values()];
}

export function bbsMentionCounts(values: readonly BbsMentionSelection[]) {
  const unique = bbsUniqueMentions(values);
  return { agents: unique.length, projects: bbsMentionGroups(unique).length, devices: new Set(unique.map(value => value.deviceId)).size };
}

export const bbsMentionCountLabel = (n: number, noun: string) => `${n} ${noun}${n === 1 ? '' : 's'}`;

export function bbsMentionTargets(values: readonly BbsMentionSelection[]): BbsMentionTarget[] {
  return bbsUniqueMentions(values).map(({ deviceId, projectId, agentId }) => ({ deviceId, projectId, agentId }));
}
