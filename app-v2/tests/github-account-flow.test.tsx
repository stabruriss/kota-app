import { act, cleanup, fireEvent, render, renderHook, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import * as ipc from '../src/pty-client';
import { GithubSignIn } from '../src/chrome/GithubSignIn';
import { GithubSignInDialog } from '../src/chrome/GithubSignInDialog';
import { GithubAccountsCard } from '../src/chrome/GithubAccountsCard';
import {
  closeGithubSignIn, continueGithubSignIn, refreshGithubAccounts, setDefaultGithubAccount, useGithubAccounts,
} from '../src/lib/github-account-flow';
import { fakeGithubSignIn } from './github-sign-in-fake';

vi.mock('../src/pty-client', () => ({
  GITHUB_CLI_INSTALL_URL: 'https://fixture.invalid/install',
  ghAuthStatus: vi.fn(), githubAccountsRead: vi.fn(), githubAccountSetDefault: vi.fn(), githubAccountRemove: vi.fn(),
  githubSignInStart: vi.fn(), githubSignInCancel: vi.fn(), onGithubSignIn: vi.fn(), openExternalUrl: vi.fn(),
}));
const auth = { authenticated: true, username: 'original', scopes: [], error: null, cliMissing: false };
const empty: ipc.GithubAccounts = { defaultLogin: null, accounts: [] };
const accounts = (defaultLogin = 'first', state: 'ok' | 'needs_login' = 'ok'): ipc.GithubAccounts => ({ defaultLogin,
  accounts: ['first', 'second'].map(login => ({ login, default: login === defaultLogin, state: login === 'second' ? state : 'ok', source: 'gh-cli' })) });
let gh: ReturnType<typeof fakeGithubSignIn>;
let copied: string[];

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.ghAuthStatus).mockResolvedValue(auth);
  vi.mocked(ipc.githubAccountsRead).mockResolvedValue(empty);
  gh = fakeGithubSignIn(ipc);
  copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text: string) => { copied.push(text); } } });
  closeGithubSignIn();
  await refreshGithubAccounts();
  vi.clearAllMocks();
});
afterEach(() => { cleanup(); vi.useRealTimers(); });

const dialog = () => screen.getByRole('dialog', { name: 'Sign in to GitHub' });
const button = (name: string) => within(dialog()).getByRole('button', { name });
const press = (name: string) => act(async () => { fireEvent.click(button(name)); });
const open = async (props: Parameters<typeof GithubSignIn>[0] = {}) => {
  render(<><GithubSignIn {...props} /><GithubSignInDialog /></>);
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: props.label ?? 'Sign in' })); });
};
const toCode = async (props?: Parameters<typeof GithubSignIn>[0]) => { await open(props); await press('Signed in, next'); await gh.print(); };

it('asks for the account first and starts gh only after that', async () => {
  await open();
  expect(dialog()).toHaveTextContent('Step 1: In your browser, sign in to the GitHub account you want to add');
  expect(dialog()).toHaveTextContent('Step 2: Paste the code');
  expect(button('Signed in, next')).toHaveFocus();
  fireEvent.click(button('Open GitHub'));
  expect(ipc.openExternalUrl).toHaveBeenLastCalledWith('https://github.com/login');
  expect([ipc.githubSignInStart, ipc.onGithubSignIn].map(mock => vi.mocked(mock).mock.calls.length)).toEqual([0, 0]);
  await press('Cancel');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(ipc.githubSignInCancel).not.toHaveBeenCalled();
});

it('listens before it starts gh, shows gh’s own line above the code, and copies the code', async () => {
  await open();
  await press('Signed in, next');
  expect(vi.mocked(ipc.onGithubSignIn).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(ipc.githubSignInStart).mock.invocationCallOrder[0]);
  expect(dialog()).toHaveTextContent('Getting a code from GitHub…');
  expect(dialog()).toHaveFocus();
  await gh.print('\n');
  expect(dialog()).toHaveTextContent('Getting a code from GitHub…');
  await gh.print();
  expect(dialog()).toHaveTextContent('! First copy your one-time code: WDJB-MJHT');
  expect(screen.getByText('WDJB-MJHT', { selector: '.local-auth-device-code' })).toBeInTheDocument();
  expect(button('Copy code and open GitHub')).toHaveFocus();
  await press('Copy code and open GitHub');
  expect(copied).toEqual(['WDJB-MJHT']);
  expect(ipc.openExternalUrl).toHaveBeenLastCalledWith('https://github.com/login/device');
  expect(button('Copied')).toBeInTheDocument();
});

it('shows gh’s words in place of a code it cannot read', async () => {
  await open();
  await press('Signed in, next');
  await gh.print('\n\u001b[0;33m!\u001b[0m Enter WDJBMJHT at https://github.com/login/device\n');
  expect(dialog()).toHaveTextContent('! Enter WDJBMJHT at https://github.com/login/device');
  expect(dialog().textContent).not.toContain('\u001b');
  await press('Open GitHub');
  expect(copied).toEqual([]);
  expect(ipc.openExternalUrl).toHaveBeenLastCalledWith('https://github.com/login/device');
});

it('counts down to the backend’s deadline once a second, and only while it is shown', async () => {
  vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'Date'] });
  await open();
  await press('Signed in, next');
  expect(vi.getTimerCount()).toBe(0);
  await gh.print(undefined, Date.now() + 125_000);
  expect(screen.getByRole('timer')).toHaveTextContent('Expires in 2:05');
  expect(vi.getTimerCount()).toBe(1);
  act(() => { vi.advanceTimersByTime(66_000); });
  expect(screen.getByRole('timer')).toHaveTextContent('Expires in 0:59');
  expect(screen.getByRole('timer')).toHaveClass('ending');
  await gh.emit({ kind: 'timed_out' });
  expect(dialog()).toHaveTextContent('Sign-in did not finish10 minutes passed. Try again for a new code.');
  expect(vi.getTimerCount()).toBe(0);
});

it('stops listening with the sign-in, however it ends', async () => {
  for (const event of [{ kind: 'failed', output: '' }, { kind: 'timed_out' }, { kind: 'cancelled' }] as const) {
    await toCode();
    expect(gh.listeners.size).toBe(1);
    await gh.emit(event);
    expect(gh.listeners.size).toBe(0);
    act(closeGithubSignIn); cleanup();
  }
  await toCode();
  await gh.signIn({ login: 'second', view: accounts() });
  expect(gh.listeners.size).toBe(0);
  act(closeGithubSignIn); cleanup();
  await open();
  vi.mocked(ipc.githubSignInStart).mockRejectedValueOnce('start gh: permission denied');
  await press('Signed in, next');
  expect(gh.listeners.size).toBe(0);
  expect(dialog()).toHaveTextContent('Sign-in did not finishstart gh: permission denied');
});

it('gives gh’s reason without the dead code, and Try again gets a new one without Step 1', async () => {
  await toCode();
  await gh.emit({ kind: 'failed', output: '\n! First copy your one-time code: WDJB-MJHT\nOpen this URL: https://github.com/login/device\nfailed to authenticate via web browser: HTTP 503\n' });
  expect(screen.getByRole('alert')).toHaveTextContent('Sign-in did not finishfailed to authenticate via web browser: HTTP 503');
  expect(dialog()).not.toHaveTextContent('WDJB-MJHT');
  await press('Try again');
  expect(dialog()).toHaveTextContent('Getting a code from GitHub…');
  expect(gh.ids).toHaveLength(2);
  expect(gh.ids[0]).not.toBe(gh.ids[1]);
  await gh.emit({ kind: 'failed', output: '' });
  expect(screen.getByRole('alert')).toHaveTextContent('GitHub CLI stopped without saying why.');
});

it.each([
  ['a first account', {}, { login: 'first', view: { defaultLogin: 'first', accounts: [accounts().accounts[0]] } }, 'first addedIt is the default for new projects.'],
  ['another account', {}, { login: 'second', view: accounts() }, 'second added'],
  ['gh that could not be switched back', {}, { login: 'second', active: 'second', restoreFailed: true, view: accounts() }, 'second addedThe current gh account is now second.'],
  ['an account already in Kota', {}, { login: 'first', added: false, view: accounts() }, 'first is already in Kota'],
  ['the account it was about', { expect: 'second' }, { login: 'second', added: false, view: accounts() }, 'second is signed in'],
  ['another new account instead', { expect: 'second' }, { login: 'third', view: accounts() }, 'You signed in as thirdthird was added. second still needs to sign in.'],
  ['another known account instead', { expect: 'second' }, { login: 'first', added: false, view: accounts() }, 'You signed in as firstsecond still needs to sign in.'],
])('says who signed in: %s', async (_, props, result, text) => {
  await toCode(props);
  await gh.signIn(result);
  expect(dialog().querySelector('.local-auth-result')!.textContent).toBe(text);
});

it('treats a signed-out account added without naming it as signed in, and Try again goes back to Step 1', async () => {
  vi.mocked(ipc.githubAccountsRead).mockResolvedValue(accounts('first', 'needs_login'));
  await act(refreshGithubAccounts);
  await toCode();
  await gh.signIn({ login: 'second', added: false, view: accounts() });
  expect(dialog()).toHaveTextContent('second is signed in');
  await press('Done'); cleanup();
  await toCode();
  await gh.signIn({ login: 'first', added: false, view: accounts() });
  await press('Try again');
  expect(button('Signed in, next')).toBeInTheDocument();
  expect(gh.ids).toHaveLength(2);
});

it('names the account that needs to sign in again', async () => {
  await open({ expect: 'second' });
  expect(dialog()).toHaveTextContent('Step 1: In your browser, sign in to GitHub as second');
});

it('cancels gh, and still lists an account gh had just signed in', async () => {
  const { result } = renderHook(() => useGithubAccounts());
  await toCode();
  await act(async () => { fireEvent.keyDown(window, { key: 'Escape' }); });
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(ipc.githubSignInCancel).toHaveBeenCalledWith(gh.ids[0]);
  await gh.signIn({ login: 'second', view: accounts() });
  expect(result.current.view).toEqual(accounts());
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it('cancels a gh that was still starting, and keeps late output out of a new sign-in', async () => {
  let started!: () => void;
  vi.mocked(ipc.githubSignInStart).mockImplementationOnce(id => new Promise(resolve => { gh.ids.push(id); started = resolve; }));
  await open();
  await press('Signed in, next');
  await press('Cancel');
  vi.mocked(ipc.githubSignInCancel).mockClear();
  await act(async () => started());
  expect(ipc.githubSignInCancel).toHaveBeenCalledWith(gh.ids[0]);
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Sign in' })); });
  await press('Signed in, next');
  await gh.send(gh.ids[0], { kind: 'output', output: 'ABCD-1234', deadlineMs: Date.now() + 1000 });
  expect(dialog()).toHaveTextContent('Getting a code from GitHub…');
});

it('never starts a gh that was cancelled before it could listen', async () => {
  let listening!: (stop: () => void) => void;
  const stop = vi.fn();
  vi.mocked(ipc.onGithubSignIn).mockReturnValueOnce(new Promise(resolve => { listening = resolve; }));
  await open();
  await press('Signed in, next');
  await press('Cancel');
  await act(async () => listening(stop));
  expect(stop).toHaveBeenCalledTimes(1);
  expect(ipc.githubSignInStart).not.toHaveBeenCalled();
});

it('says at once that GitHub CLI is missing, and checks again', async () => {
  vi.mocked(ipc.ghAuthStatus).mockResolvedValueOnce({ ...auth, cliMissing: true });
  await open();
  expect(dialog()).toHaveTextContent('GitHub CLI is not installed');
  fireEvent.click(button('Install GitHub CLI'));
  expect(ipc.openExternalUrl).toHaveBeenLastCalledWith('https://fixture.invalid/install');
  await press('Check again');
  expect(button('Signed in, next')).toBeInTheDocument();
  vi.mocked(ipc.githubSignInStart).mockRejectedValueOnce('github_cli_missing');
  await press('Signed in, next');
  expect(dialog()).toHaveTextContent('GitHub CLI is not installed');
});

it('keeps Tab inside, where WebKit would leave for the fields behind', async () => {
  render(<input aria-label="behind" />);
  await open();
  const names: string[] = [];
  for (let i = 0; i < 4; i += 1) { fireEvent.keyDown(document.activeElement!, { key: 'Tab' }); names.push(document.activeElement!.textContent!); }
  expect(names).toEqual(['Cancel', 'Open GitHub', 'Signed in, next', 'Cancel']);
  fireEvent.keyDown(document.activeElement!, { key: 'Tab', shiftKey: true });
  expect(button('Signed in, next')).toHaveFocus();
});

it('shares one dialog: the entry point that started it may go away', async () => {
  const entry = render(<GithubSignIn label="Add GitHub account" />);
  render(<GithubSignInDialog />);
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Add GitHub account' })); });
  await press('Signed in, next');
  entry.unmount();
  await gh.print();
  expect(dialog()).toHaveTextContent('WDJB-MJHT');
});

it('does not read the accounts or listen while the dialog is closed', () => {
  render(<GithubSignInDialog />);
  expect(ipc.githubAccountsRead).not.toHaveBeenCalled();
  expect(ipc.onGithubSignIn).not.toHaveBeenCalled();
});

it('Link signs in from + New and from a row, and points at the account once the dialog closes', async () => {
  vi.mocked(ipc.githubAccountsRead).mockResolvedValue(accounts('first', 'needs_login'));
  render(<><GithubAccountsCard /><GithubSignInDialog /></>);
  await screen.findByText('second');
  fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
  expect(dialog()).toHaveTextContent('sign in to GitHub as second');
  await press('Cancel');
  fireEvent.click(screen.getByRole('button', { name: '+ New' }));
  await press('Signed in, next');
  await gh.signIn({ login: 'second', added: false, view: accounts() });
  const row = () => screen.getByText('second', { selector: 'strong' }).closest('.github-account-row')!;
  expect(row()).not.toHaveClass('is-landing');
  await press('Done');
  expect(row()).toHaveClass('is-landing');
  expect(screen.getAllByText('Default')).toHaveLength(1);
});

it('lights each account by the state its row already has', async () => {
  vi.mocked(ipc.githubAccountsRead).mockResolvedValue(accounts('first', 'needs_login'));
  render(<GithubAccountsCard />);
  await screen.findByText('second');
  const light = (login: string) => within(screen.getByText(login, { selector: 'strong' }).closest<HTMLElement>('.github-account-row')!).getByRole('img');
  expect(light('first')).toHaveAttribute('title', 'Signed in');
  expect(light('first')).toHaveClass('signed-in');
  expect(light('second')).toHaveAttribute('title', 'Sign in required');
  expect(light('second')).not.toHaveClass('signed-in');
  expect(ipc.githubAccountsRead).toHaveBeenCalledTimes(1);
  expect(ipc.ghAuthStatus).not.toHaveBeenCalled();
});

it('does not let a late account read undo a sign-in or a default change', async () => {
  const { result } = renderHook(() => useGithubAccounts());
  for (const change of [
    async () => { await act(async () => { void continueGithubSignIn(); }); await gh.signIn({ login: 'second', view: accounts('second') }); act(closeGithubSignIn); },
    async () => { vi.mocked(ipc.githubAccountSetDefault).mockResolvedValueOnce(accounts('second')); await act(() => setDefaultGithubAccount('second')); },
  ]) {
    let read!: (value: ipc.GithubAccounts) => void;
    vi.mocked(ipc.githubAccountsRead).mockReturnValueOnce(new Promise(resolve => { read = resolve; }));
    let oldRead!: Promise<void>;
    act(() => { oldRead = refreshGithubAccounts(); });
    await change();
    await act(async () => { read(accounts('first')); await oldRead; });
    expect(result.current.view?.defaultLogin).toBe('second');
    expect(result.current.loading).toBe(false);
  }
});

it('lists gh accounts, marks the Kota default, and removes through the account command only', async () => {
  vi.mocked(ipc.githubAccountsRead).mockResolvedValue(accounts());
  vi.mocked(ipc.githubAccountRemove).mockResolvedValue({ defaultLogin: 'second', accounts: [accounts('second').accounts[1]] });
  render(<GithubAccountsCard />);
  await screen.findByText('second');
  expect(screen.getByText('Default')).toBeInTheDocument();
  expect(ipc.githubAccountsRead).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole('button', { name: 'Refresh accounts' })).not.toBeInTheDocument();
  fireEvent.click(screen.getAllByRole('button', { name: 'Remove' })[0]);
  await waitFor(() => expect(screen.queryByText('first')).not.toBeInTheDocument());
  expect(ipc.githubAccountRemove).toHaveBeenCalledWith('first');
  expect(ipc.githubAccountSetDefault).not.toHaveBeenCalled();
});

it('styles only this dialog: the Kota Local sign-in that shares the layer keeps its look', async () => {
  const { readFile } = await import('node:fs/promises');
  const css = (await readFile('src/styles/github-sign-in.css', 'utf8')).replace(/\/\*[\s\S]*?\*\//g, '');
  const selectors = [...css.matchAll(/([^{}@]+)\{/g)].flatMap(match => match[1].split(',')).map(text => text.trim()).filter(text => text && !text.includes('prefers-reduced-motion'));
  expect(selectors.length).toBeGreaterThan(20);
  for (const selector of selectors) expect(selector).toMatch(/\.github-/);
});
