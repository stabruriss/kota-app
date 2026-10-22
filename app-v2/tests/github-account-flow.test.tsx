import { act, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import * as ipc from '../src/pty-client';
import { GithubSignIn } from '../src/chrome/GithubSignIn';
import { GithubAccountsCard } from '../src/chrome/GithubAccountsCard';
import {
  beginGithubSignIn, finishGithubSignIn, refreshGithubAccounts,
  setDefaultGithubAccount, useGithubAccounts,
} from '../src/lib/github-account-flow';

vi.mock('../src/pty-client', () => ({
  GITHUB_CLI_INSTALL_URL: 'https://fixture.invalid/install', GITHUB_CLI_LOGIN_COMMAND: 'gh auth login fixture',
  ghAuthStatus: vi.fn(), githubAccountAddBegin: vi.fn(), githubAccountAddFinish: vi.fn(),
  githubAccountsRead: vi.fn(), githubAccountSetDefault: vi.fn(), githubAccountRemove: vi.fn(), openExternalUrl: vi.fn(),
}));
const empty: ipc.GithubAccounts = { defaultLogin: null, accounts: [] };
const accounts = (defaultLogin = 'first'): ipc.GithubAccounts => ({ defaultLogin,
  accounts: ['first', 'second'].map(login => ({ login, default: login === defaultLogin, state: 'ok', source: 'gh-cli' })) });

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.ghAuthStatus).mockResolvedValue({ authenticated: true, username: 'original', scopes: [], error: null, cliMissing: false });
  vi.mocked(ipc.githubAccountsRead).mockResolvedValue(empty);
  vi.mocked(ipc.githubAccountAddBegin).mockResolvedValue('original');
  vi.mocked(ipc.githubAccountAddFinish).mockResolvedValue({ added: [], active: 'original', restoreFailed: false, view: empty });
  await finishGithubSignIn();
  await refreshGithubAccounts();
  vi.clearAllMocks();
});

it('shares one pending login across entry points and finishes only after an explicit click', async () => {
  const run = vi.fn(); window.addEventListener('kota:smart-run', run);
  try {
    const first = render(<GithubSignIn label="Login GitHub" />);
    fireEvent.click(screen.getByRole('button', { name: 'Login GitHub' }));
    await screen.findByText('Complete sign-in in Smart Terminal.');
    first.unmount();
    render(<GithubSignIn label="Add GitHub account" />);
    expect(screen.getByText('Complete sign-in in Smart Terminal.')).toBeInTheDocument();
    await act(beginGithubSignIn);
    fireEvent.focus(window);
    await act(refreshGithubAccounts);
    expect(run).toHaveBeenCalledTimes(1);
    expect(ipc.githubAccountAddBegin).toHaveBeenCalledTimes(1);
    expect(ipc.githubAccountAddFinish).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Check sign-in' }));
    await waitFor(() => expect(ipc.githubAccountAddFinish).toHaveBeenCalledWith('original'));
    await screen.findByRole('button', { name: 'Add GitHub account' });
  } finally { window.removeEventListener('kota:smart-run', run); }
});

it('allows an explicit finish after App restart with no previous account to restore', async () => {
  render(<GithubSignIn />);
  fireEvent.click(screen.getByRole('button', { name: 'Check sign-in' }));
  await waitFor(() => expect(ipc.githubAccountAddFinish).toHaveBeenCalledWith(null));
});

it('serializes finish, shows restore failure as a notice, and keeps the returned account list', async () => {
  await beginGithubSignIn();
  let finish!: (result: ipc.GithubAccountAddResult) => void;
  vi.mocked(ipc.githubAccountAddFinish).mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
  const { result } = renderHook(() => useGithubAccounts());
  render(<GithubSignIn />);
  fireEvent.click(screen.getByRole('button', { name: 'Check sign-in' }));
  expect(screen.getByRole('button', { name: 'Check sign-in' })).toBeDisabled();
  await act(finishGithubSignIn);
  expect(ipc.githubAccountAddFinish).toHaveBeenCalledTimes(1);
  await act(async () => finish({ added: ['second'], active: 'second', restoreFailed: true, view: accounts() }));
  expect(screen.getByRole('status')).toHaveTextContent('The current gh account is now second.');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(result.current.view).toEqual(accounts());
});

it('does not let a late account read undo a successful default change', async () => {
  let read!: (value: ipc.GithubAccounts) => void;
  vi.mocked(ipc.githubAccountsRead).mockReturnValueOnce(new Promise(resolve => { read = resolve; }));
  vi.mocked(ipc.githubAccountSetDefault).mockResolvedValueOnce(accounts('second'));
  const { result } = renderHook(() => useGithubAccounts());
  let oldRead!: Promise<void>;
  act(() => { oldRead = refreshGithubAccounts(); });
  await act(() => setDefaultGithubAccount('second'));
  await act(async () => { read(accounts('first')); await oldRead; });
  expect(result.current.view?.defaultLogin).toBe('second');
});

it('retains pending restoration information after a finish error for an explicit retry', async () => {
  await beginGithubSignIn();
  vi.mocked(ipc.githubAccountAddFinish).mockRejectedValueOnce(new Error('fixture network failure'));
  render(<GithubSignIn />);
  fireEvent.click(screen.getByRole('button', { name: 'Check sign-in' }));
  await screen.findByRole('alert');
  expect(screen.getByText('Complete sign-in in Smart Terminal.')).toBeInTheDocument();
  expect(ipc.githubAccountAddFinish).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole('button', { name: 'Check sign-in' }));
  await waitFor(() => expect(ipc.githubAccountAddFinish).toHaveBeenCalledTimes(2));
  expect(ipc.githubAccountAddFinish).toHaveBeenLastCalledWith('original');
});

it('lists gh accounts, marks the Kota default, and removes through the account command only', async () => {
  vi.mocked(ipc.githubAccountsRead).mockResolvedValue(accounts());
  vi.mocked(ipc.githubAccountRemove).mockResolvedValue({ defaultLogin: 'second', accounts: [accounts('second').accounts[1]] });
  render(<GithubAccountsCard />);
  await screen.findByText('second');
  expect(screen.getByText('Default')).toBeInTheDocument();
  fireEvent.click(screen.getAllByRole('button', { name: 'Remove' })[0]);
  await waitFor(() => expect(screen.queryByText('first')).not.toBeInTheDocument());
  expect(ipc.githubAccountRemove).toHaveBeenCalledWith('first');
  expect(ipc.githubAccountSetDefault).not.toHaveBeenCalled();
});
