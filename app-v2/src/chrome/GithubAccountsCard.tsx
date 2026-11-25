import { useEffect, useState } from 'react';
import {
  beginGithubSignIn, refreshGithubAccounts, removeGithubAccount,
  setDefaultGithubAccount, useGithubAccounts,
} from '../lib/github-account-flow';
import '../styles/github-accounts.css';

/** Whether gh holds a token for the account, as the row's own state says; GitHub is not asked. */
function AccountLight({ signedIn }: { signedIn: boolean }) {
  const label = signedIn ? 'Signed in' : 'Sign in required';
  return <span className={`github-account-light${signedIn ? ' signed-in' : ''}`} role="img" title={label} aria-label={label} />;
}

export function GithubAccountsCard() {
  const flow = useGithubAccounts();
  const [landed, setLanded] = useState<string | null>(null);
  useEffect(() => { void refreshGithubAccounts(); }, []);
  // Once the sign-in dialog closes, its account is pointed at in the list.
  useEffect(() => { if (flow.signIn) setLanded(flow.signIn.phase === 'done' ? flow.signIn.result.login : null); }, [flow.signIn]);
  return <section className="tavern-section full github-accounts-card" aria-label="GitHub accounts" aria-busy={flow.loading || flow.busy}>
    <header className="github-accounts-head">
      <div><div className="tavern-section-title">GitHub</div><p>Accounts for your projects</p></div>
      <button type="button" className="tavern-raw-button github-account-add" disabled={flow.busy} onClick={() => void beginGithubSignIn()}><span className="github-account-add-symbol">+</span> New</button>
    </header>
    <div className="github-account-list">
      {flow.view?.accounts.map(account => <div className={`github-account-row${!flow.signIn && landed === account.login ? ' is-landing' : ''}`} key={account.login}>
        <span className="github-account-avatar" aria-hidden>{account.login[0]?.toUpperCase()}</span>
        <div className="github-account-name"><div className="github-account-name-line"><strong>{account.login}</strong>
          <AccountLight signedIn={account.state === 'ok'} />
          {account.default && <span className="github-account-default">Default</span>}</div>
          <small>gh CLI{account.state === 'needs_login' ? ' · Sign in required' : ''}</small></div>
        <div className="github-account-actions">
          {account.state === 'needs_login' && <button type="button" className="tavern-raw-button" disabled={flow.busy} onClick={() => void beginGithubSignIn(account.login)}>Sign in</button>}
          {!account.default && <button type="button" className="tavern-raw-button" disabled={flow.busy} onClick={() => void setDefaultGithubAccount(account.login)}>Set as default</button>}
          <button type="button" className="tavern-raw-button" disabled={flow.busy} onClick={() => void removeGithubAccount(account.login)}>Remove</button>
        </div>
      </div>)}
      {!flow.view?.accounts.length && <p>{flow.loading ? 'Loading GitHub accounts…' : flow.view ? 'No GitHub accounts yet.' : 'GitHub accounts unavailable.'}</p>}
    </div>
    <div className="github-accounts-footer">
      <p className="github-accounts-hint">The default is used for new projects. Existing project connections are unchanged.</p>
      {flow.error && <p className="tavern-status error" role="alert">{flow.error}</p>}
    </div>
  </section>;
}
