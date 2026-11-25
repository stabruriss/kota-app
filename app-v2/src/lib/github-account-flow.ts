import { useEffect, useSyncExternalStore } from 'react';
import {
  GITHUB_CLI_INSTALL_URL, ghAuthStatus, githubAccountRemove, githubAccountSetDefault, githubAccountsRead,
  githubSignInCancel, githubSignInStart, onGithubSignIn, openExternalUrl,
  type GithubAccounts, type GithubSignInEvent, type GithubSignInResult,
} from '../pty-client';

/** One sign-in, shown in one dialog whichever entry point started it. `expect` is the account
 *  that needs to sign in again, if it is about one. */
export type GithubSignIn = { expect: string | null } & (
  /** Step 1: nothing has started; the person signs in to GitHub in their browser. */
  | { phase: 'account' }
  /** Step 2: gh is running, so its code works. No deadline until it has printed something. */
  | { phase: 'running'; output: string; deadline: number | null }
  | { phase: 'failed'; output: string }
  | { phase: 'timed_out' }
  | { phase: 'cli_missing' }
  | { phase: 'done'; outcome: 'added' | 'signed_in' | 'already' | 'different'; result: GithubSignInResult }
);
interface AccountFlow {
  view: GithubAccounts | null;
  loading: boolean;
  busy: boolean;
  signIn: GithubSignIn | null;
  error: string | null;
}

// One App-memory flow for Link, Open Project, and the project connection UI.
let state: AccountFlow = { view: null, loading: false, busy: false, signIn: null, error: null };
const listeners = new Set<() => void>();
let generation = 0;
let reading: Promise<void> | null = null;
const publish = (patch: Partial<AccountFlow>) => {
  state = { ...state, ...patch };
  listeners.forEach(listener => listener());
};
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
const snapshot = () => state;

export function useGithubAccounts(enabled = true) {
  const current = useSyncExternalStore(subscribe, snapshot);
  useEffect(() => {
    if (enabled && state.view === null && !state.error) void refreshGithubAccounts();
  }, [enabled]);
  return current;
}

export function refreshGithubAccounts(): Promise<void> {
  if (state.busy) return Promise.resolve();
  if (reading) return reading;
  const version = generation;
  publish({ loading: true, error: null });
  const task = Promise.resolve().then(githubAccountsRead).then(view => {
    if (version === generation) publish({ view });
  }).catch(error => {
    if (version === generation) publish({ error: String(error) });
  }).finally(() => {
    if (reading === task) reading = null;
    if (version === generation) publish({ loading: false });
  });
  reading = task;
  return task;
}

async function mutate(action: () => Promise<void>): Promise<void> {
  if (state.busy) return;
  ++generation;
  reading = null;
  publish({ busy: true, loading: false, error: null });
  try { await action(); } catch (error) { publish({ error: String(error) }); }
  finally { publish({ busy: false }); }
}

// The gh the dialog is waiting for, if any.
let session: string | null = null;
function show(signIn: GithubSignIn | null) {
  if (session) void githubSignInCancel(session);
  session = null;
  publish({ signIn });
}

/** Step 1. Without gh there is no point in signing in to GitHub first, so that is said at once. */
export async function beginGithubSignIn(expect: string | null = null) {
  const step: GithubSignIn = { expect, phase: 'account' };
  show(step);
  const auth = await ghAuthStatus();
  if (auth.cliMissing && state.signIn === step) show({ expect, phase: 'cli_missing' });
}

/** Step 2, and a new code after gh stopped. gh starts here, not in Step 1: its code should not
 *  age while the person signs in to GitHub. */
export async function continueGithubSignIn() {
  const expect = state.signIn?.expect ?? null;
  const before = state.view;
  show({ expect, phase: 'running', output: '', deadline: null });
  const id = crypto.randomUUID();
  session = id;
  const end = (signIn: GithubSignIn) => { if (session === id) { session = null; publish({ signIn }); } };
  // Listening first: gh may stop before start even returns.
  const unlisten = await onGithubSignIn(event => {
    if (event.id !== id) return;
    if (event.kind === 'output') {
      if (session === id) publish({ signIn: { expect, phase: 'running', output: event.output, deadline: event.deadlineMs } });
      return;
    }
    unlisten();
    if (event.kind === 'signed_in') {
      // Even after Cancel: gh can finish just before it is stopped, and the account is there then.
      ++generation; reading = null;
      publish({ view: event.result.view, loading: false });
    }
    const ended = endOf(event, expect, before);
    if (ended) end(ended);
  });
  if (session !== id) { unlisten(); return; }
  try {
    await githubSignInStart(id);
  } catch (error) {
    unlisten();
    end(String(error).includes('github_cli_missing') ? { expect, phase: 'cli_missing' } : { expect, phase: 'failed', output: String(error) });
    return;
  }
  // Cancelled while it was starting, when there was nothing to cancel yet.
  if (session !== id) void githubSignInCancel(id);
}

function endOf(event: GithubSignInEvent, expect: string | null, before: GithubAccounts | null): GithubSignIn | null {
  if (event.kind === 'failed') return { expect, phase: 'failed', output: event.output };
  if (event.kind === 'timed_out') return { expect, phase: 'timed_out' };
  if (event.kind !== 'signed_in') return null;
  const { login, added } = event.result;
  const signedOut = before?.accounts.some(account => account.login === login && account.state === 'needs_login');
  const outcome = expect ? (login === expect ? 'signed_in' : 'different') : added ? 'added' : signedOut ? 'signed_in' : 'already';
  return { expect, phase: 'done', outcome, result: event.result };
}

/** Another account means signing in again in the browser, so back to Step 1; otherwise a new code is enough. */
export function retryGithubSignIn() {
  const signIn = state.signIn;
  if (signIn?.phase === 'failed' || signIn?.phase === 'timed_out') void continueGithubSignIn();
  else void beginGithubSignIn(signIn?.expect ?? null);
}
export const closeGithubSignIn = () => show(null);
export const installGithubCli = () => openExternalUrl(GITHUB_CLI_INSTALL_URL);

export const setDefaultGithubAccount = (login: string) => mutate(async () => {
  publish({ view: await githubAccountSetDefault(login) });
});
export const removeGithubAccount = (login: string) => mutate(async () => {
  publish({ view: await githubAccountRemove(login) });
});

export const GITHUB_LINK_EVENT = 'kota:github-link';
export function openGithubAccounts() {
  window.dispatchEvent(new CustomEvent(GITHUB_LINK_EVENT));
}
