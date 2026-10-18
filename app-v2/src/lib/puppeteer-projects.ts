import { loadProjectAgentDetail, saveProjectAgentDetail, type ProjectAgentDetail } from '../pty-client';
import { AUTO_ROUTE_MODEL, type AutoRouteGroup } from './string-routing';

export function isStringAgent(detail: ProjectAgentDetail): boolean {
  return detail.status === 'active' && detail.provider === 'kota-local' && detail.model === AUTO_ROUTE_MODEL;
}
/** Reload before each write so Apply never carries the source's identity or SHELL fields. */
export async function saveAgentString(projectRoot: string, agentId: string, autoRoute: AutoRouteGroup, sourceRevision?: string) {
  const detail = await loadProjectAgentDetail({ projectRoot, agentId }, { readOnly: true });
  if (!isStringAgent(detail)) throw { code: 'string_agent_changed' };
  if (sourceRevision !== undefined && sourceRevision !== detail.shellRevision) throw { code: 'local_config_changed' };
  return saveProjectAgentDetail({ projectRoot, agentId, autoRoute,
    shellRevision: detail.shellRevision, provider: detail.provider, displayName: detail.displayName, nameFields: detail.nameFields,
    model: detail.model, effort: detail.effort, avatarId: detail.avatarId, skills: detail.skills, ghost: detail.ghost,
  });
}
export interface StringAgentTarget { projectRoot: string; agentId: string }
export async function applyAgentString(targets: readonly StringAgentTarget[], group: AutoRouteGroup,
  onResult: (target: StringAgentTarget, detail: ProjectAgentDetail | null) => void) {
  for (const target of targets) {
    let detail: ProjectAgentDetail | null = null;
    try { detail = await saveAgentString(target.projectRoot, target.agentId, group); } catch { /* Keep partial successes and let the caller select failures for Retry. */ }
    onResult(target, detail);
  }
}
