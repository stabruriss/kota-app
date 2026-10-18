import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { AgentSessionLeaseConflictError, parseAgentSessionLeaseConflict } from '../pty-client';
import type { AutoRouteGroup, LocalAutoRouteView } from './string-routing';
import type { SoulInput } from '../pty-client';

/** Explicit opt-in. No discovery or background process when it is off. */
export const KOTA_LOCAL_ENABLED = import.meta.env.VITE_KOTA_LOCAL_ENABLED === '1';

export interface LocalScope {
  projectRoot: string;
  agentId: string;
  sessionId: string;
  epoch: number;
}
export type LocalOnlineState = 'offline' | 'starting' | 'ready' | 'busy' | 'error';
export interface LocalSnapshot {
  scope: LocalScope;
  state: LocalOnlineState;
  queued: number;
  compacting: boolean;
  childPid: number | null;
  model: { provider: string; id: string } | null;
  thinkingLevel: string | null;
  error: string | null;
}
export type LocalNotice =
  | { kind: 'status'; scope: LocalScope; state: LocalOnlineState; queued: number; compacting: boolean; error?: string }
  | { kind: 'event'; scope: LocalScope; event: { type: string; [key: string]: unknown } }
  | { kind: 'delivery'; scope: LocalScope; kotaId: string; messageId: string; state: string; at: string; customType: string };

export interface LocalComposerInput {
  projectRoot: string;
  agentId: string;
  messageId: string;
  createdAt: string;
  private: boolean;
  text: string;
  soulInput?: SoulInput;
}
export interface LocalDiscovery {
  installed: boolean;
  version: string | null;
  versionSupported: boolean;
  auth: { provider: string; status: 'ready' | 'not_ready' | 'invalid'; reason?: string } | null;
  models: { id: string; provider: string; model: string }[];
}

export const localSnapshot = (agentId: string) => invoke<LocalSnapshot>('local_rpc_snapshot', { agentId });
export const localInputTargets = (projectRoot: string, agentIds: readonly string[]) =>
  invoke<string[]>('local_rpc_targets', { projectRoot, agentIds });
export async function submitLocal(request: LocalComposerInput): Promise<void> {
  try { await invoke<void>('local_rpc_submit', { request }); }
  catch (error) {
    const conflict = parseAgentSessionLeaseConflict(error);
    if (conflict) throw new AgentSessionLeaseConflictError(conflict);
    throw error;
  }
}
export const discoverLocal = (provider?: string) => invoke<LocalDiscovery>('local_rpc_discover', { provider });
export const controlLocal = (agentId: string, request: Record<string, unknown>) => invoke<unknown>('local_rpc_control', { agentId, request });
export type LocalLaunchCheck = { status: 'ok' } | { status: 'blocked'; code: string; providerId?: string };
export const checkLocalLaunch = (projectRoot: string, agentId: string) =>
  invoke<LocalLaunchCheck>('local_rpc_launch_check', { projectRoot, agentId });
export const setOfflineLocalModel = (projectRoot: string, agentId: string, provider: string, modelId: string, autoRoute?: AutoRouteGroup) =>
  invoke<{ provider: string; modelId: string; thinkingLevel: string; autoRoute?: AutoRouteGroup }>('local_rpc_control', {
    agentId, request: { type: 'set_model', projectRoot, provider, modelId, ...(autoRoute ? { autoRoute } : {}) },
  });
export const onLocalNotice = (agentId: string, callback: (notice: LocalNotice) => void) =>
  listen<LocalNotice>(`local-rpc://agent/${agentId}/notice`, (event) => callback(event.payload));

/** Current-run display only. No native history or attachment objects. */
export interface LocalViewBlock {
  id: string; kind: 'thinking' | 'tool' | 'progress' | 'end'; label: string;
  text: string; active: boolean; error: boolean; truncated: boolean;
  /** Rust receipt time of the first event for this thinking block. */
  startedAt?: string;
}
export interface LocalQueueItem {
  kotaId: string; customType: string; senderId: string | null; createdAt: string;
  state: 'held' | 'sent' | 'accepted' | 'clear_requested'; summary: string; truncated: boolean; handed?: boolean;
}
export interface LocalWorkbenchView extends LocalSnapshot {
  autoRoute?: LocalAutoRouteView | null;
  revision: number; runId: number; startedAt: string | null; phase: string;
  statsRevision: number; dropped: number; queueTotal: number; queueRevision: number;
  /** Pi's successful compaction-end estimate; not provider-exact usage. */
  contextEstimate?: number | null;
  blocks?: LocalViewBlock[]; queue?: LocalQueueItem[];
}
export interface LocalModel { provider: string; id: string; name?: string }
export interface LocalContext { tokens: number | null; contextWindow: number; percent: number | null }
export interface LocalAuth { provider: string; status: 'ready' | 'not_ready' | 'invalid'; authType: 'oauth' | 'api_key' | null }
export const sameLocalScope = (a: LocalScope, b: LocalScope) =>
  a.projectRoot === b.projectRoot && a.agentId === b.agentId && a.sessionId === b.sessionId && a.epoch === b.epoch;
export const readLocalWorkbench = (projectRoot: string, agentId: string, since?: { scope: LocalScope; revision: number; queueRevision: number }) =>
  invoke<LocalWorkbenchView | null>('local_rpc_workbench', { projectRoot, agentId, since });
export const controlLocalWorkbench = <T,>(scope: LocalScope, request: Record<string, unknown>) =>
  invoke<T>('local_rpc_workbench_control', { scope, request });
export const readLocalAuth = (provider: string) => invoke<LocalAuth>('local_rpc_workbench_auth', { provider });
