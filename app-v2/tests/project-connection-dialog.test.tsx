import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { ProjectConnectionDialog, type RepositoryCheckKind } from '../src/chrome/ProjectConnectionDialog';

const props = {
  name: 'Fixture', replacing: true,
  accounts: { defaultLogin: 'fixture', accounts: [{ login: 'fixture', default: true, state: 'ok' as const, source: 'gh-cli' as const }] },
  draft: { accountLogin: 'fixture', mode: 'existing' as const, name: 'fixture', private: true, repoFullName: 'fixture/repo' },
  repos: [], reposLoading: false, error: null, busy: false,
  onDraft: vi.fn(), onRetry: vi.fn(), onAddAccount: vi.fn(), onContinue: vi.fn(), onCancel: vi.fn(),
};

it.each<[RepositoryCheckKind, boolean]>([
  ['empty', true], ['in_sync', true], ['local_ahead', true], ['remote_ahead', true], ['diverged', true],
  ['unrelated', false], ['no_access', false], ['already_open', false], ['checking', false], ['check_failed', false], ['blocked_large_file', false],
])('allows confirmation only for a connectable repository (%s)', (kind, allowed) => {
  render(<ProjectConnectionDialog {...props} check={{ kind }} />);
  const connect = screen.getByRole('button', { name: 'Connect' });
  if (allowed) expect(connect).toBeEnabled(); else expect(connect).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus();
});

it('offers Retry for failed checking without presenting it as an access failure', () => {
  const retry = vi.fn();
  render(<ProjectConnectionDialog {...props} check={{ kind: 'check_failed' }} onRetry={retry} />);
  expect(screen.getByText('Could not check repository')).toBeInTheDocument();
  expect(screen.queryByText('Access unavailable')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  expect(retry).toHaveBeenCalledTimes(1);
});

it('lists oversized history files and keeps confirmation unavailable', () => {
  render(<ProjectConnectionDialog {...props} check={{ kind: 'blocked_large_file', largeFiles: [{ path: 'fixtures/archive.bin', bytes: 120_500_000 }] }} />);
  expect(screen.getByText('fixtures/archive.bin')).toBeInTheDocument();
  expect(screen.getByText(/120.5 MB/)).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Connect' })).toBeDisabled();
});

it('does not offer a connection through an account whose credentials need login', () => {
  render(<ProjectConnectionDialog {...props} accounts={{ defaultLogin: 'fixture', accounts: [{ ...props.accounts.accounts[0], state: 'needs_login' }] }} check={{ kind: 'in_sync' }} />);
  expect(screen.getByRole('button', { name: 'Connect' })).toBeDisabled();
  expect(screen.getByRole('option', { name: /Sign in required/ })).toBeDisabled();
});
