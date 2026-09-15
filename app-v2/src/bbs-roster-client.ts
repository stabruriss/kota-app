import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { BBS_AVATAR_MAX_BYTES, bbsAvatarExt, bbsAvatarHash } from './bbs-avatar-images';
import type { BbsRosterAvatar, BbsRosterDevice, BbsRosterPage, BbsRosterProject, BbsRosterRow, BbsRosterView } from './types/bbs-roster';

const messages = {
  unavailable: 'The agent roster requires the Kota runtime.',
  read: 'Could not refresh the agent roster.',
  protocol: 'Kota returned an incompatible agent roster.',
  roster_changed: 'The agent roster changed. Please retry.',
  listen: 'Live roster updates are unavailable. Reopen the picker or retry.',
  cancelled: 'The agent picker is closed.',
} as const;
export class BbsRosterClientError extends Error {
  constructor(public readonly code: keyof typeof messages) { super(messages[code]); this.name = 'BbsRosterClientError'; }
}
export const bbsRosterSafeId = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_-]{1,80}$/.test(value);
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const name = (value: unknown): value is string => typeof value === 'string' && value.length > 0;
const deviceId = (value: unknown): value is string | null => value === null || bbsRosterSafeId(value);
const offset = (value: unknown): value is string => typeof value === 'string' && /^(0|[1-9][0-9]*)$/.test(value) && Number.isSafeInteger(Number(value));
const invalid = () => new BbsRosterClientError('protocol');

function avatar(value: unknown): BbsRosterAvatar {
  if (!object(value)) throw invalid();
  if (value.kind === 'none') return { kind: 'none' };
  // A future builtin ID is public data, not authority to look up a local hero.
  if (value.kind === 'builtin' && bbsRosterSafeId(value.id)) return { kind: 'builtin', id: value.id };
  if (value.kind === 'image' && bbsAvatarHash(value.sha256) && bbsAvatarExt(value.ext)
    && typeof value.sizeBytes === 'number' && Number.isSafeInteger(value.sizeBytes) && value.sizeBytes > 0
    && value.sizeBytes <= BBS_AVATAR_MAX_BYTES && typeof value.available === 'boolean') {
    return { kind: 'image', sha256: value.sha256, ext: value.ext, sizeBytes: value.sizeBytes, available: value.available };
  }
  throw invalid();
}

/** Strict, non-coercing public projection. No paths or unrecognized fields reach
 * the view. Budgets apply per page, never to the total number of legal agents. */
export function parseBbsRosterPage(value: unknown): BbsRosterPage {
  if (!object(value) || !bbsAvatarHash(value.version) || !Array.isArray(value.items) || value.items.length > 64
    || !(value.next === null || offset(value.next))) throw invalid();
  try {
    const encoded = JSON.stringify(value);
    if (encoded.length > 16_375 || new TextEncoder().encode(encoded).length > 16_375) throw invalid();
  } catch { throw invalid(); }
  const items: BbsRosterRow[] = value.items.map(item => {
    if (!object(item) || !deviceId(item.deviceId) || !name(item.name)) throw invalid();
    if (item.kind === 'device' && typeof item.local === 'boolean' && typeof item.online === 'boolean'
      && (item.rosterStatus === 'synced' || item.rosterStatus === 'not_synced')
      && (item.receivedAt === null || (typeof item.receivedAt === 'string' && Number.isFinite(Date.parse(item.receivedAt))))
      && (item.local || (item.deviceId !== null && item.deviceId !== 'local'))
      && (!item.local || item.rosterStatus === 'synced')) {
      return { kind: 'device', deviceId: item.deviceId, name: item.name, local: item.local, online: item.online,
        rosterStatus: item.rosterStatus, receivedAt: item.receivedAt };
    }
    if (!bbsRosterSafeId(item.projectId)) throw invalid();
    if (item.kind === 'project') return { kind: 'project', deviceId: item.deviceId, projectId: item.projectId, name: item.name };
    if (item.kind === 'agent' && bbsRosterSafeId(item.agentId) && typeof item.targetRef === 'string') {
      return { kind: 'agent', deviceId: item.deviceId, projectId: item.projectId, agentId: item.agentId,
        name: item.name, targetRef: item.targetRef, avatar: avatar(item.avatar) };
    }
    throw invalid();
  });
  return { version: value.version, items, next: value.next };
}

export interface BbsRosterPageRequest { version: string | null; after: string | null }
export async function bbsRosterRead({ version, after }: BbsRosterPageRequest): Promise<BbsRosterPage> {
  if (!isTauri()) throw new BbsRosterClientError('unavailable');
  if (!((version === null && after === null) || (bbsAvatarHash(version) && offset(after)))) throw invalid();
  let value: unknown;
  try { value = await invoke('bbs_roster_read', { request: { version, after } }); }
  catch (error) {
    if (object(error) && Object.keys(error).length === 1 && Object.hasOwn(error, 'code') && error.code === 'roster_changed') {
      throw new BbsRosterClientError('roster_changed');
    }
    throw new BbsRosterClientError('read');
  }
  return parseBbsRosterPage(value);
}

/** Accumulate one consistent snapshot only. Cancellation stops the next page;
 * the already-issued memory-only IPC may finish. No server cursor is created. */
export async function readCompleteBbsRoster(
  readPage: (request: BbsRosterPageRequest) => Promise<BbsRosterPage> = bbsRosterRead,
  cancelled: () => boolean = () => false,
): Promise<BbsRosterView> {
  type Device = Omit<BbsRosterDevice, 'projects'> & { projects: Project[] | null };
  type Project = Omit<BbsRosterProject, 'agents'> & { agents: BbsRosterProject['agents'][number][] };
  const devices = new Map<string | null, Device>();
  const projects = new Map<string, Project>();
  const agents = new Set<string>();
  let version: string | null = null;
  let after: string | null = null;
  let local: Device | undefined;
  do {
    if (cancelled()) throw new BbsRosterClientError('cancelled');
    const page = await readPage({ version, after });
    if (cancelled()) throw new BbsRosterClientError('cancelled');
    if (version !== null && version !== page.version) throw new BbsRosterClientError('roster_changed');
    version = page.version;
    const end = Number(after ?? '0') + page.items.length;
    if (!Number.isSafeInteger(end) || (page.next !== null && (page.items.length === 0 || Number(page.next) !== end))) throw invalid();
    for (const row of page.items) {
      if (row.kind === 'device') {
        if (devices.has(row.deviceId) || (row.local && local)) throw invalid();
        const { kind: _, ...fields } = row;
        const item: Device = { ...fields, projects: row.rosterStatus === 'synced' ? [] : null };
        devices.set(row.deviceId, item);
        if (row.local) local = item;
      } else {
        const device = devices.get(row.deviceId);
        if (!device || device.projects === null) throw invalid();
        const projectKey = JSON.stringify([row.deviceId, row.projectId]);
        if (row.kind === 'project') {
          if (projects.has(projectKey)) throw invalid();
          const project: Project = { projectId: row.projectId, name: row.name, agents: [] };
          projects.set(projectKey, project); device.projects.push(project);
        } else {
          const project = projects.get(projectKey);
          const key = JSON.stringify([row.deviceId, row.projectId, row.agentId]);
          const refs = [row.deviceId, ...(device.local ? ['local'] : [])].filter(value => value !== null)
            .map(value => `${value}/${row.projectId}/${row.agentId}`);
          if (!project || agents.has(key) || !refs.includes(row.targetRef)) throw invalid();
          agents.add(key);
          project.agents.push({ agentId: row.agentId, name: row.name, targetRef: row.targetRef, avatar: row.avatar });
        }
      }
    }
    after = page.next;
  } while (after !== null);
  if (!local) throw invalid();
  return { version, devices: [...devices.values()] };
}

export async function onBbsRosterChanged(changed: () => void): Promise<() => void> {
  if (!isTauri()) throw new BbsRosterClientError('unavailable');
  try { return await listen('bbs-roster://changed', () => changed()); }
  catch { throw new BbsRosterClientError('listen'); }
}
/** Stable and inert on import; all reads are explicitly view-driven. */
export const bbsRosterSource = Object.freeze({ read: (cancelled: () => boolean) => readCompleteBbsRoster(bbsRosterRead, cancelled), listen: onBbsRosterChanged });
