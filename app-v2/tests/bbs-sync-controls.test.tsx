import { Profiler, type ReactNode } from 'react';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { BbsSyncScope, BbsSyncControlButton, BbsSyncActivity, BbsThreadSharing } from '../src/chrome/BbsSyncControls';
import type { BbsSyncView } from '../src/bbs-sync-view';
import type { BbsSyncInvitationResult } from '../src/types/bbs-sync';
import { BbsSyncClientError } from '../src/bbs-sync-client';
import { BBS_SYNC_ERROR_DETAILS } from '../src/bbs-sync-errors';

const code = (generation = '7') => `kota-bbs://test.example.invalid/join#opaque-generation-${generation}`;
const owner = (): BbsSyncView => ({
  deviceId: 'self', deviceName: 'Study Mac', workerAvailable: true,
  group: { id: 'one', name: 'Studio', role: 'owner', members: [
    { id: 'self', name: 'Study Mac', role: 'owner', online: true },
    { id: 'peer', name: 'Travel Mac', role: 'member', online: true },
  ] },
  invitation: { state: 'preparing' }, invitationGeneration: '7', phase: 'idle', progress: null,
  lastSuccessfulAt: null, error: null, controlRecoverable: false,
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
function harness(initial = owner()) {
  let state = initial;
  let hint: () => void = () => {};
  const stop = vi.fn();
  const source = {
    read: vi.fn(async () => state),
    listen: vi.fn(async (changed: () => void) => { hint = changed; return stop; }),
  };
  const actions = {
    invitation: vi.fn(async () => ({ groupId: state.group?.id ?? 'one', generation: state.invitationGeneration ?? '7', invitation: code(state.invitationGeneration ?? '7') })),
    join: vi.fn(async () => {}), disconnect: vi.fn(async () => {}), rename: vi.fn(async () => {}),
    remove: vi.fn(async () => {}), start: vi.fn(async () => {}), cancel: vi.fn(async () => {}),
  };
  return {
    source, actions, stop,
    update: async (next: BbsSyncView) => {
      state = next;
      hint();
      await act(async () => vi.advanceTimersByTimeAsync(500));
    },
  };
}
async function mount(task: ReturnType<typeof harness>, children?: ReactNode) {
  const result = render(<BbsSyncScope source={task.source} actions={task.actions}>
    <BbsSyncControlButton /><BbsSyncActivity />
    {children ?? <BbsThreadSharing sharingGroupId="one" />}
  </BbsSyncScope>);
  await act(async () => {});
  return result;
}
async function openManagement() {
  await act(async () => fireEvent.click(screen.getByRole('button', { name: /Manage .* connected devices|Connect devices to BBS/ })));
}
beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(new Date('2026-09-12T08:00:00Z')); });
afterEach(() => vi.useRealTimers());

describe('BBS real control composition', () => {
  it('allows manual recovery with zero peers, shows one error, and only clears it after authoritative recovery', async () => {
    const busy = { ...owner(), group: { ...owner().group!, members: [] }, phase: 'failed' as const,
      controlRecoverable: true, error: BBS_SYNC_ERROR_DETAILS.sync_busy };
    const task = harness(busy);
    task.actions.start.mockRejectedValueOnce(new BbsSyncClientError('sync_busy'));
    await mount(task);
    expect(screen.getByRole('button', { name: 'Retry sync' })).toBeEnabled();
    expect(screen.queryByText('No other device online now.')).not.toBeInTheDocument();
    expect(screen.queryByText('Sync failed')).not.toBeInTheDocument();
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Retry sync' })));
    expect(task.actions.start).toHaveBeenCalledExactlyOnceWith({ expectedGroupId: 'one' });
    expect(screen.getAllByText('Sync error:')).toHaveLength(1);
    expect(screen.getAllByText(BBS_SYNC_ERROR_DETAILS.sync_busy)).toHaveLength(1);
    await act(async () => vi.advanceTimersByTimeAsync(60_000));
    expect(task.actions.start).toHaveBeenCalledOnce();
    expect(screen.getByRole('button', { name: 'Retry sync' })).toBeEnabled();
    await task.update(owner());
    expect(screen.queryByText('Sync error:')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Manual sync' })).toBeEnabled();
  });

  it('gives immediate single-flight feedback through ACK and fresh read without manufacturing sync success', async () => {
    const task = harness();
    const start = deferred<void>(), read = deferred<BbsSyncView>();
    task.actions.start.mockReturnValueOnce(start.promise);
    await mount(task, <textarea aria-label="Draft" defaultValue="Keep typing" />);
    const editor = screen.getByRole('textbox', { name: 'Draft' });
    fireEvent.click(screen.getByRole('button', { name: 'Manual sync' }));
    const starting = screen.getByRole('button', { name: 'Starting…' });
    expect(starting).toBeDisabled(); expect(starting).toHaveAttribute('aria-busy', 'true');
    fireEvent.click(starting);
    expect(task.actions.start).toHaveBeenCalledOnce();
    task.source.read.mockReturnValueOnce(read.promise);
    await act(async () => start.resolve());
    await act(async () => vi.advanceTimersByTimeAsync(500));
    expect(screen.getByRole('button', { name: 'Starting…' })).toBeDisabled();
    expect(screen.getByText('Not synced yet')).toBeInTheDocument();
    expect(screen.queryByText(/^Syncing/)).not.toBeInTheDocument();
    await act(async () => read.resolve({ ...owner(), phase: 'syncing', progress: { completed: 0, total: 3 } }));
    expect(screen.getByRole('button', { name: 'Syncing 0/3' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeEnabled();
    expect(screen.getByRole('textbox', { name: 'Draft' })).toBe(editor);
    expect(editor).toHaveValue('Keep typing');
    const cancel = deferred<void>(); task.actions.cancel.mockReturnValueOnce(cancel.promise);
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.getByRole('button', { name: 'Cancelling…' })).toBeDisabled();
    await act(async () => cancel.resolve());
    await act(async () => vi.advanceTimersByTimeAsync(500));
    expect(task.actions.cancel).toHaveBeenCalledExactlyOnceWith({ expectedGroupId: 'one' });
    expect(screen.getByRole('button', { name: 'Manual sync' })).toBeEnabled();
  });

  it('merges different action/status errors and lets new backend state supersede the action error', async () => {
    const state = { ...owner(), phase: 'failed' as const, error: 'Cannot reach the sync service; check your connection and retry.' };
    const task = harness(state);
    task.actions.start.mockRejectedValueOnce(new BbsSyncClientError('sync_busy'));
    await mount(task);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Retry sync' })));
    expect(screen.getAllByText('Sync error:')).toHaveLength(1);
    expect(screen.queryByText(state.error)).not.toBeInTheDocument();
    expect(screen.getByText(BBS_SYNC_ERROR_DETAILS.sync_busy)).toBeInTheDocument();
    await task.update({ ...state });
    expect(screen.getByText(BBS_SYNC_ERROR_DETAILS.sync_busy)).toBeInTheDocument();
    await task.update({ ...state, error: 'A received file failed verification; click Retry to download it again.' });
    expect(screen.getAllByText('Sync error:')).toHaveLength(1);
    expect(screen.queryByText(BBS_SYNC_ERROR_DETAILS.sync_busy)).not.toBeInTheDocument();
    expect(screen.getByText(/A received file failed verification/)).toBeInTheDocument();
  });

  it('merges read failures with status errors and releases local feedback even when the post-ACK read fails', async () => {
    const state = { ...owner(), phase: 'failed' as const, error: BBS_SYNC_ERROR_DETAILS.sync_busy, controlRecoverable: true };
    const task = harness(state);
    await mount(task);
    task.source.read.mockRejectedValueOnce(new Error('PRIVATE status payload'));
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Retry sync' })));
    expect(screen.getByRole('button', { name: 'Starting…' })).toBeDisabled();
    await act(async () => vi.advanceTimersByTimeAsync(500));
    expect(screen.getAllByText('Sync error:')).toHaveLength(1);
    expect(screen.getByText('Could not refresh device sync status.')).toBeInTheDocument();
    expect(screen.queryByText(BBS_SYNC_ERROR_DETAILS.sync_busy)).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain('PRIVATE');
    expect(screen.getByRole('button', { name: 'Retry sync' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: 'Retry status' }));
    await act(async () => vi.advanceTimersByTimeAsync(500));
    expect(screen.getAllByText('Sync error:')).toHaveLength(1);
    expect(screen.getByText(BBS_SYNC_ERROR_DETAILS.sync_busy)).toBeInTheDocument();
    expect(task.actions.start).toHaveBeenCalledOnce();
  });

  it('does not clear an exchange error on command ACK or let a late rejection cover a newer round', async () => {
    const state = { ...owner(), phase: 'failed' as const, error: 'A received file failed verification; click Retry to download it again.' };
    const task = harness(state);
    await mount(task);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Retry sync' })));
    await act(async () => vi.advanceTimersByTimeAsync(500));
    expect(screen.getByText(state.error)).toBeInTheDocument();
    const late = deferred<void>(); task.actions.start.mockReturnValueOnce(late.promise);
    fireEvent.click(screen.getByRole('button', { name: 'Retry sync' }));
    await task.update({ ...owner(), phase: 'syncing', progress: { completed: 1, total: 2 } });
    await act(async () => late.reject(new BbsSyncClientError('sync_busy')));
    expect(screen.queryByText('Sync error:')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Syncing 1/2' })).toBeDisabled();
  });

  it('discards late rejection after group changes and preserves new-group controls', async () => {
    const task = harness();
    const late = deferred<void>(); task.actions.start.mockReturnValueOnce(late.promise);
    await mount(task);
    fireEvent.click(screen.getByRole('button', { name: 'Manual sync' }));
    const next = owner(); next.group!.id = 'new-group';
    await task.update(next);
    await act(async () => late.reject(new BbsSyncClientError('sync_busy')));
    expect(screen.queryByText('Sync error:')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Manual sync' })).toBeEnabled();
  });

  it.each(['stale_signature', 'worker_update_required', 'sync_busy'] as const)(
    'preserves the safe %s instruction through activity and invitation controls without retrying', async (code) => {
    const task = harness();
    const failure = new BbsSyncClientError(code);
    task.actions.start.mockRejectedValueOnce(failure);
    task.actions.invitation.mockRejectedValueOnce(failure);
    await mount(task);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Manual sync' })));
    expect(screen.getByText(failure.message)).toBeInTheDocument();
    await openManagement();
    expect(within(screen.getByRole('dialog')).getAllByText(failure.message)).toHaveLength(1);
    await act(async () => vi.advanceTimersByTimeAsync(15_000));
    expect(task.actions.start).toHaveBeenCalledOnce(); expect(task.actions.invitation).toHaveBeenCalledOnce();
  });

  it('shows one actionable error after explicit Create group and retries only on a user click', async () => {
    const local = { ...owner(), group: null, invitation: { state: 'none' as const }, invitationGeneration: null };
    const task = harness(local);
    const failure = new BbsSyncClientError('worker_update_required');
    task.actions.invitation.mockRejectedValueOnce(failure);
    await mount(task); await openManagement();
    const input = screen.getByRole('textbox', { name: 'Invitation' });
    fireEvent.change(input, { target: { value: code() } });
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Create group' })));
    const dialog = within(screen.getByRole('dialog'));
    expect(dialog.getAllByText(failure.message)).toHaveLength(1);
    expect(dialog.getByRole('alert')).toHaveTextContent(failure.message);
    expect(dialog.queryByText('Could not prepare an invitation. Please retry.')).not.toBeInTheDocument();
    expect(dialog.queryByRole('list', { name: 'Group members' })).not.toBeInTheDocument();
    expect(input).toHaveValue(code());
    expect(task.actions.invitation).toHaveBeenCalledExactlyOnceWith({ expectedGroupId: null, refresh: false });
    await act(async () => vi.advanceTimersByTimeAsync(15_000));
    expect(task.actions.invitation).toHaveBeenCalledOnce();
    expect(task.actions.join).not.toHaveBeenCalled();
    expect(task.actions.disconnect).not.toHaveBeenCalled();
    await act(async () => fireEvent.click(dialog.getByRole('button', { name: 'Create group' })));
    expect(task.actions.invitation).toHaveBeenCalledTimes(2);
    expect(dialog.queryByText(failure.message)).not.toBeInTheDocument();
    expect(input).toHaveValue(code());
  });

  it('reads public state on BBS open but only retrieves raw invitation when management opens', async () => {
    const task = harness();
    const result = await mount(task);
    expect(task.actions.invitation).not.toHaveBeenCalled();
    expect(task.actions.start).not.toHaveBeenCalled();
    expect(screen.getByText('Sharing on 2 devices')).toBeInTheDocument();
    expect(screen.queryByText(code())).not.toBeInTheDocument();
    await openManagement();
    expect(task.actions.invitation).toHaveBeenCalledExactlyOnceWith({ expectedGroupId: 'one', refresh: false });
    expect(screen.getByText(code())).toBeInTheDocument();
    await act(async () => vi.advanceTimersByTimeAsync(15_000));
    expect(task.actions.invitation).toHaveBeenCalledTimes(1);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Done' })));
    expect(screen.queryByText(code())).not.toBeInTheDocument();
    await openManagement();
    expect(task.actions.invitation).toHaveBeenCalledTimes(2);
    result.unmount();
    expect(task.stop).toHaveBeenCalledTimes(1);
    expect(task.actions.cancel).not.toHaveBeenCalled();
    expect(task.actions.disconnect).not.toHaveBeenCalled();
  });

  it('leaves unjoined BBS and member management read-only until an explicit action', async () => {
    const local = { ...owner(), group: null, invitation: { state: 'none' as const }, invitationGeneration: null };
    const task = harness(local);
    await mount(task);
    await openManagement();
    await act(async () => vi.advanceTimersByTimeAsync(60_000));
    expect(task.actions.invitation).not.toHaveBeenCalled();
    expect(task.source.read).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'Create group' })).toBeEnabled();
    const member = owner();
    member.group!.role = 'member'; member.invitationGeneration = null;
    await task.update(member);
    expect(task.actions.invitation).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'Create invitation' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Create group' })).not.toBeInTheDocument();
    expect(screen.queryByText(code())).not.toBeInTheDocument();
  });

  it('hides old generation immediately and retrieves the new generation once', async () => {
    const task = harness();
    await mount(task); await openManagement();
    const next = deferred<BbsSyncInvitationResult>();
    task.actions.invitation.mockReturnValueOnce(next.promise);
    await task.update({ ...owner(), invitationGeneration: '8' });
    expect(screen.queryByText(code())).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Copy invitation' })).not.toBeInTheDocument();
    expect(task.actions.invitation).toHaveBeenCalledTimes(2);
    await act(async () => next.resolve({ groupId: 'one', generation: '8', invitation: code('8') }));
    expect(screen.getByText(code('8'))).toBeInTheDocument();
    await task.update({ ...owner(), invitationGeneration: '8', progress: { completed: 1, total: 2 }, phase: 'syncing' });
    expect(task.actions.invitation).toHaveBeenCalledTimes(2);
  });

  it('never paints a late invitation from an earlier generation', async () => {
    const task = harness();
    const old = deferred<BbsSyncInvitationResult>();
    task.actions.invitation.mockReturnValueOnce(old.promise);
    await mount(task); await openManagement();
    await task.update({ ...owner(), invitationGeneration: '8' });
    await act(async () => old.resolve({ groupId: 'one', generation: '7', invitation: code('7') }));
    expect(screen.queryByText(code('7'))).not.toBeInTheDocument();
    expect(screen.getByText(code('8'))).toBeInTheDocument();
    expect(task.actions.invitation).toHaveBeenCalledTimes(2);
  });

  it('provides an explicit retry after retrieval failure without an automatic retry storm', async () => {
    const task = harness();
    task.actions.invitation.mockRejectedValueOnce(new Error('PRIVATE server dump'));
    await mount(task); await openManagement();
    expect(screen.getByText('Could not prepare an invitation. Please retry.')).toBeInTheDocument();
    expect(document.body.textContent).not.toContain('PRIVATE');
    await act(async () => vi.advanceTimersByTimeAsync(15_000));
    expect(task.actions.invitation).toHaveBeenCalledTimes(1);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Create invitation' })));
    expect(task.actions.invitation).toHaveBeenCalledTimes(2);
    expect(screen.getByRole('button', { name: 'Copy invitation' })).toBeEnabled();
  });

  it('hides an explicitly refreshed code until its authoritative generation catches up', async () => {
    const task = harness();
    await mount(task); await openManagement();
    task.actions.invitation.mockResolvedValueOnce({ groupId: 'one', generation: '8', invitation: code('8') });
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Refresh invitation code' })));
    expect(task.actions.invitation).toHaveBeenLastCalledWith({ expectedGroupId: 'one', refresh: true });
    expect(screen.queryByText(code('7'))).not.toBeInTheDocument();
    expect(screen.queryByText(code('8'))).not.toBeInTheDocument();
    await task.update({ ...owner(), invitationGeneration: '8' });
    expect(screen.getByText(code('8'))).toBeInTheDocument();
    expect(task.actions.invitation).toHaveBeenCalledTimes(2);
  });

  it('ignores a closed dialog response and obtains a fresh code when reopened', async () => {
    const task = harness();
    const old = deferred<BbsSyncInvitationResult>();
    task.actions.invitation.mockReturnValueOnce(old.promise);
    await mount(task); await openManagement();
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Close device sync' })));
    await act(async () => old.resolve({ groupId: 'one', generation: '7', invitation: 'PRIVATE old window' }));
    expect(screen.queryByText('PRIVATE old window')).not.toBeInTheDocument();
    await openManagement();
    expect(task.actions.invitation).toHaveBeenCalledTimes(2);
    expect(screen.getByText(code())).toBeInTheDocument();
  });

  it('uses group identity to reject a late response after changing groups', async () => {
    const task = harness();
    const old = deferred<BbsSyncInvitationResult>();
    task.actions.invitation.mockReturnValueOnce(old.promise);
    await mount(task); await openManagement();
    const nextGroup = owner(); nextGroup.group!.id = 'two'; nextGroup.invitationGeneration = '8';
    await task.update(nextGroup);
    await act(async () => old.resolve({ groupId: 'one', generation: '7', invitation: 'PRIVATE old group' }));
    expect(screen.queryByText('PRIVATE old group')).not.toBeInTheDocument();
    expect(task.actions.invitation).toHaveBeenLastCalledWith({ expectedGroupId: 'two', refresh: false });
    expect(screen.getByText(code('8'))).toBeInTheDocument();
  });

  it('keeps the editor/list out of progress renders and clears Sharing on disconnect', async () => {
    const task = harness();
    const bodyRendered = vi.fn(); const rowCommitted = vi.fn();
    function Body() {
      bodyRendered();
      return <><textarea aria-label="Post body" defaultValue="Keep my text" />
        <Profiler id="sharing" onRender={rowCommitted}><BbsThreadSharing sharingGroupId="one" /></Profiler>
        <BbsThreadSharing sharingGroupId="other" /></>;
    }
    await mount(task, <Body />);
    const rowCommitsAfterHydrate = rowCommitted.mock.calls.length;
    for (let completed = 1; completed <= 5; completed++) {
      await task.update({ ...owner(), phase: 'syncing', progress: { completed, total: 10 } });
    }
    expect(bodyRendered).toHaveBeenCalledTimes(1);
    expect(rowCommitted).toHaveBeenCalledTimes(rowCommitsAfterHydrate);
    expect(screen.getByRole('textbox', { name: 'Post body' })).toHaveValue('Keep my text');
    expect(screen.getAllByText('Sharing on 2 devices')).toHaveLength(1);
    await task.update({ ...owner(), group: null, invitationGeneration: null, invitation: { state: 'none' } });
    expect(screen.queryByText(/Sharing on/)).not.toBeInTheDocument();
    expect(bodyRendered).toHaveBeenCalledTimes(1);
  });

  it('treats start completion as command completion, not sync success, and stops only visible reads on unmount', async () => {
    const task = harness();
    const start = deferred<void>(); task.actions.start.mockReturnValueOnce(start.promise);
    const result = await mount(task);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Manual sync' })));
    expect(task.actions.start).toHaveBeenCalledExactlyOnceWith({ expectedGroupId: 'one' });
    expect(screen.getByRole('button', { name: 'Starting…' })).toBeDisabled();
    expect(screen.queryByText(/Sync success/)).not.toBeInTheDocument();
    result.unmount();
    const reads = task.source.read.mock.calls.length;
    await act(async () => start.resolve());
    await act(async () => vi.advanceTimersByTimeAsync(60_000));
    expect(task.actions.cancel).not.toHaveBeenCalled();
    expect(task.actions.disconnect).not.toHaveBeenCalled();
    expect(task.source.read).toHaveBeenCalledTimes(reads);
  });
});
