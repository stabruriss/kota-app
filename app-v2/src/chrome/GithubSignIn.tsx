import { beginGithubSignIn, finishGithubSignIn, useGithubAccounts } from '../lib/github-account-flow';

/** Both entry points share the pending login and the same explicit finish. */
export function GithubSignIn({ label = 'Sign in', disabled = false, showStart = true }: { label?: string; disabled?: boolean; showStart?: boolean }) {
  const flow = useGithubAccounts();
  return <div className="github-sign-in">
    {flow.pending ? <p role="status">Complete sign-in in Smart Terminal.</p>
      : showStart && <button type="button" disabled={disabled || flow.busy} onClick={() => void beginGithubSignIn()}>{label}</button>}
    <button type="button" disabled={disabled || flow.busy} onClick={() => void finishGithubSignIn()}>Check sign-in</button>
    {flow.notice && <p role="status">{flow.notice}</p>}
    {flow.error && <p className="tavern-status error" role="alert">{flow.error}</p>}
  </div>;
}
