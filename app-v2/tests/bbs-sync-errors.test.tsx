import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('../src/pty-client', () => ({ openExternalUrl: vi.fn().mockResolvedValue(undefined) }));
import { openExternalUrl } from '../src/pty-client';
import { BbsSyncStatusBar } from '../src/chrome/BbsDeviceSync';
import { BBS_SYNC_DISCUSSIONS_URL, BBS_SYNC_ERROR_DETAILS, BBS_SYNC_PROTOCOL_ERROR_DETAIL } from '../src/bbs-sync-errors';
import type { BbsSyncView } from '../src/bbs-sync-view';
import retryRecovery from './fixtures/bbs-retry-recovery.json';
import protocolErrors from './fixtures/bbs-protocol-errors.json';

const state = (error: string | null): BbsSyncView => ({
  deviceId: 'self', deviceName: 'Mac', workerAvailable: true,
  group: { id: 'one', name: 'Studio', role: 'owner', members: [] },
  invitation: { state: 'none' }, phase: 'failed', progress: null,
  lastSuccessfulAt: '2026-09-13T04:00:00Z', error, controlRecoverable: true,
});
const props = { onSync: vi.fn(), onCancel: vi.fn() };
// Actual Rust display_error output, not HTML taken from the preview.
const copy = Object.values(retryRecovery.details);
beforeEach(() => vi.clearAllMocks());

describe('BBS single-line sync error presentation', () => {
  it('reserves the upgrade instruction for a confirmed version mismatch from Rust', () => {
    // Produced by only_confirmed_version_mismatch_recommends_updating.
    expect(BBS_SYNC_PROTOCOL_ERROR_DETAIL).toBe(protocolErrors.sync_protocol_error);
    const rendered = render(<BbsSyncStatusBar view={state(protocolErrors.sync_protocol_error)} {...props} />);
    expect(screen.getByRole('status')).not.toHaveTextContent('update Kota');
    expect(screen.getByRole('link')).toHaveAttribute('href', BBS_SYNC_DISCUSSIONS_URL);
    rendered.rerender(<BbsSyncStatusBar view={state(protocolErrors.protocol_mismatch)} {...props} />);
    expect(screen.getByRole('status')).toHaveTextContent('update Kota');
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    rendered.rerender(<BbsSyncStatusBar view={state(protocolErrors.sync_timeout)} {...props} />);
    expect(screen.getByRole('status')).not.toHaveTextContent('protocol');
    expect(screen.getAllByText('Sync error:')).toHaveLength(1);
  });

  it('keeps frontend safe-action and link copy byte-equal to the Rust producer details', () => {
    expect(copy).toHaveLength(11);
    expect(BBS_SYNC_ERROR_DETAILS).toEqual({ unknown: retryRecovery.details.sync_unavailable,
      identity: retryRecovery.details.incomplete_sync_identity, sync_busy: retryRecovery.details.control_in_use,
      stale_signature: retryRecovery.details.stale_signature, worker_update_required: retryRecovery.details.worker_update_required });
  });

  it.each(copy)('renders one prefix and the approved detail: %s', (detail) => {
    render(<BbsSyncStatusBar view={state(detail)} {...props} />);
    const line = screen.getByRole('status');
    expect(line).toHaveTextContent(`Sync error: ${detail.replace('GitHub Discussions.', 'GitHub Discussions↗.')}`);
    expect(screen.getAllByText('Sync error:')).toHaveLength(1);
    expect(screen.queryByText('Sync failed')).not.toBeInTheDocument();
    expect(screen.getByText(/^Last sync at/)).toBeInTheDocument();
    const linked = detail === BBS_SYNC_ERROR_DETAILS.unknown || detail === BBS_SYNC_ERROR_DETAILS.identity;
    expect(screen.queryAllByRole('link')).toHaveLength(linked ? 1 : 0);
    expect(openExternalUrl).not.toHaveBeenCalled();
  });

  it('opens only the fixed Discussions destination, using the existing native external opener', () => {
    render(<BbsSyncStatusBar view={state(BBS_SYNC_ERROR_DETAILS.unknown)} {...props} />);
    const link = screen.getByRole('link', { name: 'GitHub Discussions' });
    expect(link).toHaveAttribute('href', BBS_SYNC_DISCUSSIONS_URL);
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
    fireEvent.click(link);
    expect(openExternalUrl).toHaveBeenCalledExactlyOnceWith(BBS_SYNC_DISCUSSIONS_URL);
  });

  it('does not interpret backend markup, URLs or near-match link text', () => {
    const detail = '<a href="https://wrong.invalid">GitHub Discussions</a> Sync could not finish; retry or report it on GitHub Discussions.';
    render(<BbsSyncStatusBar view={state(detail)} {...props} />);
    expect(screen.getByRole('status')).toHaveTextContent(detail);
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    expect(openExternalUrl).not.toHaveBeenCalled();
  });

  it('shows incomplete identity without fabricating group controls or a self-repair action', () => {
    render(<BbsSyncStatusBar view={{ ...state(BBS_SYNC_ERROR_DETAILS.identity), group: null, controlRecoverable: false }} {...props} />);
    expect(screen.getByRole('link', { name: 'GitHub Discussions' })).toBeInTheDocument();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('gates only by the typed recovery capability, never error wording or a code substring', () => {
    const view = state(BBS_SYNC_ERROR_DETAILS.sync_busy);
    const rendered = render(<BbsSyncStatusBar view={{ ...view, controlRecoverable: false }} {...props} />);
    expect(screen.getByRole('button', { name: 'Retry sync' })).toBeDisabled();
    expect(screen.getByText('No other device online now.')).toBeInTheDocument();
    rendered.rerender(<BbsSyncStatusBar view={{ ...view, error: 'An unrelated display-safe detail.' }} {...props} />);
    expect(screen.getByRole('button', { name: 'Retry sync' })).toBeEnabled();
    expect(screen.queryByText('No other device online now.')).not.toBeInTheDocument();
    expect(props.onSync).not.toHaveBeenCalled();
    rendered.rerender(<BbsSyncStatusBar view={{ ...view, phase: 'connecting' }} {...props} />);
    expect(screen.getByRole('button', { name: 'Retry sync' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
  });
});
