import { useEffect, useState } from 'react';
import { projectConnectionStatus, type ConnectionStatus, type WorkspaceProject } from '../pty-client';
import { useGithubAccounts } from './github-account-flow';

const reads = new Map<string, Promise<ConnectionStatus>>();
function readStatus(key: string, projectId: string) {
  let task = reads.get(key);
  if (!task) {
    task = Promise.resolve().then(() => projectConnectionStatus(projectId)).finally(() => { if (reads.get(key) === task) reads.delete(key); });
    reads.set(key, task);
  }
  return task;
}

/** Shared in-flight reads, no polling. Account actions and project metadata invalidate the view. */
export function useProjectConnectionStatus(workspace?: WorkspaceProject | null) {
  const local = workspace?.connection?.kind === 'local';
  const accounts = useGithubAccounts(!!workspace && !local);
  const signature = JSON.stringify([workspace?.projectId, workspace?.connection, accounts.view?.defaultLogin,
    accounts.view?.accounts.map(account => [account.login, account.state])]);
  const [snapshot, setSnapshot] = useState<{ key: string; status: ConnectionStatus | null; error: string | null }>({ key: '', status: null, error: null });
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    if (!workspace || local || (!accounts.view && !accounts.error)) return;
    let cancelled = false;
    setSnapshot({ key: signature, status: null, error: null });
    void readStatus(signature, workspace.projectId).then(status => {
      if (!cancelled) setSnapshot({ key: signature, status, error: null });
    }).catch(error => {
      if (!cancelled) setSnapshot({ key: signature, status: null, error: String(error) });
    });
    return () => { cancelled = true; };
  }, [signature, workspace?.projectId, local, accounts.view == null, accounts.error, retry]);
  return {
    status: local ? { kind: 'local' } as const : snapshot.key === signature ? snapshot.status : null,
    error: !local && snapshot.key === signature ? snapshot.error : null,
    retry: () => setRetry(value => value + 1),
  };
}
