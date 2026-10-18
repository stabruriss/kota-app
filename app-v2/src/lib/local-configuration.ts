import { listen } from '@tauri-apps/api/event';

export interface LocalConfigurationChange { projectRoot: string; agentId: string; revision: string }
export const onLocalConfiguration = (projectRoot: string, agentId: string, callback: (change: LocalConfigurationChange) => void) =>
  listen<LocalConfigurationChange>(`local-rpc://agent/${agentId}/configuration`, event => {
    if (event.payload.projectRoot === projectRoot && event.payload.agentId === agentId) callback(event.payload);
  });

const FAILURES: Record<string, { message: string; reload: boolean }> = {
  local_session_changed: { message: 'The session changed before settings could be applied. Nothing was saved. Reopen this profile.', reload: false },
  local_model_apply_unconfirmed: { message: 'Settings were not saved. Session changes may have partially applied, or the session may have closed. Check the workbench before retrying.', reload: true },
  local_model_session_replaced: { message: 'The session was replaced while applying settings. Nothing was saved; changes to the previous session may have applied. Reopen this profile.', reload: true },
  local_model_session_closed: { message: 'Pi returned a different model and the session was closed. Settings were not saved. No session was restarted.', reload: true },
  local_model_applied_save_failed: { message: 'The model settings were applied to the running session, but could not be saved. Reload saved settings and check the workbench before retrying.', reload: true },
  local_model_applied_metadata_failed: { message: 'The model settings were saved and applied to the running session, but the remaining profile changes could not be fully saved.', reload: true },
};
export function localConfigurationSaveFailure(error: unknown) {
  const code = typeof error === 'string' ? error : (error as { code?: string } | null)?.code;
  return code ? FAILURES[code] ?? null : null;
}
