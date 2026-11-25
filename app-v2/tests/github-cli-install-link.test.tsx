import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { GithubSignInDialog } from '../src/chrome/GithubSignInDialog';
import { TavernModal } from '../src/chrome/TavernModal';
import { closeGithubSignIn } from '../src/lib/github-account-flow';
import * as pty from '../src/pty-client';

const cliMissing: pty.GhAuthInfo = { authenticated: false, username: null, scopes: [], error: null, cliMissing: true };
let storageDescriptor: PropertyDescriptor | undefined;
beforeEach(() => {
  const values = new Map<string, string>();
  storageDescriptor = Object.getOwnPropertyDescriptor(window, 'localStorage');
  Object.defineProperty(window, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
});
afterEach(() => {
  closeGithubSignIn(); cleanup(); vi.restoreAllMocks();
  if (storageDescriptor) Object.defineProperty(window, 'localStorage', storageDescriptor);
});

it('opens the GitHub CLI install page in the system browser, never with window.open', async () => {
  const accounts: pty.GithubAccounts = { defaultLogin: null, accounts: [] };
  vi.spyOn(pty, 'githubAccountsRead').mockResolvedValue(accounts);
  vi.spyOn(pty, 'ghAuthStatus').mockResolvedValue(cliMissing);
  const startSignIn = vi.spyOn(pty, 'githubSignInStart').mockResolvedValue(undefined);
  const openExternal = vi.spyOn(pty, 'openExternalUrl').mockResolvedValue(undefined);
  const windowOpen = vi.spyOn(window, 'open').mockImplementation(() => null);
  render(<><TavernModal open onClose={() => {}} initialTab="link" /><GithubSignInDialog /></>);
  const card = screen.getByRole('region', { name: 'GitHub accounts' });
  await within(card).findByText('No GitHub accounts yet.');
  await userEvent.click(within(card).getByRole('button', { name: '+ New' }));
  expect(openExternal).not.toHaveBeenCalled();
  await userEvent.click(await screen.findByRole('button', { name: 'Install GitHub CLI' }));
  await waitFor(() => expect(openExternal).toHaveBeenCalledTimes(1));
  expect(openExternal).toHaveBeenCalledWith(pty.GITHUB_CLI_INSTALL_URL);
  expect(windowOpen).not.toHaveBeenCalled();
  expect(startSignIn).not.toHaveBeenCalled();
});
