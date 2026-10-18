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

export function TavernLocalStatus({ state, onRetry }: { state: TavernLocalState; onRetry: () => void }) {
  return (
    <div className="tavern-local-auth-note" data-auth={state} role="status">
      <span className="tavern-local-auth-dot" aria-hidden="true" />
      <span>{TAVERN_LOCAL_LABELS[state]}</span>
      <TavernLocalAction state={state} onRetry={onRetry} />
    </div>
  );
}

export function TavernLocalAction({ state, onRetry }: { state: TavernLocalState; onRetry: () => void }) {
  if (state === 'error') return <button type="button" onClick={onRetry}>Retry</button>;
  if (state === 'not_ready' || state === 'invalid') {
    return <button type="button" onClick={() => requestKotaLocalLink('kota-local')}>Sign in <span aria-hidden="true">→</span></button>;
  }
  return null;
}
