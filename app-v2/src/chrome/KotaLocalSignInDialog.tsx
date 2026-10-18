import { useCallback, useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { openExternalUrl } from '../pty-client';
import {
  cancelLocalAuth,
  isLocalResourceUrl,
  localAuthErrorCode,
  localAuthErrorText,
  localAuthMonogram,
  openLocalAuthSession,
  respondLocalAuth,
  type LocalAuthCommit,
  type LocalAuthEvent,
  type LocalAuthNotification,
  type LocalAuthOutcome,
  type LocalAuthPrompt,
  type LocalAuthProvider,
  type LocalAuthType,
} from '../lib/local-auth';

export interface LocalSignInResult {
  outcome: LocalAuthOutcome;
  /** Only 'not-written' lets the dialog say that nothing was saved. */
  commit: LocalAuthCommit;
  method: LocalAuthType | null;
  code?: string;
}

type Item =
  | (LocalAuthNotification & { at: number })
  | { type: 'note'; tone: 'done' | 'closed'; message: string; at: number };

const AZURE_PROVIDER = 'azure';
/** Azure keys also need the resource URL, sent with the login request (D4 addenda, D5 v1.1). */
const needsResource = (providerId: string, method: LocalAuthType | null) => providerId === AZURE_PROVIDER && method === 'api_key';
const methodName = (method: LocalAuthType | null) => (method === 'oauth' ? 'OAuth' : 'API key');

export function LocalAuthLogo({ provider, className }: { provider: LocalAuthProvider; className: string }) {
  return provider.icon
    ? <span className={className} aria-hidden="true"><img src={provider.icon} alt="" /></span>
    : <span className={`${className} mono`} aria-hidden="true">{localAuthMonogram(provider.name)}</span>;
}

/**
 * Viewport-fixed layer below the 48px bar, portaled to body so the Tavern pane's stacking and section
 * styles don't reach it (kota-confirm-layer is not used or changed). While open it owns Escape: Tavern
 * closes itself on Escape at document capture, so the layer takes the key first, at window capture.
 */
export function LocalAuthLayer({ onEscape, children }: { onEscape: () => void; children: ReactNode }) {
  const escape = useRef(onEscape);
  escape.current = onEscape;
  const [opener] = useState(() => (document.activeElement instanceof HTMLElement ? document.activeElement : null));
  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape' || event.isComposing) return;
      event.preventDefault();
      event.stopPropagation();
      escape.current();
    };
    window.addEventListener('keydown', onKey, true);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      if (opener?.isConnected) opener.focus({ preventScroll: true });
    };
  }, [opener]);
  const trapTab = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Tab') return;
    const focusable = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), a[href]'));
    if (!focusable.length) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
  };
  return createPortal(
    <div className="local-auth-layer" role="presentation" onKeyDown={trapTab}>{children}</div>,
    document.body,
  );
}

/** Opens an authorization page in the system browser, only from a user gesture and only for http(s) URLs.
 *  WKWebView ignores window.open, so this goes through Kota's open_external_url; Copy link stays the fallback. */
function openExternal(url: string) {
  if (!/^https?:\/\//i.test(url)) return;
  void openExternalUrl(url).catch(() => {});
}

function hostOf(url: string) {
  try { return new URL(url).host; } catch { return url; }
}

/** A start that Rust refuses before any event still says why when the code is a known one. */
const startErrorCode = (error: unknown) => (/busy-auth/.test(localAuthErrorText(error)) ? 'busy-auth' : undefined);

function resultCopy(result: LocalSignInResult, provider: LocalAuthProvider): { tone: string; title: string; body?: string; retry: boolean } {
  if (result.outcome === 'succeeded') return { tone: 'success', title: 'Linked', body: `${provider.name} · ${methodName(result.method)}.`, retry: false };
  // "Nothing was saved/changed" only when Rust reports nothing written. A stop after the commit began
  // (confirmed or unknown), an unsynchronized write and an unknown result are the approved unverified
  // failure; the row then shows the next fresh status read (v1.4 §1, D5 v1.1).
  if (result.commit !== 'not-written' || result.outcome === 'committed-unsynchronized' || result.outcome === 'indeterminate') {
    return { tone: 'failed', title: 'Could not verify the sign-in. Try again.', retry: true };
  }
  switch (result.outcome) {
    case 'cancelled': return { tone: 'neutral', title: 'Sign-in cancelled', body: 'Nothing was saved.', retry: false };
    case 'timeout': return { tone: 'neutral', title: 'Sign-in timed out', body: 'Nothing was saved.', retry: true };
    case 'interrupted': return { tone: 'failed', title: 'Sign-in stopped', body: 'Kota Local’s sign-in helper exited before finishing. Nothing was saved.', retry: true };
    default:
      if (result.code === 'busy-auth') return { tone: 'failed', title: 'Another sign-in is in progress', body: 'Finish or cancel it first.', retry: true };
      if (result.code === 'storage-failed') return { tone: 'failed', title: 'Couldn’t save the sign-in', body: 'Kota couldn’t write it to its credential store. Nothing was changed.', retry: true };
      return { tone: 'failed', title: 'Sign-in failed', body: 'Nothing was saved.', retry: true };
  }
}

const RESULT_ICON: Record<string, string> = {
  success: 'M8 12.4 10.6 15 16.4 9',
  failed: 'M12 7.5v5.5M12 16.2v.3',
  neutral: 'M8.5 12h7',
};

interface Props {
  provider: LocalAuthProvider;
  /** Known method (Sign in again); otherwise a two-method provider asks first. */
  presetMethod?: LocalAuthType;
  onClose: (result: LocalSignInResult | null) => void;
}

export function KotaLocalSignInDialog({ provider, presetMethod, onClose }: Props) {
  const initialMethod = presetMethod ?? (provider.methods.length === 1 ? provider.methods[0] : null);
  const [method, setMethod] = useState<LocalAuthType | null>(initialMethod);
  const [stage, setStage] = useState<'choose' | 'resource' | 'running'>(
    initialMethod ? (needsResource(provider.id, initialMethod) ? 'resource' : 'running') : 'choose');
  const [choice, setChoice] = useState<LocalAuthType | null>(null);
  const [resourceUrl, setResourceUrl] = useState('');
  const [items, setItems] = useState<Item[]>([]);
  const [prompt, setPrompt] = useState<{ promptId: string; prompt: LocalAuthPrompt } | null>(null);
  // Prompt answers stay in an uncontrolled input: React mirrors controlled values into the value attribute.
  const [filled, setFilled] = useState(false);
  const [chosen, setChosen] = useState<string | null>(null);
  // The prompt whose answer is on its way; its field stays until Rust acknowledges the answer.
  const [sending, setSending] = useState<string | null>(null);
  // Rust refused an answer and kept the native dialog waiting (not a result): why, for that prompt.
  const [refused, setRefused] = useState<{ promptId: string; message: string } | null>(null);
  const [result, setResult] = useState<LocalSignInResult | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const session = useRef<{ operationId: string; close: () => void } | null>(null);
  // Set synchronously on the result event, which can arrive before start returns.
  const settled = useRef(false);
  const starting = useRef(false);
  const cancelAsked = useRef(false);
  const disposed = useRef(false);
  const methodRef = useRef(method);
  const resultRef = useRef<LocalSignInResult | null>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const answerRef = useRef<HTMLInputElement>(null);
  const fieldId = useId();
  methodRef.current = method;
  resultRef.current = result;

  const handleEvent = useCallback((event: LocalAuthEvent) => {
    if (event.event === 'auth_event') {
      setItems((current) => [...current, { ...event.notification, at: Date.now() }]);
    } else if (event.event === 'prompt') {
      setPrompt({ promptId: event.promptId, prompt: event.prompt });
      setFilled(false);
      setChosen(null);
    } else if (event.event === 'prompt_closed') {
      setPrompt((current) => (current?.promptId === event.promptId ? null : current));
      if (event.reason === 'step-aborted') {
        setItems((current) => [...current, { type: 'note', tone: 'closed', message: 'Browser sign-in finished. The paste field closed.', at: Date.now() }]);
      }
    } else if (event.event === 'result') {
      settled.current = true;
      setPrompt(null);
      setCancelling(false);
      // A result without a commit fact is treated as unknown, never as not written.
      setResult({ outcome: event.outcome, commit: event.commit ?? 'unknown', method: methodRef.current, code: event.code });
      session.current?.close();
    }
  }, []);

  const started = useRef(false);
  const begin = useCallback(async (authType: LocalAuthType, url?: string) => {
    started.current = true;
    setStage('running');
    setItems([]);
    setPrompt(null);
    setSending(null);
    setRefused(null);
    setResult(null);
    setCancelling(false);
    settled.current = false;
    cancelAsked.current = false;
    starting.current = true;
    try {
      const open = await openLocalAuthSession(
        { operation: 'login', providerId: provider.id, authType, ...(url ? { resourceUrl: url } : {}) },
        handleEvent,
      );
      starting.current = false;
      if (disposed.current) {
        // Closed while starting: a sign-in never keeps running without its dialog.
        if (!settled.current) void cancelLocalAuth(open.operationId).catch(() => {});
        open.close();
        return;
      }
      session.current = open;
      if (settled.current) open.close();
      else if (cancelAsked.current) void cancelLocalAuth(open.operationId).catch(() => {});
    } catch (error) {
      starting.current = false;
      settled.current = true;
      if (disposed.current) return;
      setCancelling(false);
      // Rust refused the start, so no operation ran and nothing was written.
      setResult({ outcome: 'failed', commit: 'not-written', method: authType, code: startErrorCode(error) });
    }
  }, [handleEvent, provider.id]);

  // Start right away when nothing needs asking first (a known method that needs no Resource URL).
  const [autoStart] = useState(() => (initialMethod && !needsResource(provider.id, initialMethod) ? initialMethod : null));
  useEffect(() => {
    if (started.current || !autoStart) return;
    void begin(autoStart);
  }, [autoStart, begin]);

  // Closing the dialog (or Tavern) mid-flow cancels the sign-in; it never lingers in the background.
  useEffect(() => {
    disposed.current = false;
    return () => {
      disposed.current = true;
      const open = session.current;
      if (!open) return;
      if (!settled.current) void cancelLocalAuth(open.operationId).catch(() => {});
      open.close();
    };
  }, []);

  // Device codes count down until the flow settles.
  const counting = !result && items.some((item) => item.type === 'device_code' && item.expiresInSeconds);
  useEffect(() => {
    if (!counting) return undefined;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [counting]);

  const finish = useCallback(() => {
    session.current?.close();
    onClose(resultRef.current);
  }, [onClose]);

  const cancel = useCallback(async () => {
    if (resultRef.current) { finish(); return; }
    if (cancelling) return;
    if (starting.current) { cancelAsked.current = true; setCancelling(true); return; }
    if (!session.current) { onClose(null); return; }
    setCancelling(true);
    try { await cancelLocalAuth(session.current.operationId); } catch { /* the result event still settles the flow */ }
  }, [cancelling, finish, onClose]);

  const retry = useCallback(() => {
    session.current?.close();
    session.current = null;
    void begin(methodRef.current ?? choice ?? provider.methods[0], resourceUrl.trim() || undefined);
  }, [begin, choice, provider.methods, resourceUrl]);

  const continueChoice = useCallback(() => {
    if (!choice) return;
    setMethod(choice);
    if (needsResource(provider.id, choice)) { setStage('resource'); return; }
    void begin(choice);
  }, [begin, choice, provider.id]);

  const resourceReady = isLocalResourceUrl(resourceUrl);
  const submitResource = useCallback(() => {
    if (!isLocalResourceUrl(resourceUrl)) return;
    void begin('api_key', resourceUrl.trim());
  }, [begin, resourceUrl]);

  const submitPrompt = useCallback(async () => {
    if (!prompt || !session.current || sending === prompt.promptId) return;
    const { promptId, prompt: shape } = prompt;
    const answer = shape.type === 'select' ? chosen : answerRef.current?.value ?? '';
    if (answer == null) return;
    // A blank text answer is valid (e.g. "blank for github.com"); keys and codes are not.
    if (shape.type !== 'select' && shape.type !== 'text' && !answer.trim()) return;
    // A key leaves the page at once; it lives only in this reply, never in React state.
    if (shape.type === 'secret' && answerRef.current) {
      answerRef.current.value = '';
      setFilled(false);
    }
    setSending(promptId);
    setRefused((current) => (current?.promptId === promptId ? null : current));
    try {
      await respondLocalAuth({ operationId: session.current.operationId, promptId, value: answer });
      // Acknowledged: drop this field only, never a newer prompt that arrived before the ACK.
      setPrompt((current) => (current?.promptId === promptId ? null : current));
      if (shape.type === 'secret') {
        setItems((current) => [...current, { type: 'note', tone: 'done', message: 'Key received. It is not shown again.', at: Date.now() }]);
      }
    } catch (error) {
      // Not a result: the native dialog still waits, so the field stays to be answered again.
      // prompt_closed and result remain authoritative.
      setRefused({
        promptId,
        message: localAuthErrorCode(error) === 'credential_invalid'
          ? 'Paste the key itself. Commands and $VARIABLES aren’t accepted.'
          : 'Couldn’t submit this answer. Try again.',
      });
    } finally {
      setSending((current) => (current === promptId ? null : current));
    }
  }, [chosen, prompt, sending]);

  const promptKey = prompt?.promptId ?? null;
  const done = !!result;
  const refusal = refused && refused.promptId === promptKey ? refused.message : null;
  const sendingNow = !!promptKey && sending === promptKey;
  useEffect(() => {
    if (refusal) answerRef.current?.focus({ preventScroll: true });
  }, [refusal]);
  useEffect(() => {
    const card = cardRef.current;
    if (!card) return;
    const target = card.querySelector<HTMLElement>('input, .local-auth-option, button.confirm:not(:disabled)') ?? card;
    target.focus({ preventScroll: true });
  }, [stage, promptKey, done]);

  const copy = (key: string, text: string) => {
    void navigator.clipboard?.writeText(text).catch(() => {});
    setCopied(key);
    window.setTimeout(() => setCopied((current) => (current === key ? null : current)), 1100);
  };

  const renderItem = (item: Item, index: number) => {
    if (item.type === 'note' && item.tone === 'closed') return <div key={index} className="local-auth-closed">{item.message}</div>;
    if (item.type === 'progress' || item.type === 'note') {
      // Only the latest step spins; anything after it (or the result) settles it.
      const settledStep = item.type === 'note' || index !== items.length - 1 || !!result;
      return (
        <div key={index} className={`local-auth-progress${settledStep ? ' done' : ''}`}>
          <span className="local-auth-spinner" aria-hidden="true" /><span>{item.message}</span>
        </div>
      );
    }
    if (item.type === 'info') {
      return (
        <div key={index} className="local-auth-info" role="note">
          {item.message}
          {(item.links ?? []).map((link) => (
            <button key={link.url} type="button" className="local-auth-link" onClick={() => openExternal(link.url)}>
              {link.label ?? hostOf(link.url)}
            </button>
          ))}
        </div>
      );
    }
    if (item.type === 'auth_url') {
      return (
        <div key={index} className="local-auth-step">
          <div className="local-auth-step-title">Continue in your browser</div>
          {item.instructions && <p>{item.instructions}</p>}
          <div className="local-auth-step-actions">
            <button type="button" className="local-auth-button primary" onClick={() => openExternal(item.url)}>Open browser</button>
            <button type="button" className="local-auth-button" onClick={() => copy(`url-${index}`, item.url)}>
              {copied === `url-${index}` ? 'Copied' : 'Copy link'}
            </button>
          </div>
          <code className="local-auth-url" title={item.url}>{item.url}</code>
        </div>
      );
    }
    const elapsed = Math.max(0, Math.floor((now - item.at) / 1000));
    const left = item.expiresInSeconds ? Math.max(0, item.expiresInSeconds - elapsed) : null;
    return (
      <div key={index} className="local-auth-step local-auth-device">
        <div className="local-auth-device-label">Enter this code</div>
        <div className="local-auth-device-code">{item.userCode}</div>
        <div className="local-auth-step-actions">
          <button type="button" className="local-auth-button primary" onClick={() => copy(`code-${index}`, item.userCode)}>
            {copied === `code-${index}` ? 'Copied' : 'Copy code'}
          </button>
          <button type="button" className="local-auth-button" onClick={() => openExternal(item.verificationUri)}>
            Open {hostOf(item.verificationUri)}
          </button>
        </div>
        {left != null && <div className="local-auth-device-expiry">Expires in {Math.floor(left / 60)}:{String(left % 60).padStart(2, '0')}</div>}
      </div>
    );
  };

  const options = (message: string, list: Array<{ id: string; label: string; description?: string }>, selected: string | null, pick: (id: string) => void) => (
    <fieldset className="local-auth-options" role="radiogroup">
      <legend>{message}</legend>
      {list.map((option) => (
        <button key={option.id} type="button" role="radio" aria-checked={selected === option.id}
          className="local-auth-option" onClick={() => pick(option.id)}>
          <span><b>{option.label}</b>{option.description && <small>{option.description}</small>}</span>
        </button>
      ))}
    </fieldset>
  );

  let body: ReactNode = null;
  let primary: { label: string; disabled: boolean; run: () => void } | null = null;
  if (stage === 'choose') {
    body = options('How do you want to sign in?', provider.methods.map((m) => ({
      id: m,
      label: (m === 'oauth' ? provider.oauthLabel : provider.apiKeyLabel) ?? methodName(m),
      description: m === 'oauth' ? 'Continue in your browser' : 'Paste a key',
    })), choice, (id) => setChoice(id as LocalAuthType));
    primary = { label: 'Continue', disabled: !choice, run: continueChoice };
  } else if (stage === 'resource') {
    body = (
      <label className="local-auth-field">
        <span>Resource URL</span>
        <input type="text" value={resourceUrl} placeholder="https://<resource>.openai.azure.com" autoComplete="off" spellCheck={false}
          onChange={(event) => setResourceUrl(event.target.value)}
          onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); submitResource(); } }} />
      </label>
    );
    primary = { label: 'Continue', disabled: !resourceReady, run: submitResource };
  } else {
    const shape = prompt?.prompt;
    const last = items[items.length - 1];
    // Device codes and browser links without a paste field wait on the browser; say so while nothing else moves.
    const waiting = !result && !shape && (last?.type === 'device_code' || last?.type === 'auth_url');
    body = (
      <>
        {items.map(renderItem)}
        {waiting && (
          <div className="local-auth-progress">
            <span className="local-auth-spinner" aria-hidden="true" /><span>Waiting for you to confirm in the browser…</span>
          </div>
        )}
        {shape && shape.type === 'select' && options(shape.message, shape.options, chosen, setChosen)}
        {shape && shape.type === 'select' && refusal && <div className="local-auth-field-error" role="alert">{refusal}</div>}
        {shape && shape.type !== 'select' && (
          <label className="local-auth-field">
            <span id={`${fieldId}-label`}>{shape.message}</span>
            <input key={promptKey} ref={answerRef} type={shape.type === 'secret' ? 'password' : 'text'} defaultValue=""
              aria-labelledby={`${fieldId}-label`}
              aria-describedby={[refusal ? `${fieldId}-error` : '', shape.type === 'secret' ? `${fieldId}-note` : ''].filter(Boolean).join(' ') || undefined}
              disabled={sendingNow} placeholder={shape.placeholder} autoComplete="off" spellCheck={false}
              onChange={(event) => setFilled(!!event.target.value.trim())}
              onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void submitPrompt(); } }} />
            {refusal && <span id={`${fieldId}-error`} className="local-auth-field-error" role="alert">{refusal}</span>}
            {shape.type === 'secret' && <span id={`${fieldId}-note`} className="local-auth-field-note">Saved locally.</span>}
          </label>
        )}
      </>
    );
    if (shape && !result) {
      const empty = shape.type === 'select' ? !chosen : shape.type !== 'text' && !filled;
      primary = { label: shape.type === 'secret' ? 'Save key' : shape.type === 'manual_code' ? 'Submit' : 'Continue', disabled: empty || cancelling || sendingNow, run: () => void submitPrompt() };
    }
  }

  const copyOf = result ? resultCopy(result, provider) : null;
  return (
    <LocalAuthLayer onEscape={() => void cancel()}>
      <div ref={cardRef} className="kota-confirm-card local-auth-dialog" role="dialog" aria-modal="true"
        aria-label={`Sign in to ${provider.name}`} tabIndex={-1}>
        <div className="local-auth-dialog-head">
          <LocalAuthLogo provider={provider} className="local-auth-dialog-logo" />
          <div>
            <h2>Sign in · {provider.name}</h2>
            <div className="local-auth-dialog-sub">Kota Local{method ? ` · ${methodName(method)}` : ''}</div>
          </div>
        </div>
        <div className="local-auth-body" aria-live="polite">
          {body}
          {copyOf && (
            <div className={`local-auth-result ${copyOf.tone}`} role="status">
              <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d={RESULT_ICON[copyOf.tone]} /></svg>
              <div>
                <b>{copyOf.title}</b>
                {copyOf.body && <p>{copyOf.body}</p>}
                {result?.code && <code>{result.code}</code>}
              </div>
            </div>
          )}
        </div>
        <div className="kota-confirm-actions">
          {result ? (
            <>
              {result.outcome === 'succeeded'
                ? <button type="button" className="confirm local-auth-done" onClick={finish}>Done</button>
                : <button type="button" onClick={finish}>Close</button>}
              {copyOf?.retry && <button type="button" className="confirm" onClick={retry}>Try again</button>}
            </>
          ) : (
            <>
              <button type="button" disabled={cancelling} onClick={() => void cancel()}>{cancelling ? 'Cancelling…' : 'Cancel'}</button>
              {primary && <button type="button" className="confirm" disabled={primary.disabled} onClick={primary.run}>{primary.label}</button>}
            </>
          )}
        </div>
      </div>
    </LocalAuthLayer>
  );
}
