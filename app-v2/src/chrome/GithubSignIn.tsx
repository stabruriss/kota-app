import { beginGithubSignIn, useGithubAccounts } from '../lib/github-account-flow';

/** Opens the sign-in dialog (GithubSignInDialog, mounted once in App). */
export function GithubSignIn({ label = 'Sign in', expect = null, disabled = false, className }: {
  label?: string; expect?: string | null; disabled?: boolean; className?: string;
}) {
  const flow = useGithubAccounts();
  return <div className="github-sign-in">
    <button type="button" className={className} disabled={disabled || flow.busy} onClick={() => void beginGithubSignIn(expect)}>{label}</button>
    {flow.error && <p className="tavern-status error" role="alert">{flow.error}</p>}
  </div>;
}
