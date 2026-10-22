import { CONNECTABLE_STATES, type ConnectionCandidate, type GithubAccounts } from '../pty-client';
import { GithubDialog } from './GithubDialog';

export const REPOSITORY_STATES = {
  empty: ['Empty repository', 'This repository has no commits. Your local history will be pushed when you connect.'],
  in_sync: ['Already in sync', 'The local and remote histories are identical. This repository is ready to connect.'],
  local_ahead: ['Local is ahead', 'The remote history is already in this project. Connecting will push your newer commits.'],
  remote_ahead: ['Remote is ahead', 'Connect, then use Pull in Bartender to bring in the remote commits.'],
  diverged: ['Merge required', 'Both sides have new commits. Connect, then merge through Bartender before pushing.'],
  unrelated: ['Different history', 'This repository has no shared history with this project. Create a new repository in this account instead.'],
  no_access: ['Access unavailable', 'This account cannot push to this repository. It may be unavailable, archived, or require permission.'],
  already_open: ['Already open in Kota', 'Another Kota project is connected to this repository. Open that project instead.'],
  checking: ['Checking repository…', 'Checking access and shared history.'],
  check_failed: ['Could not check repository', 'The network request failed or timed out. Retry the check.'],
  blocked_large_file: ['First push blocked', "Files in this project's history exceed GitHub's 100 MB limit."],
} as const;
export type RepositoryCheckKind = keyof typeof REPOSITORY_STATES;
export interface ConnectionDraft {
  accountLogin: string;
  mode: 'new' | 'existing';
  name: string;
  private: boolean;
  repoFullName: string;
}
export interface RepositoryCheckView {
  kind: RepositoryCheckKind;
  largeFiles?: { path: string; bytes: number }[];
}
export const repositoryCanConnect = (kind?: RepositoryCheckKind) =>
  kind != null && CONNECTABLE_STATES.some(state => state === kind);

/** View only; probing/committing is owned by the project-scoped controller. */
export function ProjectConnectionDialog({ name, replacing, accounts, draft, repos, reposLoading, check, error, busy, onDraft,
  onRetry, onAddAccount, onContinue, onCancel }: {
  name: string; replacing: boolean; accounts: GithubAccounts | null; draft: ConnectionDraft;
  repos: ConnectionCandidate[]; reposLoading: boolean; check: RepositoryCheckView | null;
  error: string | null; busy: boolean; onDraft: (draft: ConnectionDraft) => void;
  onRetry: () => void; onAddAccount: () => void; onContinue: () => void; onCancel: () => void;
}) {
  const accountReady = accounts?.accounts.some(account => account.login === draft.accountLogin && account.state === 'ok');
  const complete = accountReady && (draft.mode === 'new' ? !!draft.name.trim() : !!draft.repoFullName) && repositoryCanConnect(check?.kind);
  return <GithubDialog title="Connect to GitHub" busy={busy} onCancel={onCancel}>
    <p>{name}{replacing ? ' · Change account' : ''}</p>
    <div className="github-connect-form">
      <div className="github-connect-account"><label>Connect to GH ID
        <select value={draft.accountLogin} disabled={busy} onChange={event => onDraft({ ...draft, accountLogin: event.currentTarget.value })}>
          <option value="" disabled>Choose an account</option>
          {accounts?.accounts.map(account => <option key={account.login} value={account.login} disabled={account.state !== 'ok'}>
            {account.login}{account.default ? ' · Default' : ''}{account.state === 'needs_login' ? ' · Sign in required' : ''}
          </option>)}
        </select>
      </label><button type="button" disabled={busy} onClick={onAddAccount}>+ New</button></div>
      <div className="github-connect-modes" role="group" aria-label="Repository choice">
        <button type="button" aria-pressed={draft.mode === 'new'} disabled={busy} onClick={() => onDraft({ ...draft, mode: 'new' })}>New repository</button>
        <button type="button" aria-pressed={draft.mode === 'existing'} disabled={busy} onClick={() => onDraft({ ...draft, mode: 'existing' })}>Existing repository</button>
      </div>
      {draft.mode === 'new' ? <>
        <label>Repository name<input value={draft.name} disabled={busy} onChange={event => onDraft({ ...draft, name: event.currentTarget.value })} autoComplete="off" spellCheck={false} /></label>
        <label className="github-private"><input type="checkbox" checked={draft.private} disabled={busy} onChange={event => onDraft({ ...draft, private: event.currentTarget.checked })} />Private repository</label>
      </> : <label>Repository<select value={draft.repoFullName} disabled={busy || reposLoading} onChange={event => onDraft({ ...draft, repoFullName: event.currentTarget.value })}>
        <option value="" disabled>{reposLoading ? 'Loading repositories…' : 'Choose a repository'}</option>
        {draft.repoFullName && !repos.some(repo => repo.fullName === draft.repoFullName) && <option value={draft.repoFullName}>{draft.repoFullName}</option>}
        {repos.map(repo => <option key={repo.fullName} value={repo.fullName}>{repo.fullName}{repo.private ? ' · Private' : ''}</option>)}
      </select></label>}
      {check && !(draft.mode === 'new' && check.kind === 'empty') && <div className="github-repo-check" data-state={check.kind} role="status" aria-busy={check.kind === 'checking'}>
        <strong>{REPOSITORY_STATES[check.kind][0]}</strong><p>{REPOSITORY_STATES[check.kind][1]}</p>
        {check.kind === 'blocked_large_file' && <><ul>{check.largeFiles?.map(file => <li key={file.path}><code>{file.path}</code> · {(file.bytes / 1_000_000).toFixed(1)} MB</li>)}</ul><p>Handle these files before connecting.</p></>}
        {check.kind === 'check_failed' && <button type="button" disabled={busy} onClick={onRetry}>Retry</button>}
      </div>}
      {error && <div role="alert" className="github-dialog-error">{error}{check?.kind !== 'check_failed' && <button type="button" disabled={busy} onClick={onRetry}>Retry</button>}</div>}
    </div>
    <div className="kota-confirm-actions"><button type="button" data-dialog-focus disabled={busy} onClick={onCancel}>Cancel</button>
      <button type="button" className="confirm" disabled={busy || !complete} onClick={onContinue}>{draft.mode === 'new' ? 'Create & connect' : 'Connect'}</button></div>
  </GithubDialog>;
}
