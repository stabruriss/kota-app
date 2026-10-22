import { GithubDialog } from './GithubDialog';

export type ProjectConnectionChange = 'local' | 'connect' | 'account';

export function ProjectConnectionConfirm({ change, name, from, to, login, loginReady, operation, action, busy, error, onCancel, onConfirm }: {
  change: ProjectConnectionChange; name: string; from: string; to: string;
  login?: string; loginReady: boolean; operation?: string; action?: string;
  busy: boolean; error?: string | null; onCancel: () => void; onConfirm: () => void;
}) {
  const title = change === 'local' ? 'Switch to Local only?' : change === 'account' ? 'Change GitHub account?' : `Connect ${name} to GitHub?`;
  const body = change === 'local'
    ? `${name} will stop pulling from and pushing to GitHub. Your files and Git history stay on this computer.`
    : change === 'account' ? 'Only this project will switch accounts. The default account and other projects stay unchanged.'
    : 'Only this project will be connected to GitHub.';
  const warning = change === 'local'
    ? login && loginReady ? `Running agents keep gh access to ${login} until they restart.` : 'Running agents keep any gh access they started with until they restart.'
    : change === 'account' ? 'Git will use the new account immediately. Running agents keep their current gh account until they restart.'
    : 'Git can reach GitHub immediately. Running agents must restart before gh can authenticate.';
  return <GithubDialog title={title} confirm busy={busy} onCancel={onCancel}>
    <p>{body}</p>
    <dl className="github-confirm-transition"><div><dt>From</dt><dd>{from}</dd></div><div><dt>To</dt><dd>{to}</dd></div></dl>
    {operation && <p>{operation}</p>}
    <p className="github-confirm-warning">{warning}</p>
    {error && <p className="github-dialog-error" role="alert">{error}</p>}
    <div className="kota-confirm-actions">
      <button type="button" data-dialog-focus disabled={busy} onClick={onCancel}>Cancel</button>
      <button type="button" className="confirm" disabled={busy} onClick={onConfirm}>{busy ? 'Working…' : action ?? (change === 'local' ? 'Switch to Local only' : change === 'account' ? 'Change account' : 'Connect')}</button>
    </div>
  </GithubDialog>;
}
