import { invoke } from '@tauri-apps/api/core';
import type { LocalScope } from './local-rpc';

export type LocalMcpHealth = 'connected' | 'connecting' | 'needs_auth' | 'not_connected' | 'failed';
export type LocalMcpReason = 'connection_failed' | 'connection_refused' | 'command_not_found' | 'connection_timeout' | 'authentication_failed';
export interface LocalMcpServer {
  name: string; status: LocalMcpHealth; configured: boolean; enabled: boolean; reason?: LocalMcpReason;
}
export interface LocalMcpView { schema: 1; available: boolean; servers: LocalMcpServer[] }
export const readLocalMcp = (projectRoot: string, agentId: string, scope?: LocalScope) =>
  invoke<LocalMcpView>('local_rpc_mcp_status', { projectRoot, agentId, scope });
export const removeLocalMcp = (projectRoot: string, agentId: string, name: string) =>
  invoke<boolean>('local_rpc_mcp_remove', { projectRoot, agentId, name });

export const localMcpLabels: Record<LocalMcpHealth, string> = {
  connected: 'Connected', connecting: 'Connecting', needs_auth: 'Needs sign-in',
  not_connected: 'Not connected', failed: 'Failed',
};
export const localMcpReasons: Record<LocalMcpReason, string> = {
  connection_failed: 'Connection failed.', connection_refused: 'Connection refused.',
  command_not_found: 'Server command not found.', connection_timeout: 'Connection timed out.',
  authentication_failed: 'Authentication failed.',
};
