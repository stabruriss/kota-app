import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import flame from '../assets/tavern/icons/tavern-entry.svg';
import { loadProjectAgentDetail } from '../pty-client';
import {
  LOCAL_AUTH_MAIN,
  LOCAL_AUTH_MORE,
  LOCAL_AUTH_PROVIDERS,
  openLocalAuthSession,
  readLocalAuthAvailability,
  readLocalAuthInUse,
  requestKotaLocalLink,
  type KotaLocalLinkTarget,
  type LocalAuthAvailability,
  type LocalAuthProvider,
  type LocalAuthType,
} from '../lib/local-auth';
import { KotaLocalSignInDialog, LocalAuthLayer, LocalAuthLogo, type LocalSignInResult } from './KotaLocalSignInDialog';
import { KotaLocalMagpieRow } from './KotaLocalMagpieRow';
import '../styles/kota-local-link.css';

type RowView = 'off' | 'api_key' | 'oauth' | 'invalid' | 'checking' | 'error' | 'unknown' | 'busy' | 'signing_out';
const ROW: Record<RowView, { dot: string; text: string; action: string }> = {
  off: { dot: 'off', text: 'Not linked', action: 'Sign in' },
  api_key: { dot: 'ok', text: 'Linked · API key', action: 'Sign out' },
  oauth: { dot: 'ok', text: 'Linked · OAuth', action: 'Sign out' },
  invalid: { dot: 'missing', text: 'Needs to sign in again', action: 'Sign in again' },
  checking: { dot: 'checking', text: 'Checking…', action: 'Checking…' },
  // availability `error` for one provider: no trustworthy answer, Retry only (v1.4 §2).
  error: { dot: 'setup', text: 'Status unavailable', action: 'Retry' },
  // the whole read failed: rows say so instead of falling back to Not linked.
  unknown: { dot: 'setup', text: 'Status unavailable', action: '' },
  busy: { dot: 'checking', text: 'Signing in…', action: 'Signing in…' },
  signing_out: { dot: 'checking', text: 'Signing out…', action: 'Signing out…' },
};
const MOVING: readonly RowView[] = ['busy', 'checking', 'signing_out'];
const methodsLabel = (provider: LocalAuthProvider) =>
  provider.methods.map((method) => (method === 'oauth' ? 'OAuth' : 'API key')).join(' · ');
interface Props {
  /** Landing target from D7 / Sign in links (Tavern switches to Link first). */
  focusTarget?: KotaLocalLinkTarget | null;
  onFocusHandled?: () => void;
}

export function KotaLocalLinkCard({ focusTarget, onFocusHandled }: Props) {
  const [availability, setAvailability] = useState<LocalAuthAvailability | null>(null);
  const [readState, setReadState] = useState<'reading' | 'ready' | 'failed'>('reading');
  const [overlay, setOverlay] = useState<Record<string, RowView>>({});
  const [signIn, setSignIn] = useState<{ provider: LocalAuthProvider; method?: LocalAuthType } | null>(null);
  const [signOut, setSignOut] = useState<{ provider: LocalAuthProvider; agents: string[] } | null>(null);
  const [moreOpen, setMoreOpen] = useState(false);
  const [magpieLinked, setMagpieLinked] = useState(false);
  const [filter, setFilter] = useState('');
  const readSeq = useRef(0);
  const moreDecided = useRef(false);
  const preparingSignOut = useRef(false);
  const mounted = useRef(true);
  const landingTimer = useRef<number | null>(null);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (landingTimer.current) window.clearTimeout(landingTimer.current);
    };
  }, []);

  // The one quick retry happens inside the availability IPC; the card only
  // projects credential status. Model selection belongs to each Local agent.
  const refresh = useCallback(() => {
    const seq = ++readSeq.current;
    const current = () => mounted.current && seq === readSeq.current;
    setReadState('reading');
    void readLocalAuthAvailability().then((next) => {
      if (!current()) return;
      // A global failure is never an empty list read as "nothing linked" (v1.4 §2).
      if (next.summary === 'error' && next.providers.length === 0) { setReadState('failed'); return; }
      setAvailability(next);
      setReadState('ready');
    }, () => { if (current()) setReadState('failed'); });
  }, []);

  useEffect(() => { refresh(); }, [refresh]);

  const viewOf = useCallback((id: string): RowView => {
    const local = overlay[id];
    if (local) return local;
    if (readState === 'failed') return 'unknown';
    if (!availability) return 'checking';
    const entry = availability.providers.find((provider) => provider.providerId === id);
    switch (entry?.status) {
      case 'signed_in': return entry.authType === 'oauth' ? 'oauth' : 'api_key';
      case 'invalid': return 'invalid';
      case 'checking': return 'checking';
      case 'error': return 'error';
      default: return 'off';
    }
  }, [availability, overlay, readState]);

  const views = useMemo(() => new Map(LOCAL_AUTH_PROVIDERS.map((provider) => [provider.id, viewOf(provider.id)])), [viewOf]);
  // One sign-in or sign-out at a time; rows waiting on their own read are not a flow.
  const flowing = Object.keys(overlay).length > 0;

  // More opens by itself once, when a provider in it already has a state worth seeing.
  useEffect(() => {
    if (moreDecided.current || !availability) return;
    moreDecided.current = true;
    if (LOCAL_AUTH_MORE.some((provider) => views.get(provider.id) !== 'off')) setMoreOpen(true);
  }, [availability, views]);

  useEffect(() => {
    if (!focusTarget) return;
    const target = document.getElementById('link-kota-local');
    if (!target) return;
    target.scrollIntoView?.({ block: 'start', behavior: 'smooth' });
    target.focus({ preventScroll: true });
    target.classList.remove('is-landing');
    void target.offsetWidth;
    target.classList.add('is-landing');
    if (landingTimer.current) window.clearTimeout(landingTimer.current);
    landingTimer.current = window.setTimeout(() => target.classList.remove('is-landing'), 2400);
    onFocusHandled?.();
  }, [focusTarget, onFocusHandled]);

  const openSignOut = useCallback(async (provider: LocalAuthProvider) => {
    if (preparingSignOut.current) return;
    preparingSignOut.current = true;
    let agents: string[] = [];
    try {
      const inUse = await readLocalAuthInUse(provider.id);
      agents = await Promise.all(inUse.map(async ({ agentId, projectRoot }) => {
        try { return (await loadProjectAgentDetail({ agentId, projectRoot })).displayName || agentId; } catch { return agentId; }
      }));
    } catch { /* the list is informational; sign-out still works */ }
    preparingSignOut.current = false;
    if (mounted.current) setSignOut({ provider, agents });
  }, []);

  const confirmSignOut = useCallback(async () => {
    const target = signOut;
    if (!target) return;
    setSignOut(null);
    const id = target.provider.id;
    setOverlay((current) => ({ ...current, [id]: 'signing_out' }));
    // The result can arrive before start returns (held by the session), so settle through flags.
    let settled = false;
    let close: (() => void) | null = null;
    const done = () => {
      setOverlay((current) => { const next = { ...current }; delete next[id]; return next; });
      refresh();
    };
    try {
      const session = await openLocalAuthSession({ operation: 'logout', providerId: id }, (event) => {
        if (event.event !== 'result' || settled) return;
        settled = true;
        close?.();
        done();
      });
      close = session.close;
      if (settled) session.close();
    } catch { done(); }
  }, [refresh, signOut]);

  const beginSignIn = (provider: LocalAuthProvider, method?: LocalAuthType) => {
    setOverlay((current) => ({ ...current, [provider.id]: 'busy' }));
    setSignIn({ provider, method });
  };
  const onRow = (provider: LocalAuthProvider, view: RowView) => {
    if (view === 'off') beginSignIn(provider);
    else if (view === 'invalid') {
      beginSignIn(provider, availability?.providers.find((entry) => entry.providerId === provider.id)?.authType);
    } else if (view === 'api_key' || view === 'oauth') void openSignOut(provider);
    else if (view === 'error') refresh();
  };

  const signingIn = signIn?.provider.id;
  const closeSignIn = useCallback((result: LocalSignInResult | null) => {
    setSignIn(null);
    if (signingIn) setOverlay((rows) => { const next = { ...rows }; delete next[signingIn]; return next; });
    // Any finished attempt, including a failed or unverified one, shows the fresh status (v1.4 §1).
    if (result) refresh();
  }, [refresh, signingIn]);

  const row = (provider: LocalAuthProvider) => {
    const view = views.get(provider.id) ?? 'checking';
    const copy = ROW[view];
    const own = MOVING.includes(view);
    const busyElsewhere = flowing && !own;
    return (
      <article key={provider.id} className="local-auth-row" id={`kota-local-provider-${provider.id}`}
        data-provider={provider.id} data-state={view} aria-label={provider.name}>
        <LocalAuthLogo provider={provider} className="tavern-connection-logo local-auth-logo" />
        <div className="local-auth-main">
          <div className="local-auth-name"><b>{provider.name}</b></div>
          <div className="local-auth-status">
            <span className={`tavern-dot ${copy.dot}`} aria-hidden="true" />
            <span className="local-auth-status-text">{copy.text}</span>
            {view === 'off' && <span className="local-auth-methods">{methodsLabel(provider)}</span>}
          </div>
        </div>
        {copy.action && (
          <div className="local-auth-actions">
            <button type="button" className={`local-auth-action${view === 'api_key' || view === 'oauth' ? ' quiet' : ''}`}
              disabled={own || busyElsewhere} title={busyElsewhere ? 'Another sign-in is in progress' : undefined}
              onClick={() => onRow(provider, view)}>
              {copy.action}
            </button>
          </div>
        )}
      </article>
    );
  };

  // Summary line and dot.
  const linked = [...views.values()].filter((view) => view === 'api_key' || view === 'oauth').length + Number(magpieLinked);
  const attention = [...views.values()].filter((view) => view === 'invalid' || view === 'error').length;
  let summary = linked ? `${linked} provider${linked > 1 ? 's' : ''} linked` : 'No provider linked';
  if (attention) summary += ` · ${attention} need${attention > 1 ? '' : 's'} attention`;
  if (readState === 'failed') summary = 'Status unavailable';
  else if (!availability) summary = 'Checking…';
  const reading = readState === 'reading' && !availability;
  const dot = reading || flowing ? 'checking' : linked && !attention && readState !== 'failed' ? 'ok' : 'setup';

  const filterText = filter.trim().toLowerCase();
  const moreRows = LOCAL_AUTH_MORE.filter((provider) => !filterText
    || provider.id.includes(filterText) || provider.name.toLowerCase().includes(filterText));

  return (
    <section className="tavern-section full tavern-local-auth" id="link-kota-local" tabIndex={-1}
      aria-label="Kota Local sign-ins" data-testid="tavern-kota-local">
      <div className="tavern-card-head">
        <div className="tavern-connection-title">
          <span className="tavern-connection-logo kota-local" aria-hidden="true"><img src={flame} alt="" /></span>
          <div>
            <div className="tavern-section-title">Kota Local</div>
            <div className="tavern-identity">
              <span data-testid="kota-local-summary">{summary}</span>
              {readState === 'failed' && (
                <button type="button" className="local-auth-action local-auth-retry" onClick={refresh}>Retry</button>
              )}
            </div>
          </div>
        </div>
        <span className={`tavern-dot ${dot}`} aria-hidden="true" />
      </div>
      <div className="tavern-connection-copy">
        Kota Local Agent — Kota&apos;s embedded agent harness, based on Pi, with some unique beta features. Use it in Kota alongside Codex, Claude Code, and others.
      </div>
      <div className="local-auth-policy">Configure LLM providers to use Kota Local Agent.</div>
      <div className="local-auth-policy">Jev is configured in Puppeteer. <button type="button" className="local-model-text" onClick={() => requestKotaLocalLink('puppeteer-jev')}>Open Puppeteer</button></div>

      <div className="local-auth-grid">{LOCAL_AUTH_MAIN.map(row)}<KotaLocalMagpieRow onLinked={setMagpieLinked} /></div>
      <details className="local-auth-more" open={moreOpen} onToggle={(event) => setMoreOpen(event.currentTarget.open)}>
        <summary><span>More providers</span><span className="local-auth-count">{LOCAL_AUTH_MORE.length}</span></summary>
        <label className="local-auth-filter">
          <input type="search" value={filter} placeholder="Filter providers" aria-label="Filter providers" autoComplete="off"
            onChange={(event) => setFilter(event.target.value)} />
        </label>
        <div className="local-auth-grid">{moreRows.map(row)}</div>
      </details>

      {signIn && <KotaLocalSignInDialog provider={signIn.provider} presetMethod={signIn.method} onClose={closeSignIn} />}
      {signOut && (
        <LocalAuthLayer onEscape={() => setSignOut(null)}>
          <div className="kota-confirm-card danger" role="alertdialog" aria-modal="true" aria-label={`Sign out of ${signOut.provider.name}`}>
            <h2>Sign out of {signOut.provider.name}?</h2>
            {signOut.agents.length > 0 && (
              <div className="local-auth-signout-users">
                <span>Local agents using {signOut.provider.name}:</span>
                {signOut.agents.map((name, index) => <b key={`${name}-${index}`}>{name}</b>)}
              </div>
            )}
            <pre>{signOut.agents.length
              ? 'After you sign out, remember to change their model.'
              : `Kota Local agents can’t use ${signOut.provider.name} until you sign in again.`}</pre>
            <div className="kota-confirm-actions">
              <button type="button" autoFocus onClick={() => setSignOut(null)}>Cancel</button>
              <button type="button" className="confirm" onClick={() => void confirmSignOut()}>Sign out</button>
            </div>
          </div>
        </LocalAuthLayer>
      )}
    </section>
  );
}
