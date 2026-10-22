import { useEffect, useRef, useState } from 'react';
import {
  projectConnectionApply, projectConnectionCandidates, projectConnectionCheck,
  type ConnectionCandidates, type ConnectionError, type WorkspaceProject,
} from '../pty-client';
import { beginGithubSignIn, openGithubAccounts, refreshGithubAccounts, useGithubAccounts } from '../lib/github-account-flow';
import { connectionChanged } from '../lib/project-connection-ui';
import { ProjectConnectionDialog, REPOSITORY_STATES, repositoryCanConnect, type ConnectionDraft, type RepositoryCheckView } from './ProjectConnectionDialog';
import { ProjectConnectionConfirm } from './ProjectConnectionConfirm';

export function connectionError(error: unknown): ConnectionError {
  if (error && typeof error === 'object' && 'message' in error) {
    const failure = error as Partial<ConnectionError>;
    return { ...failure, code: failure.code ?? 'internal', message: String(failure.message) };
  }
  return { code: 'internal', message: String(error) };
}

export function ProjectConnectDialog({ workspace, login, loginReady, switchOrigin, onCancel, onSaved }: {
  workspace: WorkspaceProject; login?: string | null; loginReady: boolean; switchOrigin: boolean;
  onCancel: () => void; onSaved: () => void;
}) {
  const flow = useGithubAccounts();
  const name = workspace.displayName || workspace.repoFullName.split('/').pop() || workspace.projectId;
  const replacing = workspace.connection?.kind !== 'local';
  const [draft, setDraft] = useState<ConnectionDraft>({ accountLogin: flow.view?.defaultLogin ?? '', mode: 'existing',
    name, private: true, repoFullName: '' });
  const [candidates, setCandidates] = useState<ConnectionCandidates | null>(null);
  const [loading, setLoading] = useState(true);
  const [check, setCheck] = useState<RepositoryCheckView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [probeRetry, setProbeRetry] = useState(0);
  const [listRetry, setListRetry] = useState(0);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const saving = useRef(false), alive = useRef(true), checkSequence = useRef(0), modeTouched = useRef(false);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { void refreshGithubAccounts(); }, []);
  useEffect(() => {
    if (!draft.accountLogin && flow.view?.defaultLogin) setDraft(value => ({ ...value, accountLogin: flow.view!.defaultLogin! }));
  }, [draft.accountLogin, flow.view?.defaultLogin]);

  useEffect(() => {
    let cancelled = false;
    ++checkSequence.current;
    setCandidates(null); setError(null); setCheck(null);
    if (!draft.accountLogin) { setLoading(false); return; }
    const accountLogin = draft.accountLogin;
    setLoading(true); setCheck({ kind: 'checking' });
    void projectConnectionCandidates(workspace.projectId, accountLogin).then(result => {
      if (cancelled || result.accountLogin !== accountLogin) return;
      setCandidates(result);
      setDraft(value => ({ ...value, repoFullName: result.selected ?? '',
        mode: modeTouched.current ? value.mode : result.selected ? 'existing' : 'new' }));
    }).catch(reason => {
      if (!cancelled) { setCheck({ kind: 'check_failed' }); setError(connectionError(reason).message); }
    }).finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [workspace.projectId, draft.accountLogin, listRetry]);

  useEffect(() => {
    const sequence = ++checkSequence.current;
    if (!candidates || candidates.accountLogin !== draft.accountLogin) return;
    if (draft.mode === 'new') {
      setCheck(candidates.largeFiles.length ? { kind: 'blocked_large_file', largeFiles: candidates.largeFiles } : { kind: 'empty' });
      return;
    }
    if (!draft.repoFullName) { setCheck(null); return; }
    setCheck({ kind: 'checking' });
    void projectConnectionCheck(workspace.projectId, draft.accountLogin, draft.repoFullName).then(result => {
      if (!alive.current || sequence !== checkSequence.current || result.accountLogin !== draft.accountLogin || result.repoFullName !== draft.repoFullName) return;
      setCheck({ kind: result.state, largeFiles: result.largeFiles });
    }).catch(() => {
      if (alive.current && sequence === checkSequence.current) setCheck({ kind: 'check_failed' });
    });
    return () => { ++checkSequence.current; };
  }, [workspace.projectId, candidates, draft.accountLogin, draft.mode, draft.repoFullName, probeRetry]);

  const changeDraft = (next: ConnectionDraft) => {
    if (saving.current) return;
    ++checkSequence.current;
    setError(null);
    if (next.mode !== draft.mode) modeTouched.current = true;
    if (next.accountLogin !== draft.accountLogin || next.repoFullName !== draft.repoFullName || next.mode !== draft.mode) setCheck({ kind: 'checking' });
    setDraft(next.accountLogin !== draft.accountLogin ? { ...next, repoFullName: '' } : next);
  };
  const apply = async () => {
    if (saving.current || !repositoryCanConnect(check?.kind)) return;
    saving.current = true; setBusy(true); setError(null);
    const sent = { ...draft };
    try {
      const result = await projectConnectionApply({ projectId: workspace.projectId, accountLogin: sent.accountLogin,
        target: sent.mode === 'new' ? { kind: 'new', name: sent.name.trim(), private: sent.private } : { kind: 'existing', repoFullName: sent.repoFullName } });
      connectionChanged(result.workspace, replacing ? 'account' : 'connect');
      if (alive.current) onSaved();
    } catch (reason) {
      if (!alive.current) return;
      const failure = connectionError(reason);
      const retained = replacing ? `This project is still connected to ${workspace.repoFullName}${login ? ` with ${login}` : ''}.` : 'Your project is still Local only.';
      setError(`${failure.createdRepo ? `${failure.createdRepo} was created. The repository remains on GitHub. ` : ''}${failure.message} ${retained}`);
      setConfirm(false);
      if (failure.createdRepo) {
        // Retrying uses the repository already created, never creates it twice.
        modeTouched.current = true;
        setDraft(value => ({ ...value, mode: 'existing', repoFullName: failure.createdRepo! }));
        setProbeRetry(value => value + 1);
      } else if (failure.state || failure.code === 'blocked_large_file' || failure.code === 'check_failed') {
        ++checkSequence.current;
        setCheck({ kind: failure.state ?? (failure.code === 'blocked_large_file' ? 'blocked_large_file' : 'check_failed'), largeFiles: failure.largeFiles });
      }
      if (failure.code.startsWith('github_account_')) void refreshGithubAccounts();
    } finally { saving.current = false; if (alive.current) setBusy(false); }
  };
  const accountReady = flow.view?.accounts.some(account => account.login === draft.accountLogin && account.state === 'ok');
  return <>
    <ProjectConnectionDialog name={name} replacing={replacing} accounts={flow.view} draft={draft}
      repos={candidates?.candidates ?? []} reposLoading={loading} check={check} error={error} busy={busy}
      onDraft={changeDraft} onCancel={onCancel} onContinue={() => { if (accountReady && repositoryCanConnect(check?.kind)) setConfirm(true); }}
      onRetry={() => { setError(null); if (!candidates || draft.mode === 'new') setListRetry(value => value + 1); else setProbeRetry(value => value + 1); }}
      onAddAccount={() => { onCancel(); openGithubAccounts(); void beginGithubSignIn(); }} />
    {confirm && <ProjectConnectionConfirm change={replacing ? 'account' : 'connect'} name={name}
      from={replacing ? `GH ID ${login ?? '—'}\n${workspace.repoFullName}` : 'Local only'}
      to={`GH ID ${draft.accountLogin}\n${draft.mode === 'new' ? `${draft.accountLogin}/${draft.name.trim()}` : draft.repoFullName}`}
      login={login ?? undefined} loginReady={loginReady} busy={busy}
      operation={draft.mode === 'new' ? `Kota will create a ${draft.private ? 'private' : 'public'} repository and push this project's history.` : check ? REPOSITORY_STATES[check.kind][1] : undefined}
      action={draft.mode === 'new' ? 'Create & connect' : replacing ? 'Change account' : 'Connect'}
      onCancel={() => { if (!saving.current) { if (switchOrigin) onCancel(); else setConfirm(false); } }} onConfirm={() => void apply()} />}
  </>;
}
