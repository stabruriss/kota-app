import { useEffect } from 'react';
import {
  beginGithubSignIn, refreshGithubAccounts, removeGithubAccount,
  setDefaultGithubAccount, useGithubAccounts,
} from '../lib/github-account-flow';
import { GithubSignIn } from './GithubSignIn';
import '../styles/github-accounts.css';

export function GithubAccountsCard() {
  const flow = useGithubAccounts();
  useEffect(() => { void refreshGithubAccounts(); }, []);
  return <section className="tavern-section full github-accounts-card" aria-label="GitHub accounts" aria-busy={flow.loading || flow.busy}>
    <header className="github-accounts-head">
      <div><div className="tavern-section-title">GitHub</div><p>Accounts for your projects</p></div>
      <button type="button" disabled={flow.busy || !!flow.pending} onClick={() => void beginGithubSignIn()}>+ New</button>
    </header>
    <div className="github-account-list">
      {flow.view?.accounts.map(account => <div className="github-account-row" key={account.login}>
        <span className="github-account-avatar" aria-hidden>{account.login[0]?.toUpperCase()}</span>
        <div className="github-account-name"><strong>{account.login}</strong>
          <small>gh CLI{account.state === 'needs_login' ? ' · Sign in required' : ''}</small></div>
        {account.default && <span className="github-account-default">Default</span>}
        <div className="github-account-actions">
          {account.state === 'needs_login' && <button type="button" disabled={flow.busy || !!flow.pending} onClick={() => void beginGithubSignIn()}>Sign in</button>}
          {!account.default && <button type="button" disabled={flow.busy} onClick={() => void setDefaultGithubAccount(account.login)}>Set as default</button>}
          <button type="button" disabled={flow.busy} onClick={() => void removeGithubAccount(account.login)}>Remove</button>
        </div>
      </div>)}
      {!flow.view?.accounts.length && <p>{flow.loading ? 'Loading GitHub accounts…' : flow.view ? 'No GitHub accounts yet.' : 'GitHub accounts unavailable.'}</p>}
    </div>
    <p className="github-accounts-hint">The default is used for new projects. Existing project connections are unchanged.</p>
    <GithubSignIn showStart={false} />
    <button type="button" className="local-model-text" disabled={flow.loading || flow.busy} onClick={() => void refreshGithubAccounts()}>Refresh accounts</button>
  </section>;
}
