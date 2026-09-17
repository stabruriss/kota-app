import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../src/bbs-sync-client', async (original) => {
  const actual = await original<typeof import('../src/bbs-sync-client')>();
  return { ...actual,
    bbsSyncViewSource: { read: vi.fn(), listen: vi.fn() },
    bbsSyncInvitation: vi.fn(), bbsSyncJoin: vi.fn(), bbsSyncDisconnect: vi.fn(),
    bbsSyncRename: vi.fn(), bbsSyncRemove: vi.fn(), bbsSyncStart: vi.fn(), bbsSyncCancel: vi.fn(),
  };
});
import * as client from '../src/pty-client';
import * as sync from '../src/bbs-sync-client';
import { RightColumn } from '../src/chrome/RightColumn';
import { bbsSyncTimeLabel, type BbsSyncView } from '../src/bbs-sync-view';
import managerProgress from './fixtures/bbs-sync-manager-progress.json';
import retryRecovery from './fixtures/bbs-retry-recovery.json';
import h3ManagerRecovery from './fixtures/bbs-h3-manager-recovery.json';
import manualAdmission from './fixtures/bbs-manual-admission.json';
import indicatorRuntime from './fixtures/bbs-sync-indicator-runtime.json';
import indicatorRecovery from './fixtures/bbs-sync-indicator-recovery.json';
import membershipRejection from './fixtures/bbs-sync-membership-rejection.json';
import { BBS_SYNC_INDICATOR_COPY } from '../src/bbs-sync-indicator';
import { isBbsSyncSafeActionErrorCode } from '../src/bbs-sync-errors';

const workspace: client.WorkspaceProject = {
  projectId: 'bbs-test', repoFullName: 'mock/bbs-test', remoteUrl: '', githubHtmlUrl: '', defaultBranch: 'main', baseRef: 'main',
  localRoot: '/tmp/bbs-test', localRootBytes: 0, sourceDir: '/tmp/bbs-test/source', sourceDirBytes: 0,
  sharedDir: '/tmp/bbs-test/project-memory', rulesDir: '/tmp/bbs-test/rules', agents: [],
};
const board: client.BbsSnapshot = {
  projectId: workspace.projectId, projectDisplayName: 'Test', root: '/tmp/bbs', newCount: 0,
  threads: [{ threadId: 'thread-one', sharingGroupId: 'one', visibility: 'broadcast', projectTags: [], projectTagLabels: [],
    createdByProject: workspace.projectId, createdByProjectLabel: 'Test', updatedAt: '2026-09-12T00:00:00Z', latestPostId: 'post-one',
    isNew: false, relevant: true, posts: [{ postId: 'post-one', threadId: 'thread-one', projectId: workspace.projectId,
      projectDisplayName: 'Test', agentId: 'human', agentDisplayName: 'User', createdAt: '2026-09-12T00:00:00Z',
      kind: 'topic', body: 'Shared thread', preview: 'Shared thread', state: 'none', external: false }],
  }],
};
const invite = 'kota-bbs://test.example.invalid/join#0123456789abcdef0123456789abcdef';
let state: BbsSyncView;
let hint: () => void;
let stop: ReturnType<typeof vi.fn>;
beforeEach(() => {
  vi.restoreAllMocks(); vi.resetAllMocks(); window.localStorage.clear();
  state = {
    deviceId: 'self', deviceName: 'Study Mac', workerAvailable: true,
    group: { id: 'one', name: 'Studio', role: 'owner', members: [
      { id: 'self', name: 'Study Mac', role: 'owner', online: true }, { id: 'peer', name: 'Travel Mac', role: 'member', online: true },
    ] },
    invitation: { state: 'preparing' }, invitationGeneration: '7', phase: 'idle', progress: null, lastSuccessfulAt: null, error: null, controlRecoverable: false, serviceRecoverable: false,
  };
  stop = vi.fn(); hint = () => {};
  vi.mocked(sync.bbsSyncViewSource.read).mockImplementation(async () => state);
  vi.mocked(sync.bbsSyncViewSource.listen).mockImplementation(async (changed) => { hint = changed; return stop; });
  vi.mocked(sync.bbsSyncInvitation).mockResolvedValue({ groupId: 'one', generation: '7', invitation: invite });
  vi.spyOn(client, 'bbsSnapshot').mockResolvedValue(board);
});

describe('BBS product entry device controls', () => {
  it('replays the actual indicator window, real progress and multi-peer debt through RightColumn', async () => {
    let payload: unknown = indicatorRuntime.healthy;
    vi.mocked(sync.bbsSyncViewSource.read).mockImplementation(async () => (
      sync.bbsSyncStatusView(sync.parseBbsSyncStatus(payload))
    ));
    render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />);
    await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
    const element = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    const dialog = within(element);
    await dialog.findByText('1 online');
    const last = dialog.getByText(/^Last sync at /).textContent;
    await userEvent.click(await dialog.findByRole('button', { name: /Shared thread/ }));
    const editor = dialog.getByTestId('input-field');
    await userEvent.type(editor, 'Keep this reply throughout recovery');
    const scans = vi.mocked(client.bbsSnapshot).mock.calls.length;
    for (const [stage, expected, button] of [
      ['connecting', 'connecting', 'Manual sync'],
      ['syncing', 'healthy', 'Syncing 1/3'],
      ['recovering', 'connecting', 'Manual sync'],
      ['afterWindow', 'fetching_session', 'Manual sync'],
      ['startedWithDebt', 'fetching_session', 'Syncing…'],
    ] as const) {
      payload = indicatorRuntime[stage]; act(() => hint());
      await waitFor(() => expect(element.querySelector('.bbs-sync-indicator')).toHaveAttribute('data-indicator', expected));
      expect(await dialog.findByRole('button', { name: button })).toBeInTheDocument();
      expect(dialog.getByText(/^Last sync at /).textContent).toBe(last);
      expect(element.querySelectorAll('.bbs-sync-indicator')).toHaveLength(1);
      expect(dialog.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
      expect(dialog.queryByText(indicatorRuntime.afterWindow.sync.error)).not.toBeInTheDocument();
      expect(dialog.getByTestId('input-field')).toBe(editor);
      expect(editor).toHaveTextContent('Keep this reply throughout recovery');
    }
    for (const [stage, expected] of [
      ['completed', 'healthy'], ['multiplePeersBefore', 'reaching_peers'], ['multiplePeersAfter', 'reaching_peers'],
    ] as const) {
      payload = indicatorRuntime[stage]; act(() => hint());
      await waitFor(() => expect(element.querySelector('.bbs-sync-indicator')).toHaveAttribute('data-indicator', expected));
      expect(await dialog.findByRole('button', { name: 'Manual sync' })).toBeInTheDocument();
      await waitFor(() => expect(dialog.getByText(/^Last sync at /)).toHaveTextContent(
        bbsSyncTimeLabel(indicatorRuntime[stage].sync.lastSuccessfulAt),
      ));
    }
    // Another peer finished; its Last sync is real, but the failed peer stays yellow.
    expect(element.querySelector('.bbs-sync-indicator')).toHaveAttribute('data-tone', 'yellow');
    expect(dialog.getByText(/^Last sync at /).textContent).not.toBe(last);
    expect(client.bbsSnapshot).toHaveBeenCalledTimes(scans);
    await userEvent.click(dialog.getByRole('button', { name: 'Close Bulletin Board' }));
    await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
    const reopened = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    await waitFor(() => expect(reopened.querySelector('.bbs-sync-indicator')).toHaveAttribute('data-indicator', 'reaching_peers'));
    expect(sync.bbsSyncStart).not.toHaveBeenCalled();
    expect(sync.bbsSyncCancel).not.toHaveBeenCalled();
    expect(sync.bbsSyncInvitation).not.toHaveBeenCalled();
  }, 15000);

  it('uses every actually emitted cause, including blockers with no group, without reading English errors', async () => {
    let payload: unknown = indicatorRuntime.healthy;
    vi.mocked(sync.bbsSyncViewSource.read).mockImplementation(async () => (
      sync.bbsSyncStatusView(sync.parseBbsSyncStatus(payload))
    ));
    render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />);
    await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
    const element = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    for (const value of Object.values(indicatorRuntime.causes)) {
      const parsed = sync.parseBbsSyncStatus(value);
      const key = parsed.sync.indicator!;
      payload = value; act(() => hint());
      await waitFor(() => expect(element.querySelector('.bbs-sync-indicator')).toHaveAttribute('data-indicator', key));
      const indicator = element.querySelector('.bbs-sync-indicator')!;
      expect(indicator).toHaveAttribute('data-tone', BBS_SYNC_INDICATOR_COPY[key].tone);
      expect(within(indicator as HTMLElement).getByText(BBS_SYNC_INDICATOR_COPY[key].text)).toBeInTheDocument();
      if (value.group.id === null) expect(within(element).queryByRole('button', { name: 'Manual sync' })).not.toBeInTheDocument();
      expect(within(element).queryByText(value.sync.error)).not.toBeInTheDocument();
    }
    // file_access_error is reserved: do not invent a backend fixture without errno evidence.
    expect(Object.keys(indicatorRuntime.causes)).not.toContain('file_access_error');
    expect(sync.bbsSyncStart).not.toHaveBeenCalled();
    expect(sync.bbsSyncCancel).not.toHaveBeenCalled();
  }, 15000);

  it('replays actual lease recovery and actual retired membership without hiding the blocker', async () => {
    let payload: unknown = indicatorRecovery.busy;
    vi.mocked(sync.bbsSyncViewSource.read).mockImplementation(async () => (
      sync.bbsSyncStatusView(sync.parseBbsSyncStatus(payload))
    ));
    const code = indicatorRecovery.busyError.code;
    if (!isBbsSyncSafeActionErrorCode(code)) throw new Error('Unexpected real action fixture');
    vi.mocked(sync.bbsSyncStart).mockRejectedValueOnce(new sync.BbsSyncClientError(code));
    render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />);
    await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
    const element = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    const dialog = within(element);
    expect(await dialog.findByText('1 Kota per device')).toBeInTheDocument();
    await userEvent.click(dialog.getByRole('button', { name: 'Manual sync' }));
    await waitFor(() => expect(sync.bbsSyncStart).toHaveBeenCalledOnce());
    expect(element.querySelector('.bbs-sync-indicator')).toHaveAttribute('data-tone', 'red');
    expect(dialog.getByText('Not synced yet')).toBeInTheDocument();
    vi.mocked(sync.bbsSyncStart).mockImplementationOnce(async () => { payload = indicatorRecovery.recovered; });
    await userEvent.click(dialog.getByRole('button', { name: 'Manual sync' }));
    await waitFor(() => expect(element.querySelector('.bbs-sync-indicator')).toHaveAttribute('data-indicator', 'healthy'));
    expect(dialog.getByText('Not synced yet')).toBeInTheDocument();
    expect(dialog.getByRole('button', { name: 'Manual sync' })).toBeDisabled();
    for (const stage of ['controlOverExchange', 'retainedExchange', 'started'] as const) {
      payload = indicatorRecovery[stage]; act(() => hint());
      await waitFor(() => expect(element.querySelector('.bbs-sync-indicator')).toHaveAttribute('data-indicator', 'connecting'));
      expect(dialog.getByText('Not synced yet')).toBeInTheDocument();
    }
    payload = membershipRejection; act(() => hint());
    expect(await dialog.findByText('Group access denied')).toBeInTheDocument();
    expect(element.querySelector('.bbs-sync-indicator')).toHaveAttribute('data-tone', 'red');
    expect(dialog.getByRole('button', { name: 'Connect devices to BBS' })).toBeEnabled();
    expect(dialog.queryByRole('button', { name: 'Manual sync' })).not.toBeInTheDocument();
    expect(dialog.queryByText(/\d+ online/)).not.toBeInTheDocument();
    expect(sync.bbsSyncStart).toHaveBeenCalledTimes(2);
    expect(sync.bbsSyncDisconnect).not.toHaveBeenCalled();
    expect(sync.bbsSyncCancel).not.toHaveBeenCalled();
  });

  it('replays the real H3 Manager recovery fixture with mutually exclusive recovery flags', () => {
    // Captured from c737264+198ef1a Manager output; fixture SHA-256:
    // 56c7cdcdf720a552026ff0412e174e7ae26c733a0576cf016899e72113ab2d33.
    const stages = ['busy', 'recovered', 'started', 'controlOverExchange', 'retainedExchange'] as const;
    for (const stage of stages) {
      const parsed = sync.parseBbsSyncStatus(h3ManagerRecovery[stage]);
      expect(parsed.sync.serviceRecoverable).toBe(false);
      expect(parsed.sync.controlRecoverable).toBe(stage === 'busy');
      expect(parsed.sync.controlRecoverable && parsed.sync.serviceRecoverable).toBe(false);
    }
  });

  it('keeps real admitted work visible after ACK and while waiting, without a phantom retry or progress', async () => {
    // Actual Manager event/admission output. Only IPC delivery is stubbed; this
    // uses the original parser, view, subscription and full RightColumn entry.
    let stage: keyof typeof manualAdmission = 'before';
    vi.mocked(sync.bbsSyncViewSource.read).mockImplementation(async () => (
      sync.bbsSyncStatusView(sync.parseBbsSyncStatus(manualAdmission[stage]))
    ));
    vi.mocked(sync.bbsSyncStart).mockImplementationOnce(async () => { stage = 'accepted'; });
    render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />);
    expect(sync.bbsSyncViewSource.read).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
    const dialog = within(await screen.findByRole('dialog', { name: 'Bulletin Board' }));
    await userEvent.click(await dialog.findByRole('button', { name: /Shared thread/ }));
    const editor = dialog.getByTestId('input-field');
    await userEvent.type(editor, 'Keep this draft during sync');
    const scans = vi.mocked(client.bbsSnapshot).mock.calls.length;
    await userEvent.click(await dialog.findByRole('button', { name: 'Manual sync' }));
    expect(await dialog.findByRole('button', { name: 'Manual sync' })).toBeDisabled();
    stage = 'waiting';
    act(() => hint());
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 650)); });
    expect(dialog.getByRole('button', { name: 'Manual sync' })).toBeDisabled();
    expect(dialog.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
    expect(dialog.getByRole('button', { name: 'Manual sync' })).toBeDisabled();
    expect(dialog.queryByText(/^Syncing/)).not.toBeInTheDocument();
    expect(sync.bbsSyncStart).toHaveBeenCalledExactlyOnceWith({ expectedGroupId: manualAdmission.accepted.group.id });
    stage = 'progress';
    act(() => hint());
    expect(await dialog.findByRole('button', { name: 'Syncing 0/3' })).toBeDisabled();
    expect(dialog.getByTestId('input-field')).toBe(editor);
    expect(editor).toHaveTextContent('Keep this draft during sync');
    expect(client.bbsSnapshot).toHaveBeenCalledTimes(scans);
    // Internal cancellation still projects normally; the UI no longer offers it.
    stage = 'cancelled'; act(() => hint());
    expect(await dialog.findByRole('button', { name: 'Manual sync' })).toBeEnabled();
    expect(sync.bbsSyncCancel).not.toHaveBeenCalled();
  });

  it('renders real Manager recovery and retained exchange errors through the existing board', async () => {
    // Unmodified KOTA_BBS_RETRY_RECOVERY_FIXTURE output (4,858 bytes).
    // Backend producer: cbeff14cd4c0640b816448ff33b2fa7869349c71.
    // SHA-256: aad3fa1b3a3c6c50d2c975640c3fe9225573a4073eb12bb112faab2c0464c036.
    // Status delivery/actions are stubbed here; parser, view, hook and RightColumn
    // are real. The raw busyError -> action client path is tested in bbs-sync-client.
    let stage: 'busy' | 'recovered' | 'controlOverExchange' | 'retainedExchange' | 'started' = 'busy';
    vi.mocked(sync.bbsSyncViewSource.read).mockImplementation(async () => (
      sync.bbsSyncStatusView(sync.parseBbsSyncStatus(retryRecovery[stage]))
    ));
    vi.mocked(sync.bbsSyncStart).mockRejectedValueOnce(new sync.BbsSyncClientError('sync_busy'));
    render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />);
    expect(sync.bbsSyncViewSource.read).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
    const boardDialog = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    const dialog = within(boardDialog);
    expect(await dialog.findByRole('button', { name: 'Manual sync' })).toBeEnabled();
    expect(dialog.getByRole('button', { name: 'Manage 0 connected devices' })).toBeEnabled();
    expect(dialog.queryByText('No other device online now.')).not.toBeInTheDocument();
    await userEvent.click(await dialog.findByRole('button', { name: /Shared thread/ }));
    const editor = dialog.getByTestId('input-field');
    await userEvent.type(editor, 'Keep drafting while Retry restores sync');
    const scans = vi.mocked(client.bbsSnapshot).mock.calls.length;
    await userEvent.click(dialog.getByRole('button', { name: 'Manual sync' }));
    expect(sync.bbsSyncStart).toHaveBeenCalledExactlyOnceWith({ expectedGroupId: retryRecovery.busy.group.id });
    expect(boardDialog.querySelectorAll('.bbs-sync-indicator')).toHaveLength(1);
    expect(dialog.getAllByText('Sync is busy; try again shortly.')).toHaveLength(1);
    expect(dialog.queryByText('Sync failed')).not.toBeInTheDocument();

    vi.mocked(sync.bbsSyncStart).mockImplementationOnce(async () => { stage = 'recovered'; });
    await userEvent.click(dialog.getByRole('button', { name: 'Manual sync' }));
    expect(dialog.getByRole('button', { name: 'Starting…' })).toBeDisabled();
    expect(await dialog.findByRole('button', { name: 'Manage 2 connected devices' })).toBeEnabled();
    await waitFor(() => expect(boardDialog.querySelector('.bbs-sync-indicator')).toHaveAttribute('data-indicator', 'healthy'));
    // The producer intentionally keeps its fake peer offline (no STUN/PC).
    expect(dialog.getByRole('button', { name: 'Manual sync' })).toBeDisabled();
    expect(dialog.getByText('Not synced yet')).toBeInTheDocument();

    for (const next of ['controlOverExchange', 'retainedExchange'] as const) {
      stage = next; act(() => hint());
      expect(await dialog.findByText(retryRecovery[next].sync.error)).toBeInTheDocument();
      expect(boardDialog.querySelectorAll('.bbs-sync-indicator')).toHaveLength(1);
      expect(dialog.getByRole('button', { name: 'Manual sync' })).toBeDisabled();
    }
    stage = 'started'; act(() => hint());
    expect(await dialog.findByRole('button', { name: 'Syncing…' })).toBeDisabled();
    expect(dialog.queryByText('Sync error:')).not.toBeInTheDocument();
    expect(dialog.getByText('Not synced yet')).toBeInTheDocument();
    expect(dialog.getByTestId('input-field')).toBe(editor);
    expect(editor).toHaveTextContent('Keep drafting while Retry restores sync');
    expect(client.bbsSnapshot).toHaveBeenCalledTimes(scans);
    expect(sync.bbsSyncStart).toHaveBeenCalledTimes(2);
    expect(sync.bbsSyncInvitation).not.toHaveBeenCalled();
    await userEvent.click(dialog.getByRole('button', { name: 'Close Bulletin Board' }));
    expect(stop).toHaveBeenCalledOnce();
    expect(sync.bbsSyncCancel).not.toHaveBeenCalled();
    expect(sync.bbsSyncDisconnect).not.toHaveBeenCalled();
  });

  it('opens/closes management from the existing board and never starts sync on opening', async () => {
    render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />);
    expect(sync.bbsSyncViewSource.read).not.toHaveBeenCalled();
    expect(sync.bbsSyncViewSource.listen).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
    const boardDialog = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    const manage = await within(boardDialog).findByRole('button', { name: 'Manage 2 connected devices' });
    expect(within(boardDialog).getByText('All threads')).toBeInTheDocument();
    expect(sync.bbsSyncInvitation).not.toHaveBeenCalled();
    expect(sync.bbsSyncStart).not.toHaveBeenCalled();
    await userEvent.click(manage);
    const management = await screen.findByRole('dialog', { name: 'Connect other devices to BBS' });
    expect(await within(management).findByText(invite)).toBeInTheDocument();
    await userEvent.click(within(management).getByRole('button', { name: 'Done' }));
    expect(manage).toHaveFocus();
    expect(screen.queryByText(invite)).not.toBeInTheDocument();
    await userEvent.click(within(boardDialog).getByRole('button', { name: 'Close Bulletin Board' }));
    expect(stop).toHaveBeenCalledTimes(1);
    expect(sync.bbsSyncDisconnect).not.toHaveBeenCalled();
    expect(sync.bbsSyncCancel).not.toHaveBeenCalled();
  });

  it('clears Sharing from the existing detail while preserving its editor and stale board snapshot', async () => {
    render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />);
    await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
    const dialog = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    await userEvent.click(await within(dialog).findByRole('button', { name: /Shared thread/ }));
    expect(within(dialog).getByText('Sharing on 2 devices')).toBeInTheDocument();
    const editor = within(dialog).getByTestId('input-field');
    await userEvent.type(editor, 'Keep this reply');
    const scans = vi.mocked(client.bbsSnapshot).mock.calls.length;
    state = { ...state, group: null, invitationGeneration: null, invitation: { state: 'none' } };
    act(() => hint());
    await waitFor(() => expect(within(dialog).queryByText(/Sharing on/)).not.toBeInTheDocument());
    expect(within(dialog).getByTestId('input-field')).toBe(editor);
    expect(editor).toHaveTextContent('Keep this reply');
    expect(client.bbsSnapshot).toHaveBeenCalledTimes(scans);
    expect(board.threads[0].sharingGroupId).toBe('one');
  });

  it('renders actual Rust Manager progress through the parser and board without inferring completion', async () => {
    // Unmodified KOTA_BBS_SYNC_MANAGER_PROGRESS_FIXTURE output from
    // b445ade73c0aaf0942758e5fda134d457342dc6a, manager/tests.rs:
    // progress_projects_directional_counts_before_finished_summary.
    // Fixture SHA-256: c7a1f0cc31ca602fbd3368cb56176424b98e6ac5decd826899ddd0ba77dec058.
    // Only delivery is stubbed: parser, view mapping, subscription and BBS entry are real.
    let index = 0;
    vi.mocked(sync.bbsSyncViewSource.read).mockImplementation(async () => (
      sync.bbsSyncStatusView(sync.parseBbsSyncStatus(managerProgress[index]))
    ));
    render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />);
    expect(sync.bbsSyncViewSource.read).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
    const dialog = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    expect(await within(dialog).findByRole('button', { name: 'Syncing 0/3' })).toBeDisabled();
    const lastSuccessful = within(dialog).getByText(/^Last sync at /).textContent;
    await userEvent.click(await within(dialog).findByRole('button', { name: /Shared thread/ }));
    const editor = within(dialog).getByTestId('input-field');
    await userEvent.type(editor, 'Keep typing during sync');
    const scans = vi.mocked(client.bbsSnapshot).mock.calls.length;

    for (const [next, label] of [[1, 'Syncing 1/3'], [2, 'Syncing 3/3']] as const) {
      index = next;
      act(() => hint());
      await waitFor(() => expect(within(dialog).getByRole('button', { name: label })).toBeDisabled());
      expect(within(dialog).getByText(/^Last sync at /).textContent).toBe(lastSuccessful);
      expect(within(dialog).getByTestId('input-field')).toBe(editor);
      expect(editor).toHaveTextContent('Keep typing during sync');
      expect(client.bbsSnapshot).toHaveBeenCalledTimes(scans);
    }
    // Receive-direction 3/3 is still Syncing until a later Finished event.
    expect(within(dialog).queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole('button', { name: 'Manual sync' })).not.toBeInTheDocument();
    expect(sync.bbsSyncStart).not.toHaveBeenCalled();
    expect(sync.bbsSyncInvitation).not.toHaveBeenCalled();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Close Bulletin Board' }));
    expect(stop).toHaveBeenCalledTimes(1);
    expect(sync.bbsSyncCancel).not.toHaveBeenCalled();
    expect(sync.bbsSyncDisconnect).not.toHaveBeenCalled();
  });
});
