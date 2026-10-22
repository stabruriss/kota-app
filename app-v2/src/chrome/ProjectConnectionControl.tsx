import { useEffect, useRef, useState } from 'react';
import { projectConnectionSetLocal, type WorkspaceProject } from '../pty-client';
import { openGithubAccounts, refreshGithubAccounts, useGithubAccounts } from '../lib/github-account-flow';
import { useProjectConnectionStatus } from '../lib/project-connection-status';
import { connectionChanged, dismissConnectionNotice, useConnectionNotice } from '../lib/project-connection-ui';
import { useProjectLinkTransition } from '../lib/project-link-transition';
import { ProjectGithubLink } from './ProjectGithubLink';
import { ProjectConnectionConfirm } from './ProjectConnectionConfirm';
import { ProjectConnectDialog, connectionError } from './ProjectConnectDialog';
import { GithubSignIn } from './GithubSignIn';

export function ProjectConnectionControl({ workspace, connectRequested, onRequestHandled, onBlockingChange }: {
  workspace: WorkspaceProject; connectRequested: boolean; onRequestHandled: () => void; onBlockingChange: (blocked: boolean) => void;
}) {
  const flow = useGithubAccounts();
  const view = useProjectConnectionStatus(workspace);
  const linked = workspace.connection?.kind !== 'local';
  const motion = useProjectLinkTransition(linked);
  const notice = useConnectionNotice(workspace.projectId);
  const [choosing, setChoosing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const saving = useRef(false), alive = useRef(true);
  const login = view.status?.kind === 'github' ? view.status.accountLogin : workspace.connection?.kind === 'github' ? workspace.connection.accountLogin : null;
  const accountState = view.status?.kind === 'github' ? view.status.accountState : null;
  const missing = accountState === 'missing' || accountState === 'unbound';
  const hasAccount = flow.view?.accounts.some(account => account.state === 'ok');
  const blocked = choosing || motion.phase !== 'idle' || busy;
  const name = workspace.displayName || workspace.repoFullName.split('/').pop() || workspace.projectId;
  useEffect(() => { alive.current = true; void refreshGithubAccounts(); return () => { alive.current = false; }; }, []);
  useEffect(() => { onBlockingChange(blocked); return () => onBlockingChange(false); }, [blocked, onBlockingChange]);
  useEffect(() => {
    if (connectRequested) { setChoosing(true); onRequestHandled(); }
  }, [connectRequested, onRequestHandled]);
  const setLocal = async () => {
    if (saving.current) return;
    saving.current = true; setBusy(true); setError(null);
    try {
      const next = await projectConnectionSetLocal(workspace.projectId);
      connectionChanged(next, 'local');
      if (alive.current) motion.complete(false);
    } catch (reason) {
      if (alive.current) setError(connectionError(reason).message);
    } finally { saving.current = false; if (alive.current) setBusy(false); }
  };
  return <>
    <ProjectGithubLink linked={motion.preview} disabled={blocked || (!linked && !hasAccount)} buttonRef={motion.trigger}
      onTransitionEnd={motion.onTransitionEnd} onChange={() => { setError(null); motion.toggle(); }}>
      {!linked ? 'Local only' : login ? `Connected to GH ID ${login}` : 'Connected'}
    </ProjectGithubLink>
    {!linked ? <><p className="project-settings-hint">Local work continues. Bartender will not pull from or push to GitHub.</p>
      {!hasAccount && <button type="button" className="project-settings-button secondary" onClick={openGithubAccounts}>Add GitHub account in Link</button>}</>
      : <>
        {workspace.repoFullName && <p className="project-settings-hint">{workspace.repoFullName}</p>}
        {(missing || accountState === 'needs_login') && <div className="project-account-issue">
          <strong>{missing ? 'No account assigned' : 'Sign in required'}</strong><p>Pull and Push are paused for this project.</p>
          {missing ? <button type="button" className="project-settings-button secondary" onClick={() => setChoosing(true)}>Choose account</button> : <GithubSignIn />}
        </div>}
        {!missing && <button type="button" className="project-settings-button secondary project-connection-action" disabled={blocked}
          onClick={() => setChoosing(true)}>Use another GH account for this project</button>}
      </>}
    {view.error && <p className="project-settings-status error" role="alert">{view.error} <button type="button" onClick={view.retry}>Retry</button></p>}
    {notice && <div className="project-connection-notice" role="status">{notice}
      <button type="button" title="Dismiss reminder" aria-label="Dismiss restart reminder" onClick={() => dismissConnectionNotice(workspace.projectId)}>×</button></div>}
    {motion.phase === 'popup' && linked && <ProjectConnectionConfirm change="local" name={name}
      from={`GH ID ${login ?? '—'}\n${workspace.repoFullName}`} to="Local only" login={login ?? undefined} loginReady={accountState === 'ok'} busy={busy} error={error}
      onCancel={() => { if (!saving.current) motion.cancel(); }} onConfirm={() => void setLocal()} />}
    {(choosing || (motion.phase === 'popup' && !linked)) && <ProjectConnectDialog workspace={workspace} login={login} loginReady={accountState === 'ok'} switchOrigin={!choosing}
      onCancel={() => { if (choosing) setChoosing(false); else motion.cancel(); }}
      onSaved={() => { setChoosing(false); motion.complete(true); }} />}
  </>;
}
