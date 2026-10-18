import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { AgentSessionLeaseConflictError, parseAgentSessionLeaseConflict } from '../pty-client';
import type { AgentRoute, AgentSpawnRequest } from '../types/agent-pty';
import { stringErrorText, type AutoRouteGroup } from './string-routing';

export interface LocalStartupOptions { attemptId: string }
export interface LocalStartSelection { providerId: string; modelId: string; thinkingLevel: string; autoRoute?: AutoRouteGroup }
export interface LocalStartError { code: string; providerId?: string; stage?: string }
export interface LocalStartStep {
  stage: string; startedAt: string; finishedAt: string | null; elapsedMs: number; error: LocalStartError | null;
}
export interface LocalStartView {
  projectRoot: string; agentId: string; attemptId: string; revision: number;
  state: 'running' | 'ready' | 'failed' | 'cancelled';
  selection: LocalStartSelection | null; saved: boolean; steps: LocalStartStep[]; error: LocalStartError | null;
}
export const localLaunchAction = <T,>(projectRoot: string, agentId: string, action: string, extra: object = {}) =>
  invoke<T>('local_rpc_launch_action', { request: { action, projectRoot, agentId, ...extra } });
export const onLocalStartup = (agentId: string, apply: (view: LocalStartView) => void) =>
  listen<LocalStartView>(`local-rpc://agent/${agentId}/startup`, event => apply(event.payload));
export async function startLocalWorkbench(request: AgentSpawnRequest, startup: LocalStartupOptions): Promise<AgentRoute> {
  try { return await invoke<AgentRoute>('local_rpc_start', { request, attemptId: startup.attemptId }); }
  catch (error) {
    const conflict = parseAgentSessionLeaseConflict(error);
    if (conflict) throw new AgentSessionLeaseConflictError(conflict);
    throw error;
  }
}
export const localStartStage = (stage: string) => ({
  configuration_read: 'Read configuration', configuration_lock: 'Open Kota configuration',
  model_catalog: 'Read model list', model_metadata: 'Read model metadata',
  credential_status: 'Check credentials', helper_start: 'Start Pi query', helper_ready: 'Wait for Pi commands',
  credential_store: 'Waiting for credential store',
  resolve_model: 'Check model and thinking', configuration_save: 'Save configuration',
  launch_configuration: 'Prepare session', prepare_adapter: 'Prepare workspace',
  session_ready: 'Start Pi session', session_commit: 'Connect session', session_takeover: 'Confirm session takeover',
}[stage] ?? 'Start session');
export function localStartErrorText(error: LocalStartError): string {
  if (['auto_route_models_required', 'auto_route_invalid', 'jev_key_required', 'jev_key_rejected'].includes(error.code)) return stringErrorText(error);
  if (error.code === 'model_required') return 'Choose a model to start this agent.';
  if (error.code === 'provider_not_signed_in') return 'This provider is not signed in. Sign in or choose another model.';
  if (error.code === 'credential_invalid' || error.code === 'azure_resource_missing') return 'This provider needs to sign in again.';
  if (error.code === 'model_not_found') return 'This model is unavailable. Choose another model.';
  if (error.code === 'local_config_busy') return 'Kota configuration is busy. Try again.';
  if (error.code === 'auth_store_busy') return 'Credential storage is busy. Try again.';
  if (error.code === 'auth_timeout') return `${localStartStage(error.stage ?? '')} timed out. Try again.`;
  if (error.code === 'startup_cancelled') return 'Startup cancelled.';
  if (error.code === 'startup_superseded' || error.code === 'local_config_changed') return 'Configuration changed. Check the model and try again.';
  return `${localStartStage(error.stage ?? '')} failed. Try again. (${error.code})`;
}
