import { useRef, useState, type ComponentProps } from 'react';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { bbsInvitationHost, bbsSyncOnlinePeers, bbsSyncTimeLabel, type BbsSyncView } from '../src/bbs-sync-view';
import { BbsDeviceSyncDialog, BbsSharingMark, BbsSyncManageButton, BbsSyncStatusBar, type BbsDeviceSyncActions } from '../src/chrome/BbsDeviceSync';

function BbsSyncControls(props: ComponentProps<typeof BbsSyncManageButton> & ComponentProps<typeof BbsSyncStatusBar>) {
  return <><BbsSyncManageButton {...props} /><BbsSyncStatusBar {...props} /></>;
}

const invite = 'kota-bbs://test.demo.invalid/join#0123456789abcdef0123456789abcdef';
function view(overrides: Partial<BbsSyncView> = {}): BbsSyncView {
  return {
    deviceId: 'self', deviceName: 'My Mac', workerAvailable: true,
    group: {
      id: 'group-one', name: 'Studio', role: 'owner', members: [
        { id: 'self', name: 'My Mac', role: 'owner', online: true },
        { id: 'peer', name: 'Other Mac', role: 'member', online: true },
        { id: 'offline', name: 'Travel Mac', role: 'member', online: false },
      ],
    },
    invitation: { state: 'ready', value: invite }, phase: 'idle', progress: null,
    lastSuccessfulAt: null, error: null, controlRecoverable: false, ...overrides,
  };
}
function actions(): BbsDeviceSyncActions {
  return {
    createInvitation: vi.fn().mockResolvedValue(undefined), refreshInvitation: vi.fn().mockResolvedValue(undefined),
    copyInvitation: vi.fn().mockResolvedValue(undefined), join: vi.fn().mockResolvedValue(undefined),
    disconnect: vi.fn().mockResolvedValue(undefined), rename: vi.fn().mockResolvedValue(undefined), remove: vi.fn().mockResolvedValue(undefined),
  };
}
function deferred() {
  let resolve!: () => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<void>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

describe('BBS sync presentation helpers', () => {
  it('previews only an invitation host, not secret material or a navigation URL', () => {
    expect(bbsInvitationHost(` ${invite} `)).toBe('test.demo.invalid');
    for (const invalid of ['ABC1234', 'https://test.demo.invalid/join#secret', 'javascript:alert(1)',
      'kota-bbs://user:pass@test.demo.invalid/join#secret', 'kota-bbs://test.demo.invalid/join?x=1#secret',
      'kota-bbs://test.demo.invalid:9999/join#secret', 'kota-bbs://test.demo.invalid/join', `${invite}\nother`]) {
      expect(bbsInvitationHost(invalid)).toBeNull();
    }
  });

  it('counts online peers separately from membership and never invents a successful date', () => {
    expect(bbsSyncOnlinePeers(view())).toBe(1);
    expect(bbsSyncOnlinePeers(view({ group: null }))).toBe(0);
    expect(bbsSyncTimeLabel(null)).toBe('Not synced yet');
    expect(bbsSyncTimeLabel('invalid')).toBe('Not synced yet');
    expect(bbsSyncTimeLabel('2026-09-12T04:00:00Z')).toMatch(/^Last sync at /);
  });
});

describe('BBS sync controls', () => {
  it('is inert before joining and keeps sharing separate from delivery progress', () => {
    const onManage = vi.fn(), onSync = vi.fn(), onCancel = vi.fn();
    render(<><BbsSyncControls view={view({ group: null })} {...{ onManage, onSync, onCancel }} /><BbsSharingMark deviceCount={0} /></>);
    expect(screen.queryByText(/Sharing on/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Last sync|Not synced|Manual sync/)).not.toBeInTheDocument();
    expect(onManage).not.toHaveBeenCalled();
    expect(onSync).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Connect devices to BBS' }));
    expect(onManage).toHaveBeenCalledTimes(1);
  });

  it('keeps stale success time while updating, hides fake success, and offers cancellation', () => {
    const state = view({ phase: 'syncing', progress: { completed: 4, total: 12 }, lastSuccessfulAt: '2026-09-12T04:00:00Z' });
    const onSync = vi.fn(), onCancel = vi.fn();
    render(<><BbsSyncControls view={state} onManage={vi.fn()} {...{ onSync, onCancel }} /><BbsSharingMark deviceCount={3} /></>);
    expect(screen.getByText('Sharing on 3 devices')).toBeInTheDocument();
    expect(screen.getByText(/^Last sync at /)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Syncing 4/12' })).toBeDisabled();
    expect(screen.queryByText(/Sync success/i)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onSync).not.toHaveBeenCalled();
  });

  it('uses a discoverable disabled-button hint when everyone else is offline', () => {
    const state = view();
    state.group!.members = state.group!.members.map((member) => ({ ...member, online: member.id === 'self' }));
    render(<BbsSyncControls view={state} onManage={vi.fn()} onSync={vi.fn()} onCancel={vi.fn()} />);
    const button = screen.getByRole('button', { name: 'Manual sync' });
    expect(button).toBeDisabled();
    expect(button.parentElement).toHaveAttribute('tabindex', '0');
    expect(within(button.parentElement!).getByRole('tooltip', { hidden: true })).toHaveTextContent('No other device online now.');
    expect(screen.getByRole('button', { name: 'Manage 3 connected devices' })).toBeEnabled();
  });

  it('shows partial and failed results without replacing the last successful timestamp', () => {
    const onSync = vi.fn();
    const props = { onManage: vi.fn(), onSync, onCancel: vi.fn() };
    const rendered = render(<BbsSyncControls view={view({ phase: 'partial', lastSuccessfulAt: '2026-09-12T04:00:00Z' })} {...props} />);
    expect(screen.getByText('Partially synced')).toBeInTheDocument();
    expect(screen.getByText(/^Last sync at /)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Retry sync' }));
    expect(onSync).toHaveBeenCalledTimes(1);
    rendered.rerender(<BbsSyncControls view={view({ phase: 'failed', error: 'Could not establish a direct connection.' })} {...props} />);
    expect(screen.queryByText('Sync failed')).not.toBeInTheDocument();
    expect(screen.getByText('Sync error:')).toBeInTheDocument();
    expect(screen.getByText('Could not establish a direct connection.')).toBeInTheDocument();
    expect(screen.getByText('Not synced yet')).toBeInTheDocument();
  });
});

describe('BBS device management', () => {
  it('opens and closes from the BBS entry, restores focus, and calls no backend on render', async () => {
    const task = actions();
    function Harness() {
      const [open, setOpen] = useState(false);
      const opener = useRef<HTMLButtonElement | null>(null);
      return <><BbsSyncControls view={view()} expanded={open} onManage={(trigger) => { opener.current = trigger; setOpen(true); }} onSync={vi.fn()} onCancel={vi.fn()} />
        {open && <BbsDeviceSyncDialog view={view()} returnFocusTo={opener.current} actions={task} onClose={() => setOpen(false)} />}</>;
    }
    render(<Harness />);
    const button = screen.getByRole('button', { name: 'Manage 3 connected devices' });
    fireEvent.click(button); // WebKit mouse clicks do not necessarily focus buttons.
    expect(screen.getByRole('dialog', { name: 'Connect other devices to BBS' })).toHaveAttribute('open');
    expect(button).toHaveAttribute('aria-expanded', 'true');
    for (const action of Object.values(task)) expect(action).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Done' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(button).toHaveFocus();
    await userEvent.click(button);
    fireEvent(screen.getByRole('dialog'), new Event('cancel', { bubbles: true, cancelable: true }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(button).toHaveFocus();
  });

  it('displays and copies the exact full code; a refreshing code is not copyable', async () => {
    const task = actions();
    const rendered = render(<BbsDeviceSyncDialog view={view()} actions={task} onClose={vi.fn()} />);
    expect(screen.getByText(invite).tagName).toBe('CODE');
    await userEvent.click(screen.getByRole('button', { name: 'Copy invitation' }));
    expect(task.copyInvitation).toHaveBeenCalledWith(invite);
    expect(await screen.findByRole('status')).toHaveTextContent('Invitation copied.');
    await userEvent.click(screen.getByRole('button', { name: 'Refresh invitation code' }));
    expect(task.refreshInvitation).toHaveBeenCalledTimes(1);
    rendered.rerender(<BbsDeviceSyncDialog view={view({ invitation: { state: 'preparing' } })} actions={task} onClose={vi.fn()} />);
    expect(screen.queryByText(invite)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Copy invitation' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Refresh invitation code' })).toBeDisabled();
  });

  it('lets a device without Worker join but never create an invitation', async () => {
    const task = actions();
    render(<BbsDeviceSyncDialog view={view({ group: null, workerAvailable: false, invitation: { state: 'unavailable' } })} actions={task} onClose={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'Create group' })).toBeDisabled();
    await userEvent.type(screen.getByRole('textbox', { name: 'Invitation' }), invite);
    await userEvent.click(screen.getByRole('button', { name: 'Join' }));
    expect(screen.getByRole('dialog', { name: 'Join this BBS group?' })).toBeInTheDocument();
    expect(screen.getByText('Worker: test.demo.invalid')).toBeInTheDocument();
    expect(task.join).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Join' }));
    expect(task.join).toHaveBeenCalledWith(invite);
    expect(task.createInvitation).not.toHaveBeenCalled();
  });

  it('keeps the input after a failed join and does not locally manufacture membership', async () => {
    const task = actions();
    vi.mocked(task.join).mockRejectedValue(new Error('Invitation expired. Copy a new invitation.'));
    render(<BbsDeviceSyncDialog view={view({ group: null })} actions={task} onClose={vi.fn()} />);
    await userEvent.type(screen.getByRole('textbox', { name: 'Invitation' }), invite);
    await userEvent.click(screen.getByRole('button', { name: 'Join' }));
    await userEvent.click(screen.getByRole('button', { name: 'Join' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Invitation expired.');
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.getByRole('textbox', { name: 'Invitation' })).toHaveValue(invite);
    expect(screen.queryByRole('list', { name: 'Group members' })).not.toBeInTheDocument();
  });

  it('rejects incomplete invitations without a backend call', async () => {
    const task = actions();
    render(<BbsDeviceSyncDialog view={view({ group: null })} actions={task} onClose={vi.fn()} />);
    await userEvent.type(screen.getByRole('textbox', { name: 'Invitation' }), 'ABC1234');
    await userEvent.click(screen.getByRole('button', { name: 'Join' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Paste the complete invitation');
    expect(task.join).not.toHaveBeenCalled();
  });

  it('does not expose owner invitation or removal controls to a member, even if supplied in props', () => {
    const state = view();
    state.group!.role = 'member';
    render(<BbsDeviceSyncDialog view={state} actions={actions()} onClose={vi.fn()} />);
    expect(screen.getByText('Invitations are managed by the group owner.')).toBeInTheDocument();
    expect(screen.queryByText(invite)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Remove/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Refresh invitation code' })).not.toBeInTheDocument();
    expect(screen.getAllByRole('listitem')).toHaveLength(3);
  });

  it('keeps the join route for a single-member owner group', () => {
    const state = view();
    state.group!.members = state.group!.members.slice(0, 1);
    render(<BbsDeviceSyncDialog view={state} actions={actions()} onClose={vi.fn()} />);
    expect(screen.getByRole('textbox', { name: 'Invitation' })).toBeInTheDocument();
    expect(screen.getByText(invite)).toBeInTheDocument();
  });

  it('requires confirmation for removal and keeps the member until authoritative state changes', async () => {
    const task = actions();
    render(<BbsDeviceSyncDialog view={view()} actions={task} onClose={vi.fn()} />);
    await userEvent.click(screen.getByRole('button', { name: 'Remove Other Mac' }));
    expect(screen.getByRole('dialog', { name: 'Remove Other Mac?' })).toBeInTheDocument();
    expect(task.remove).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Remove' }));
    expect(task.remove).toHaveBeenCalledWith('peer');
    expect(await screen.findByText('Other Mac')).toBeInTheDocument();
  });

  it('distinguishes owner dissolution and does not duplicate a pending disconnect', async () => {
    const task = actions();
    const job = deferred();
    vi.mocked(task.disconnect).mockReturnValue(job.promise);
    render(<BbsDeviceSyncDialog view={view()} actions={task} onClose={vi.fn()} />);
    await userEvent.click(screen.getByRole('button', { name: 'Disconnect' }));
    expect(screen.getByRole('dialog', { name: 'Disconnect and dissolve this group?' })).toBeInTheDocument();
    expect(screen.getByText(/Downloaded posts, replies, and attachments remain on every device/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Disconnect' }));
    expect(screen.getByRole('button', { name: 'Working…' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Working…' }));
    expect(task.disconnect).toHaveBeenCalledTimes(1);
    await act(async () => job.resolve());
  });

  it('does not overwrite a typed device name on status refresh and validates empty names', async () => {
    const task = actions();
    const rendered = render(<BbsDeviceSyncDialog view={view()} actions={task} onClose={vi.fn()} />);
    const input = screen.getByRole('textbox', { name: 'This device’s name' });
    await userEvent.clear(input);
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
    await userEvent.type(input, 'Writing Mac');
    rendered.rerender(<BbsDeviceSyncDialog view={view({ phase: 'syncing' })} actions={task} onClose={vi.fn()} />);
    expect(input).toHaveValue('Writing Mac');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(task.rename).toHaveBeenCalledWith('Writing Mac');
  });

  it('drops a late rejected action after closing without touching the reopened panel', async () => {
    const task = actions();
    const job = deferred();
    vi.mocked(task.copyInvitation).mockReturnValueOnce(job.promise);
    const first = render(<BbsDeviceSyncDialog view={view()} actions={task} onClose={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Copy invitation' }));
    first.unmount();
    render(<BbsDeviceSyncDialog view={view({ group: null })} actions={task} onClose={vi.fn()} />);
    await act(async () => job.reject(new Error('Old clipboard failure')));
    expect(screen.queryByText('Old clipboard failure')).not.toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'Invitation' })).toHaveValue('');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Copy invitation' })).toBeEnabled());
  });
});
