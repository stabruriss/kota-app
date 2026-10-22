import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { TavernModal } from '../src/chrome/TavernModal';
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
  cleanup(); vi.restoreAllMocks();
  if (storageDescriptor) Object.defineProperty(window, 'localStorage', storageDescriptor);
});

it('opens the GitHub CLI install page in the system browser, never with window.open', async () => {
  const accounts: pty.GithubAccounts = { defaultLogin: null, accounts: [] };
  vi.spyOn(pty, 'githubAccountsRead').mockResolvedValue(accounts);
  vi.spyOn(pty, 'ghAuthStatus').mockResolvedValue(cliMissing);
  const beginSignIn = vi.spyOn(pty, 'githubAccountAddBegin').mockResolvedValue(null);
  const finishSignIn = vi.spyOn(pty, 'githubAccountAddFinish').mockResolvedValue({
    added: [], active: null, restoreFailed: false, view: accounts,
  });
  const openExternal = vi.spyOn(pty, 'openExternalUrl').mockResolvedValue(undefined);
  const windowOpen = vi.spyOn(window, 'open').mockImplementation(() => null);
  render(<TavernModal open onClose={() => {}} initialTab="link" />);
  const card = screen.getByRole('region', { name: 'GitHub accounts' });
  await within(card).findByText('No GitHub accounts yet.');
  await userEvent.click(within(card).getByRole('button', { name: '+ New' }));
  await waitFor(() => {
    expect(openExternal).toHaveBeenCalledTimes(1);
    expect(card).toHaveAttribute('aria-busy', 'false');
  });
  expect(openExternal).toHaveBeenCalledWith(pty.GITHUB_CLI_INSTALL_URL);
  expect(windowOpen).not.toHaveBeenCalled();
  expect(beginSignIn).not.toHaveBeenCalled();
  expect(finishSignIn).not.toHaveBeenCalled();
});
