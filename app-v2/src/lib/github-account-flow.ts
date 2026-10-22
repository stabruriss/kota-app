import { useEffect, useSyncExternalStore } from 'react';
import {
  GITHUB_CLI_INSTALL_URL, GITHUB_CLI_LOGIN_COMMAND, ghAuthStatus,
  githubAccountAddBegin, githubAccountAddFinish, githubAccountRemove,
  githubAccountSetDefault, githubAccountsRead, openExternalUrl,
  type GithubAccounts,
} from '../pty-client';

type PendingAdd = { previousActive: string | null };
interface AccountFlow {
  view: GithubAccounts | null;
  loading: boolean;
  busy: boolean;
  pending: PendingAdd | null;
  error: string | null;
  notice: string | null;
}

// One App-memory flow for Link, Open Project, and the project connection UI.
// Deliberately not persisted: a relaunched App can still explicitly finish a
// sign-in, but can no longer restore the previous gh CLI account.
let state: AccountFlow = { view: null, loading: false, busy: false, pending: null, error: null, notice: null };
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

export function beginGithubSignIn() {
  if (state.pending || state.busy) return Promise.resolve();
  return mutate(async () => {
    const auth = await ghAuthStatus();
    if (auth.cliMissing) { await openExternalUrl(GITHUB_CLI_INSTALL_URL); return; }
    const previousActive = await githubAccountAddBegin();
    publish({ pending: { previousActive }, notice: null });
    window.dispatchEvent(new CustomEvent('kota:smart-run', { detail: { command: GITHUB_CLI_LOGIN_COMMAND } }));
  });
}

/** Explicit user action only. add_finish also restores gh's previous account. */
export function finishGithubSignIn() {
  return mutate(async () => {
    const result = await githubAccountAddFinish(state.pending?.previousActive ?? null);
    publish({ view: result.view, pending: null, notice: result.restoreFailed
      ? `The current gh account is now ${result.active ?? 'unknown'}.` : null });
  });
}

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
