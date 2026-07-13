import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  GITHUB_CLI_INSTALL_URL,
  GITHUB_CLI_LOGIN_COMMAND,
  githubCreateRepo,
  githubListRepos,
  ghAuthStatus,
  listWorkspaceProjects,
  openWorkspaceProject,
  prepareGithubProject,
  type GhAuthInfo,
  type GithubRepo,
  type WorkspaceProject,
} from '../pty-client';

interface ProjectSetupModalProps {
  open: boolean;
  onClose: () => void;
  onWorkspacePrepared: (workspace: WorkspaceProject) => void;
  mode?: 'firstProject' | 'openProject';
  embedded?: boolean;
}

export function ProjectSetupModal({
  open,
  onClose,
  onWorkspacePrepared,
  mode = 'openProject',
  embedded = false,
}: ProjectSetupModalProps) {
  const [ghAuth, setGhAuth] = useState<GhAuthInfo | null>(null);
  const [ghAuthLoading, setGhAuthLoading] = useState(false);
  const [repos, setRepos] = useState<GithubRepo[]>([]);
  const [reposLoaded, setReposLoaded] = useState(false);
  const [workspaces, setWorkspaces] = useState<WorkspaceProject[]>([]);
  const [newRepoName, setNewRepoName] = useState('');
  const [newRepoPrivate, setNewRepoPrivate] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setGhAuthLoading(true);
    try {
      const [gh, existing] = await Promise.all([
        ghAuthStatus(),
        listWorkspaceProjects(),
      ]);
      setGhAuth(gh);
      setWorkspaces(existing);
    } finally {
      setGhAuthLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!open) return;
    setError(null);
    setGhAuth(null);
    setRepos([]);
    setReposLoaded(false);
    void refresh().catch((err) => setError(String(err)));
  }, [open, refresh]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      onClose();
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, [onClose, open]);

  const run = useCallback(async (label: string, fn: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    try {
      await fn();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }, []);

  const loadRepos = useCallback(async () => {
    try {
      const next = await githubListRepos();
      setRepos(next);
    } finally {
      setReposLoaded(true);
    }
  }, []);

  useEffect(() => {
    if (!open || !ghAuth?.authenticated || reposLoaded || busy != null) return;
    void run('load repos', loadRepos);
  }, [busy, ghAuth?.authenticated, loadRepos, open, reposLoaded, run]);

  useEffect(() => {
    if (!open || ghAuthLoading || ghAuth?.authenticated) return;
    let cancelled = false;
    const interval = window.setInterval(() => {
      void ghAuthStatus()
        .then((status) => {
          if (!cancelled) setGhAuth(status);
        })
        .catch(() => {});
    }, 3000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [ghAuth?.authenticated, ghAuthLoading, open]);

  const beginGithubCliLogin = useCallback(async () => {
    const status = await ghAuthStatus();
    setGhAuth(status);
    if (status.cliMissing) {
      window.open(GITHUB_CLI_INSTALL_URL, '_blank', 'noopener,noreferrer');
      return;
    }
    window.dispatchEvent(new CustomEvent('kota:smart-run', {
      detail: { command: GITHUB_CLI_LOGIN_COMMAND },
    }));
  }, []);

  const prepare = async (repoFullName: string) => {
    const workspace = await prepareGithubProject(repoFullName);
    onWorkspacePrepared(workspace);
  };

  const openedByRepo = useMemo(() => {
    const map = new Map<string, WorkspaceProject>();
    for (const workspace of workspaces) {
      map.set(workspace.repoFullName, workspace);
    }
    return map;
  }, [workspaces]);

  const connected = !!ghAuth?.authenticated;
  const ghMissing = !ghAuthLoading && !!ghAuth?.cliMissing;
  const ghActionLabel = ghMissing ? 'Install GitHub CLI' : 'Login GitHub';
  const ghStateLabel = ghAuthLoading
    ? 'Checking GitHub CLI...'
    : ghMissing
      ? 'GitHub CLI missing'
      : 'GitHub CLI not logged in';
  const title = mode === 'firstProject' ? 'Create First Project' : 'Open Project';

  if (!open) return null;

  return (
    <section
      className={`project-setup-modal ${embedded ? 'embedded' : ''}`}
      data-testid="project-setup-modal"
      role="dialog"
      aria-label="Open project"
    >
      <div className="tavern-head">
        <div>
          <div className="tavern-title">{title}</div>
        </div>
        {!embedded && (
          <button type="button" className="tavern-close" onClick={onClose} aria-label="Close project setup">
            ×
          </button>
        )}
      </div>

      {!connected ? (
        <div className="project-setup-empty">
          <span className={`github-cli-status ${ghAuthLoading ? 'loading' : ghMissing ? 'missing' : 'setup'}`}>
            <span aria-hidden />
            {ghStateLabel}
          </span>
          <button
            type="button"
            className={ghMissing ? 'danger' : undefined}
            disabled={ghAuthLoading || busy != null}
            onClick={() => run('github auth', async () => {
              await beginGithubCliLogin();
            })}
          >
            {ghAuthLoading ? 'Checking...' : ghActionLabel}
          </button>
        </div>
      ) : (
        <div className="project-setup-grid">
          <section className="tavern-section">
            <div className="tavern-section-title">Select from GitHub</div>
            <button
              type="button"
              className="project-refresh-button"
              disabled={busy != null}
              onClick={() => run('load repos', loadRepos)}
            >
              Refresh repositories
            </button>
            <div className="project-repo-list" data-testid="github-repo-list">
              {repos.length === 0 ? (
                <div className="project-repo-empty">
                  {reposLoaded ? 'No repositories found.' : 'Loading repositories...'}
                </div>
              ) : repos.map((repo) => {
                const opened = openedByRepo.get(repo.fullName);
                return (
                  <button
                    key={repo.fullName}
                    type="button"
                    className={`project-repo-row ${opened ? 'opened' : ''}`}
                    disabled={busy != null}
                    onClick={() => run(opened ? 'open project' : 'clone repo', async () => {
                      if (opened) {
                        onWorkspacePrepared(await openWorkspaceProject(opened.projectId));
                      } else {
                        await prepare(repo.fullName);
                      }
                    })}
                  >
                    <span>
                      <b>{repo.fullName}</b>
                      <small>{repo.defaultBranch}{repo.private ? ' · private' : ''}</small>
                    </span>
                    <em>{opened ? 'Opened in Kota' : 'Clone'}</em>
                  </button>
                );
              })}
            </div>
          </section>

          <section className="tavern-section">
            <div className="tavern-section-title">Create New</div>
            <label>
              Repository name
              <input
                value={newRepoName}
                placeholder="my-kota-project"
                onChange={(e) => setNewRepoName(e.currentTarget.value)}
              />
            </label>
            <label className="tavern-inline-check">
              <input
                type="checkbox"
                checked={newRepoPrivate}
                onChange={(e) => setNewRepoPrivate(e.currentTarget.checked)}
              />
              Private repository
            </label>
            <button
              type="button"
              disabled={busy != null || !newRepoName.trim()}
              onClick={() => run('create repo', async () => {
                const repo = await githubCreateRepo({
                  name: newRepoName.trim(),
                  private: newRepoPrivate,
                  autoInit: true,
                });
                await prepare(repo.fullName);
              })}
            >
              Create on GitHub + open
            </button>
          </section>
        </div>
      )}

      {(busy || error) && (
        <div className={`tavern-status ${error ? 'error' : ''}`}>
          {error ?? `Working: ${busy}`}
        </div>
      )}
    </section>
  );
}
