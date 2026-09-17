import { fireEvent, render, screen, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('../src/pty-client', () => ({ openExternalUrl: vi.fn().mockResolvedValue(undefined) }));
import { openExternalUrl } from '../src/pty-client';
import { BbsSyncStatusBar } from '../src/chrome/BbsDeviceSync';
import { BBS_SYNC_DISCUSSIONS_URL, BBS_SYNC_ERROR_DETAILS, BBS_SYNC_PROTOCOL_ERROR_DETAIL } from '../src/bbs-sync-errors';
import type { BbsSyncView } from '../src/bbs-sync-view';
import type { BbsSyncIndicator } from '../src/types/bbs-sync';
import retryRecovery from './fixtures/bbs-retry-recovery.json';
import protocolErrors from './fixtures/bbs-protocol-errors.json';

const state = (error: string | null): BbsSyncView => ({
  deviceId: 'self', deviceName: 'Mac', workerAvailable: true,
  group: { id: 'one', name: 'Studio', role: 'owner', members: [] },
  invitation: { state: 'none' }, phase: 'failed', progress: null,
  lastSuccessfulAt: '2026-09-13T04:00:00Z', error, controlRecoverable: true, serviceRecoverable: false,
});
const props = { onSync: vi.fn() };
beforeEach(() => vi.clearAllMocks());

describe('BBS typed sync indicator', () => {
  it.each<[BbsSyncIndicator, string, string]>([
    ['healthy', 'green', '0 online'], ['connecting', 'yellow', 'Connecting'],
    ['reaching_service', 'yellow', 'Reaching Service'], ['reaching_peers', 'yellow', 'Reaching Peers'],
    ['fetching_session', 'yellow', 'Fetching Session'], ['finishing_sync', 'yellow', 'Finishing Sync'],
    ['retrying_files', 'yellow', 'Retrying files'], ['checking_protocol', 'yellow', 'Checking Protocol'],
    ['reconnecting', 'yellow', 'Reconnecting'], ['cloudflare_limit', 'yellow', 'Cloudflare limit'],
    ['update_worker', 'red', 'Update Worker'], ['update_kota', 'red', 'Update Kota'],
    ['group_access_denied', 'red', 'Group access denied'], ['device_identity_error', 'red', 'Device ID error'],
    ['other_instance', 'red', '1 Kota per device'], ['file_access_error', 'red', 'File access error'],
  ])('maps %s only from the typed enum to %s / %s', (indicator, tone, label) => {
    render(<BbsSyncStatusBar view={{ ...state('PRIVATE misleading update Kota error'), indicator }} {...props} />);
    const status = screen.getByRole('status');
    expect(status).toHaveAttribute('data-tone', tone);
    expect(status).toHaveAttribute('data-indicator', indicator);
    expect(within(status).getByText(label)).toHaveClass('bbs-sync-short-status');
    expect(status).toHaveClass('st-bar-status');
    expect(status.querySelectorAll('.st-dot.live')).toHaveLength(1);
    expect(document.body.textContent).not.toContain('PRIVATE');
    expect(screen.queryByText('Sync error:')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
    expect(screen.getByText(/^Last sync at/)).toBeInTheDocument();
    expect(props.onSync).not.toHaveBeenCalled();
    expect(openExternalUrl).not.toHaveBeenCalled();
  });

  it('integrates the online count only in healthy text, without claiming content completion', () => {
    const view = state(null);
    view.group!.members = [
      { id: 'self', name: 'Mac', role: 'owner', online: true },
      { id: 'peer', name: 'Peer', role: 'member', online: true },
      { id: 'offline', name: 'Offline', role: 'member', online: false },
    ];
    const rendered = render(<BbsSyncStatusBar view={{ ...view, phase: 'syncing', controlRecoverable: false,
      indicator: 'healthy', progress: { completed: 0, total: 3 } }} {...props} />);
    expect(screen.getAllByText('2 online')).toHaveLength(1);
    expect(screen.getByRole('button', { name: 'Syncing 0/3' })).toBeDisabled();
    expect(screen.queryByText(/All devices synced|Sync complete/)).not.toBeInTheDocument();
    rendered.rerender(<BbsSyncStatusBar view={{ ...view, indicator: 'reaching_peers' }} {...props} />);
    expect(screen.queryByText(/\d+ online/)).not.toBeInTheDocument();
    expect(screen.getByText('Reaching Peers')).toBeInTheDocument();
  });

  it('does not promote legacy English errors into a proven red fault', () => {
    const rendered = render(<BbsSyncStatusBar view={state(protocolErrors.protocol_mismatch)} {...props} />);
    expect(screen.getByRole('status')).toHaveAttribute('data-indicator', 'reconnecting');
    expect(screen.getByRole('status')).toHaveAttribute('data-tone', 'yellow');
    rendered.rerender(<BbsSyncStatusBar view={{ ...state(protocolErrors.sync_protocol_error), indicator: 'checking_protocol' }} {...props} />);
    expect(screen.getByRole('status')).not.toHaveTextContent('Update Kota');
    rendered.rerender(<BbsSyncStatusBar view={{ ...state(null), indicator: 'update_kota' }} {...props} />);
    expect(screen.getByRole('status')).toHaveAttribute('data-tone', 'red');
    expect(screen.getByText('Update Kota')).toBeInTheDocument();
  });

  it('separates ambiguous busy rejection from the historical precise lease detail', () => {
    expect(BBS_SYNC_PROTOCOL_ERROR_DETAIL).toBe(protocolErrors.sync_protocol_error);
    expect(BBS_SYNC_ERROR_DETAILS).toMatchObject({ unknown: retryRecovery.details.sync_unavailable,
      identity: retryRecovery.details.incomplete_sync_identity,
      stale_signature: retryRecovery.details.stale_signature, worker_update_required: retryRecovery.details.worker_update_required });
    expect(BBS_SYNC_ERROR_DETAILS.sync_busy).toBe('Sync is busy; try again shortly.');
    expect(BBS_SYNC_ERROR_DETAILS.sync_busy).not.toBe(retryRecovery.details.control_in_use);
  });

  it('shows incomplete identity without manufacturing group controls and opens only the fixed Discussions URL', () => {
    render(<BbsSyncStatusBar view={{ ...state(BBS_SYNC_ERROR_DETAILS.identity), indicator: 'device_identity_error',
      group: null, controlRecoverable: false }} {...props} />);
    const status = screen.getByRole('status');
    expect(status).toHaveAttribute('tabindex', '0');
    expect(status).toHaveAttribute('aria-describedby', screen.getByRole('tooltip', { hidden: true }).id);
    const link = screen.getByRole('link', { name: 'GitHub Discussions' });
    expect(link).toHaveAttribute('href', BBS_SYNC_DISCUSSIONS_URL);
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    fireEvent.click(link);
    expect(openExternalUrl).toHaveBeenCalledExactlyOnceWith(BBS_SYNC_DISCUSSIONS_URL);
  });

  it('does not interpret legacy backend markup or arbitrary URLs', () => {
    const detail = '<a href="https://wrong.invalid">GitHub Discussions</a> Device identity is incomplete.';
    render(<BbsSyncStatusBar view={state(detail)} {...props} />);
    expect(screen.getByRole('status')).toHaveTextContent(detail);
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    expect(openExternalUrl).not.toHaveBeenCalled();
  });

  it('keeps the normal Manual sync label in the recovery window without inventing progress', () => {
    render(<BbsSyncStatusBar view={{ ...state(null), phase: 'connecting', indicator: 'connecting', controlRecoverable: false,
      group: { ...state(null).group!, members: [{ id: 'peer', name: 'Peer', role: 'member', online: true }] } }} {...props} />);
    expect(screen.getByRole('button', { name: 'Manual sync' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: /Reconnecting|Connecting|Cancel/ })).not.toBeInTheDocument();
    expect(screen.queryByText(/^Syncing/)).not.toBeInTheDocument();
    expect(screen.getByText(/^Last sync at/)).toBeInTheDocument();
  });

  it('gates only by typed recovery capabilities, never by indicator color or wording', () => {
    const view = { ...state(BBS_SYNC_ERROR_DETAILS.sync_busy), indicator: 'other_instance' as const };
    const rendered = render(<BbsSyncStatusBar view={{ ...view, controlRecoverable: false }} {...props} />);
    expect(screen.getByRole('button', { name: 'Manual sync' })).toBeDisabled();
    expect(screen.getByText('No other device online now.')).toBeInTheDocument();
    rendered.rerender(<BbsSyncStatusBar view={view} {...props} />);
    expect(screen.getByRole('button', { name: 'Manual sync' })).toBeEnabled();
    expect(screen.queryByText('No other device online now.')).not.toBeInTheDocument();
    rendered.rerender(<BbsSyncStatusBar view={{ ...view, phase: 'connecting' }} {...props} />);
    expect(screen.getByRole('button', { name: 'Manual sync' })).toBeEnabled();
    expect(props.onSync).not.toHaveBeenCalled();
  });

  it('retains the reserved service capability without bypassing in-flight gates or triggering a probe', () => {
    const view = { ...state(BBS_SYNC_ERROR_DETAILS.cloudflare_resource_limit), indicator: 'cloudflare_limit' as const,
      controlRecoverable: false, serviceRecoverable: true };
    const rendered = render(<BbsSyncStatusBar view={view} {...props} />);
    expect(screen.getByRole('button', { name: 'Manual sync' })).toBeEnabled();
    rendered.rerender(<BbsSyncStatusBar view={view} {...props} pending />);
    expect(screen.getByRole('button', { name: 'Manual sync' })).toBeDisabled();
    rendered.rerender(<BbsSyncStatusBar view={view} {...props} request="start" />);
    expect(screen.getByRole('button', { name: 'Starting…' })).toBeDisabled();
    rendered.rerender(<BbsSyncStatusBar view={{ ...view, phase: 'syncing' }} {...props} />);
    expect(screen.getByRole('button', { name: 'Syncing…' })).toBeDisabled();
    rendered.rerender(<BbsSyncStatusBar view={{ ...view, serviceRecoverable: false }} {...props} />);
    expect(screen.getByRole('button', { name: 'Manual sync' })).toBeDisabled();
    expect(props.onSync).not.toHaveBeenCalled();
  });
});
