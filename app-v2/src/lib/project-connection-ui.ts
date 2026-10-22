import { useSyncExternalStore } from 'react';
import type { WorkspaceProject } from '../pty-client';

export const PROJECT_CONNECTION_REQUEST = 'kota:project-connection';
export const PROJECT_CONNECTION_CHANGED = 'kota:project-connection-changed';
export const RESTART_NOTICE = {
  account: 'Git uses the new account now. Restart running agents for gh to use it.',
  connect: 'Git can reach GitHub now. Restart running agents to use gh.',
  local: 'Git no longer reaches GitHub. Restart running agents to remove gh access.',
} as const;
// A notice acknowledges a successful connection change, not agent state. It
// survives closing Settings and only disappears on × (or App restart).
const notices = new Map<string, string>();
const listeners = new Set<() => void>();
const notify = () => listeners.forEach(listener => listener());
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
export const useConnectionNotice = (projectId: string) => useSyncExternalStore(subscribe, () => notices.get(projectId) ?? null);
export function dismissConnectionNotice(projectId: string) { notices.delete(projectId); notify(); }
export function connectionChanged(workspace: WorkspaceProject, change: keyof typeof RESTART_NOTICE) {
  notices.set(workspace.projectId, RESTART_NOTICE[change]); notify();
  window.dispatchEvent(new CustomEvent(PROJECT_CONNECTION_CHANGED, { detail: workspace }));
}
export function requestProjectConnection(projectId: string) {
  window.dispatchEvent(new CustomEvent(PROJECT_CONNECTION_REQUEST, { detail: { projectId } }));
}

/** Connection changes do not replace an independently refreshed agent roster. */
export function mergeWorkspaceConnection(current: WorkspaceProject, next: WorkspaceProject): WorkspaceProject {
  if (current.projectId !== next.projectId) return current;
  return { ...current, connection: next.connection, displayName: next.displayName,
    repoFullName: next.repoFullName, remoteUrl: next.remoteUrl, githubHtmlUrl: next.githubHtmlUrl,
    defaultBranch: next.defaultBranch, baseRef: next.baseRef };
}
