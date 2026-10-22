import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  githubCreateRepo, githubListRepos, ghAuthStatus, listWorkspaceProjects,
  openWorkspaceProject, prepareGithubProject, createLocalProject, gitAvailability,
  installGitCommandLineTools, type GhAuthInfo, type GithubRepo,
  type WorkspaceProject, type GitAvailability,
} from '../pty-client';
import { ProjectGithubLink } from './ProjectGithubLink';
import { refreshGithubAccounts, useGithubAccounts } from '../lib/github-account-flow';
import { GithubSignIn } from './GithubSignIn';
import '../styles/github-accounts.css';

interface ProjectSetupModalProps {
  open: boolean;
  onClose: () => void;
  onWorkspacePrepared: (workspace: WorkspaceProject) => void;
  mode?: 'firstProject' | 'openProject';
  embedded?: boolean;
}

export function ProjectSetupModal({ open, onClose, onWorkspacePrepared, mode = 'openProject', embedded = false }: ProjectSetupModalProps) {
  const [ghAuth, setGhAuth] = useState<GhAuthInfo | null>(null);
  const accountFlow = useGithubAccounts(open);
  const accounts = accountFlow.view;
  const [chosenLogin, setChosenLogin] = useState<string | null>(null);
  const [ghAuthLoading, setGhAuthLoading] = useState(true);
  const [authError, setAuthError] = useState<string | null>(null);
  const [git, setGit] = useState<GitAvailability | null>(null);
  const [gitError, setGitError] = useState<string | null>(null);
  const [repos, setRepos] = useState<GithubRepo[]>([]);
  const [reposLoading, setReposLoading] = useState(false);
  const [reposError, setReposError] = useState<string | null>(null);
  const [repoRefresh, setRepoRefresh] = useState(0);
  const [workspaces, setWorkspaces] = useState<WorkspaceProject[]>([]);
  const [newRepoName, setNewRepoName] = useState('');
  const [newRepoPrivate, setNewRepoPrivate] = useState(true);
  const [linkDraft, setLinkDraft] = useState<boolean | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0), authSerial = useRef(0), gitSerial = useRef(0), operating = useRef(false);
  const account = accounts?.accounts.find(item => item.login === (chosenLogin ?? accounts.defaultLogin));
  const canLink = !ghAuthLoading && account?.state === 'ok';
  const linked = canLink && (linkDraft ?? true);
  const accountLogin = canLink ? account.login : undefined;

  const refreshAuth = useCallback(async () => {
    const epoch = generation.current, serial = ++authSerial.current;
    setGhAuthLoading(true); setAuthError(null);
    const [gh] = await Promise.allSettled([ghAuthStatus(), refreshGithubAccounts()]);
    if (epoch !== generation.current || serial !== authSerial.current) return;
    setGhAuth(gh.status === 'fulfilled' ? gh.value : null);
    if (gh.status === 'rejected') setAuthError(String(gh.reason));
    setGhAuthLoading(false);
  }, []);
  const refreshGit = useCallback(async () => {
    const epoch = generation.current, serial = ++gitSerial.current;
    setGit(null); setGitError(null);
    try {
      const next = await gitAvailability();
      if (epoch === generation.current && serial === gitSerial.current) setGit(next);
    } catch (reason) {
      if (epoch === generation.current && serial === gitSerial.current) setGitError(String(reason));
    }
  }, []);

  useEffect(() => {
    if (!open) return;
    const epoch = ++generation.current;
    setError(null); setGhAuth(null); setChosenLogin(null); setLinkDraft(null); setWorkspaces([]);
    void refreshAuth(); void refreshGit();
    void listWorkspaceProjects().then(value => { if (epoch === generation.current) setWorkspaces(value); }).catch(() => {});
    return () => { ++generation.current; };
  }, [open, refreshAuth, refreshGit]);

  useEffect(() => {
    if (!open || ghAuthLoading || canLink || authError) return;
    // Existing sign-in flow uses Smart Terminal; stop probing as soon as an account is ready.
    const timer = window.setInterval(() => { void refreshAuth(); }, 3000);
    return () => window.clearInterval(timer);
  }, [open, ghAuthLoading, canLink, authError, refreshAuth]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      if (!operating.current) onClose();
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, [onClose, open]);

  useEffect(() => {
    setRepos([]); setReposError(null);
    if (!open || !accountLogin) { setReposLoading(false); return; }
    let cancelled = false;
    setReposLoading(true);
    void githubListRepos(accountLogin).then(next => { if (!cancelled) setRepos(next); })
      .catch(reason => { if (!cancelled) setReposError(String(reason)); })
      .finally(() => { if (!cancelled) setReposLoading(false); });
    return () => { cancelled = true; };
  }, [open, accountLogin, repoRefresh]);

  const run = async (label: string, fn: () => Promise<void>) => {
    if (operating.current) return;
    operating.current = true; setBusy(label); setError(null);
    try { await fn(); } catch (reason) {
      const message = String(reason);
      setError(message.includes('git_unavailable') ? 'Git is required to create a project.'
        : message.includes('local_project_name_invalid') ? 'Use 1–100 characters without control characters for the project name.' : message);
    }
    finally { operating.current = false; setBusy(null); }
  };
  const openedByRepo = useMemo(() => new Map(workspaces.filter(item => item.connection?.kind !== 'local' && item.repoFullName)
    .map(item => [item.repoFullName, item])), [workspaces]);
  const ghMissing = !ghAuthLoading && !!ghAuth?.cliMissing;
  const title = mode === 'firstProject' ? 'Create First Project' : 'Open Project';

  if (!open) return null;
  return <section className={`project-setup-modal ${embedded ? 'embedded' : ''}`} data-testid="project-setup-modal" role="dialog" aria-label="Open project">
    <div className="tavern-head"><div className="tavern-title">{title}</div>
      {!embedded && <button type="button" className="tavern-close" disabled={busy != null} onClick={onClose} aria-label="Close project setup">×</button>}
    </div>
    <div className="project-setup-grid">
      <section className="tavern-section">
        <div className="tavern-section-title">Select from GitHub</div>
        {!!accounts?.accounts.length && <label>Choose an account<select aria-label="Repository account" value={chosenLogin ?? accounts.defaultLogin ?? ''} disabled={busy != null || accountFlow.busy}
          onChange={event => setChosenLogin(event.currentTarget.value)}>
          <option value="" disabled>Choose an account</option>
          {accounts.accounts.map(item => <option key={item.login} value={item.login} disabled={item.state !== 'ok'}>{item.login}{item.state !== 'ok' ? ' · Sign in required' : ''}</option>)}
        </select></label>}
        {!canLink ? <div className="project-setup-empty">
          <span className={`github-cli-status ${ghAuthLoading ? 'loading' : ghMissing ? 'missing' : 'setup'}`}><span aria-hidden />
            {ghAuthLoading ? 'Checking GitHub CLI...' : ghMissing ? 'GitHub CLI missing' : account?.state === 'needs_login' ? 'Sign in required' : 'No GitHub account connected.'}
          </span>
          {authError && <><p className="tavern-status error" role="alert">{authError}</p><button type="button" onClick={() => void refreshAuth()}>Retry</button></>}
        </div> : <>
          <button type="button" className="project-refresh-button" disabled={reposLoading || busy != null} onClick={() => setRepoRefresh(value => value + 1)}>Refresh repositories</button>
          {reposError && <p className="tavern-status error" role="alert">{reposError}</p>}
          <div className="project-repo-list" data-testid="github-repo-list" aria-busy={reposLoading}>
            {!repos.length ? <div className="project-repo-empty">{reposLoading ? 'Loading repositories...' : reposError ? '' : 'No repositories found.'}</div> : repos.map(repo => {
              const opened = openedByRepo.get(repo.fullName);
              return <button key={repo.fullName} type="button" className={`project-repo-row ${opened ? 'opened' : ''}`} disabled={busy != null || !git?.available}
                onClick={() => void run(opened ? 'open project' : 'clone repo', async () => {
                  const workspace = opened ? await openWorkspaceProject(opened.projectId) : await prepareGithubProject(repo.fullName, accountLogin);
                  onWorkspacePrepared(workspace);
                })}><span><b>{repo.fullName}</b><small>{repo.defaultBranch}{repo.private ? ' · private' : ''}</small></span><em>{opened ? 'Opened in Kota' : 'Clone'}</em></button>;
            })}
          </div>
        </>}
        <GithubSignIn label={ghMissing ? 'Install GitHub CLI' : canLink ? 'Add GitHub account' : 'Login GitHub'} disabled={ghAuthLoading || busy != null} />
      </section>
      <section className="tavern-section project-create-card">
        <div className="tavern-section-title">Create New</div>
        <label>Project name<input value={newRepoName} placeholder="my-kota-project" disabled={busy != null}
          onChange={event => setNewRepoName(event.currentTarget.value)} /></label>
        <ProjectGithubLink linked={linked} disabled={ghAuthLoading || !canLink || busy != null} onChange={setLinkDraft}>
          {linked ? <label>Connected to GH ID <select aria-label="GitHub account" value={accountLogin} disabled={busy != null || accountFlow.busy}
            onChange={event => setChosenLogin(event.currentTarget.value)}>
            {accounts?.accounts.map(item => <option key={item.login} value={item.login} disabled={item.state !== 'ok'}>{item.login}{item.state !== 'ok' ? ' · Sign in required' : ''}</option>)}
          </select></label> : 'Local only'}
        </ProjectGithubLink>
        {linked ? <label className="tavern-inline-check"><input type="checkbox" checked={newRepoPrivate} disabled={busy != null}
          onChange={event => setNewRepoPrivate(event.currentTarget.checked)} />Private repository</label>
          : <p className="project-create-hint">{canLink ? 'This project stays on this computer.' : 'No GitHub sign-in is needed for a local project.'}</p>}
        {!git?.available && <div className="project-git-status" role="status">
          <span>{gitError ?? (git ? 'Git is required to create a project.' : 'Checking Git…')}</span>
          {git && <button type="button" disabled={busy != null} onClick={() => void run('install git', installGitCommandLineTools)}>Install Command Line Tools</button>}
          {(git || gitError) && <button type="button" disabled={busy != null} onClick={() => void refreshGit()}>Check again</button>}
        </div>}
        <button type="button" className="project-create-action" disabled={busy != null || !newRepoName.trim() || !git?.available}
          onClick={() => {
            // An account probe finishing mid-create must not change the chosen path.
            setLinkDraft(linked);
            void run(linked ? 'create repo' : 'create project', async () => {
            if (linked) {
              const repo = await githubCreateRepo({ name: newRepoName.trim(), private: newRepoPrivate, autoInit: true, accountLogin });
              onWorkspacePrepared(await prepareGithubProject(repo.fullName, accountLogin));
            } else onWorkspacePrepared(await createLocalProject(newRepoName.trim()));
            });
          }}>{linked ? 'Create on GitHub + Open' : 'Create local project'}</button>
      </section>
    </div>
    {(busy || error) && <div className={`tavern-status ${error ? 'error' : ''}`} role={error ? 'alert' : 'status'}>{error ?? `Working: ${busy}`}</div>}
  </section>;
}
