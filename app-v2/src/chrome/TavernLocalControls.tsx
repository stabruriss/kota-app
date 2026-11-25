import { requestKotaLocalLink } from '../lib/local-auth';
import type { TavernLocalState } from '../lib/tavern-local';
import '../styles/tavern-local.css';

export const TAVERN_LOCAL_LABELS: Record<TavernLocalState, string> = {
  ready: 'Ready',
  not_ready: 'Not signed in',
  checking: 'Checking sign-in',
  invalid: 'Sign-in needs attention',
  error: 'Status unavailable',
};

export function TavernLocalStatus({ state, onRetry, onSignIn }: { state: TavernLocalState; onRetry: () => void; onSignIn?: () => void }) {
  return (
    <div className="tavern-local-auth-note" data-auth={state} role="status">
      <span className="tavern-local-auth-dot" aria-hidden="true" />
      <span>{TAVERN_LOCAL_LABELS[state]}</span>
      <TavernLocalAction state={state} onRetry={onRetry} onSignIn={onSignIn} />
    </div>
  );
}

/** Sign in opens the Local card in Link. A page that must ask before it is left passes its own `onSignIn`. */
export function TavernLocalAction({ state, onRetry, onSignIn }: { state: TavernLocalState; onRetry: () => void; onSignIn?: () => void }) {
  if (state === 'error') return <button type="button" onClick={onRetry}>Retry</button>;
  if (state === 'not_ready' || state === 'invalid') {
    return <button type="button" onClick={() => onSignIn ? onSignIn() : requestKotaLocalLink('kota-local')}>Sign in <span aria-hidden="true">→</span></button>;
  }
  return null;
}
