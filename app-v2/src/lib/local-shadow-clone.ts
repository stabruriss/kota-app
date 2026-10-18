import { invoke } from '@tauri-apps/api/core';
import { controlLocalWorkbench, type LocalScope, type LocalWorkbenchView } from './local-rpc';

export interface LocalShadowClone {
  id: number; state: 'running' | 'finished' | 'stopped'; startedAt: string;
  elapsedMs: number; lastReply: string; view: LocalWorkbenchView | null;
}
export interface LocalShadowClones { scope: LocalScope; clones: LocalShadowClone[] }
export const readLocalShadowClones = (projectRoot: string, agentId: string, monitor?: number) =>
  invoke<LocalShadowClones | null>('local_rpc_shadow_clones', { projectRoot, agentId, monitor });
export const controlLocalShadowClone = (scope: LocalScope, cloneId: number, action: 'stop' | 'destroy') =>
  controlLocalWorkbench(scope, { type: action === 'stop' ? 'clone_stop' : 'clone_destroy', cloneId });

export const samuraiNames = ['Kambei', 'Gorobei', 'Shichiroji', 'Heihachi', 'Kyuzo', 'Katsushiro', 'Kikuchiyo'] as const;
export const cloneIdentity = (id: number) => samuraiNames[(id - 1) % samuraiNames.length];
