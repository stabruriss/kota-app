import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', async original => ({ ...await original<typeof import('@tauri-apps/api/core')>(), invoke: vi.fn(), isTauri: vi.fn() }));
vi.mock('../src/bbs-sync-client', async original => ({ ...await original<typeof import('../src/bbs-sync-client')>(),
  bbsSyncViewSource: { read: vi.fn(), listen: vi.fn() } }));
vi.mock('../src/bbs-roster-client', async original => ({ ...await original<typeof import('../src/bbs-roster-client')>(),
  bbsRosterSource: { read: vi.fn(), listen: vi.fn() } }));
import * as client from '../src/pty-client';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { bbsSyncViewSource } from '../src/bbs-sync-client';
import { bbsRosterSource, parseBbsRosterPage, readCompleteBbsRoster } from '../src/bbs-roster-client';
import { BbsMentionClientError } from '../src/bbs-mentions';
import { RightColumn } from '../src/chrome/RightColumn';
import type { BbsSyncView } from '../src/bbs-sync-view';
import type { BbsRosterRow } from '../src/types/bbs-roster';
import mentionFixture from './fixtures/bbs-mention-runtime.json';

const actualHumanPost = client.bbsHumanPost;

const workspace: client.WorkspaceProject = {
  projectId: 'p', repoFullName: 'mock/bbs-test', remoteUrl: '', githubHtmlUrl: '', defaultBranch: 'main', baseRef: 'main',
  localRoot: '/tmp/bbs-test', localRootBytes: 0, sourceDir: '/tmp/bbs-test/source', sourceDirBytes: 0,
  sharedDir: '/tmp/bbs-test/project-memory', rulesDir: '/tmp/bbs-test/rules', agents: [],
};
const d = (id: string, local: boolean): BbsRosterRow => ({ kind: 'device', deviceId: id, name: local ? 'This Mac' : 'Other Mac', local,
  online: local, rosterStatus: 'synced', receivedAt: null });
const p = (deviceId: string, projectId = 'p'): BbsRosterRow => ({ kind: 'project', deviceId, projectId, name: projectId === 'p' ? 'Kota' : 'Notes' });
const a = (deviceId: string, projectId = 'p'): BbsRosterRow => ({ kind: 'agent', deviceId, projectId, agentId: 'same-agent',
  name: 'Alice', targetRef: `${deviceId}/${projectId}/same-agent`, avatar: { kind: 'none' } });
let rows: BbsRosterRow[], version: string, board: client.BbsSnapshot, sync: BbsSyncView;
let rosterHint: () => void, syncHint: () => void;
let stopRoster: ReturnType<typeof vi.fn>;
beforeEach(() => {
  vi.restoreAllMocks(); vi.clearAllMocks(); window.localStorage.clear();
  vi.mocked(isTauri).mockReturnValue(false);
  rows = [d('self', true), p('self'), a('self'), p('self', 'notes'), a('self', 'notes'), d('peer', false), p('peer'), a('peer')];
  version = 'a'.repeat(64); rosterHint = () => {}; syncHint = () => {}; stopRoster = vi.fn();
  vi.mocked(bbsRosterSource.listen).mockImplementation(async changed => { rosterHint = changed; return stopRoster; });
  vi.mocked(bbsRosterSource.read).mockImplementation(cancelled => readCompleteBbsRoster(async () => parseBbsRosterPage({ version, items: rows, next: null }), cancelled));
  sync = { deviceId: 'self', deviceName: 'This Mac', group: { id: 'group-one', name: 'Group', role: 'member', members: [] },
    workerAvailable: false, invitation: { state: 'none' }, phase: 'idle', progress: null, lastSuccessfulAt: null, error: null, controlRecoverable: false };
  vi.mocked(bbsSyncViewSource.read).mockImplementation(async () => sync);
  vi.mocked(bbsSyncViewSource.listen).mockImplementation(async changed => { syncHint = changed; return () => {}; });
  board = { projectId: 'p', projectDisplayName: 'Kota', root: '/tmp/bbs', newCount: 0, threads: [{ threadId: 'thread-one', sharingGroupId: 'group-one',
    visibility: 'broadcast', projectTags: [], projectTagLabels: [], createdByProject: 'p', createdByProjectLabel: 'Kota',
    updatedAt: '2026-09-13T00:00:00Z', latestPostId: 'post-one', isNew: false, relevant: true,
    posts: [{ postId: 'post-one', threadId: 'thread-one', projectId: 'p', projectDisplayName: 'Kota', agentId: 'human', agentDisplayName: 'User',
      kind: 'topic', body: 'Shared topic', preview: 'Shared topic', createdAt: '2026-09-13T00:00:00Z', state: 'none', external: false }],
  }] };
  vi.spyOn(client, 'bbsSnapshot').mockImplementation(async () => board);
  vi.spyOn(client, 'bbsHumanReply').mockResolvedValue('post-reply');
  vi.spyOn(client, 'bbsHumanPost').mockResolvedValue('thread-one');
  vi.spyOn(client, 'agentBusSend').mockRejectedValue(new Error('front end must not deliver'));
  vi.spyOn(client, 'bbsValidateAttachments').mockImplementation(async sources => sources.map(source => ({ ...source, name: source.name || 'file.pdf', sizeBytes: 1 })));
});
async function open(mode: 'reply' | 'topic' = 'reply', currentWorkspace = workspace) {
  render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={currentWorkspace} projectRoot={currentWorkspace.localRoot} />);
  expect(bbsRosterSource.read).not.toHaveBeenCalled(); expect(bbsRosterSource.listen).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: mode === 'reply' ? /^Open$/ : /^Post$/ }));
  const dialog = await screen.findByRole('dialog', { name: 'Bulletin Board' });
  if (mode === 'reply') await userEvent.click(await within(dialog).findByRole('button', { name: /Shared topic/ }));
  expect(bbsRosterSource.read).not.toHaveBeenCalled();
  return { dialog, editor: within(dialog).getByTestId('input-field') };
}
async function picker(dialog: HTMLElement) {
  await userEvent.click(within(dialog).getByRole('button', { name: /^@ Agent/ }));
  await within(dialog).findByRole('checkbox', { name: 'Alice, This Mac, Kota' });
}
async function localAndRemote(dialog: HTMLElement) {
  await picker(dialog);
  await userEvent.click(within(dialog).getByRole('checkbox', { name: 'Alice, This Mac, Kota' }));
  await userEvent.click(within(dialog).getByRole('checkbox', { name: 'Alice, This Mac, Notes' }));
  await userEvent.click(within(dialog).getByRole('navigation', { name: 'Devices' }).querySelectorAll('button')[1]);
  await userEvent.click(within(dialog).getByRole('checkbox', { name: 'Alice, Other Mac, Kota' }));
}
const targets = [
  { deviceId: 'local', projectId: 'p', agentId: 'same-agent' },
  { deviceId: 'local', projectId: 'notes', agentId: 'same-agent' },
  { deviceId: 'peer', projectId: 'p', agentId: 'same-agent' },
];

describe('real BBS entry structured mentions', () => {
  it('takes the actual Rust rejection through the real publisher, retains the draft and only publishes again on a user retry', async () => {
    // Selection context and snapshot shell are UI fixtures; request/body/error/result
    // come from the unmodified 2b Rust fixture documented in bbs-mention-client.test.ts.
    rows = [d('self', true), { kind: 'project', deviceId: 'self', projectId: 'target', name: 'Target room' },
      { kind: 'agent', deviceId: 'self', projectId: 'target', agentId: 'agent', name: 'Receiver',
        targetRef: 'self/target/agent', avatar: { kind: 'none' } }];
    board.projectId = 'source'; board.projectDisplayName = 'Source';
    const { dialog, editor } = await open('topic', { ...workspace, projectId: 'source' });
    await userEvent.type(editor, mentionFixture.request.body);
    await userEvent.click(within(dialog).getByRole('button', { name: /^@ Agent/ }));
    await userEvent.click(await within(dialog).findByRole('checkbox', { name: 'Receiver, This Mac, Target room' }));
    await userEvent.click(within(dialog).getByRole('button', { name: 'Done' }));
    vi.mocked(client.bbsHumanPost).mockImplementation(actualHumanPost);
    vi.mocked(isTauri).mockReturnValue(true);
    vi.mocked(invoke).mockRejectedValueOnce(mentionFixture.errors.notSynced);
    await userEvent.click(within(dialog).getByRole('button', { name: 'Post' }));
    const message = 'The agent list for this device has not synced yet. Try again after syncing.';
    expect(await within(dialog).findByText(message)).toBeInTheDocument();
    expect(within(dialog).getByTestId('input-field')).toBe(editor); expect(editor).toHaveTextContent(mentionFixture.request.body);
    expect(within(dialog).getByRole('button', { name: 'Remove Receiver, This Mac, Target room' })).toBeInTheDocument();
    expect(client.bbsHumanPost).toHaveBeenCalledTimes(1); expect(client.agentBusSend).not.toHaveBeenCalled();
    const publishedRequest = vi.mocked(client.bbsHumanPost).mock.calls[0][0];
    expect(publishedRequest).toMatchObject({ projectId: mentionFixture.request.projectId,
      body: mentionFixture.request.body, mentions: mentionFixture.request.mentions });
    vi.mocked(invoke).mockImplementationOnce(async () => {
      const root = board.threads[0];
      board.threads = [{ ...root, threadId: mentionFixture.threadId, latestPostId: mentionFixture.postId,
        posts: [{ ...root.posts[0], threadId: mentionFixture.threadId, postId: mentionFixture.postId,
          body: mentionFixture.body, preview: mentionFixture.body }] }];
      return mentionFixture.threadId;
    });
    await userEvent.click(within(dialog).getByRole('button', { name: 'Post' }));
    await within(dialog).findByText('Original human input');
    expect(within(dialog).getAllByText('@Receiver')).toHaveLength(1);
    expect(within(dialog).queryByText(message)).not.toBeInTheDocument();
    expect(within(dialog).queryByLabelText('Selected recipients')).not.toBeInTheDocument();
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === 'bbs_human_post')).toEqual([
      ['bbs_human_post', { request: publishedRequest }], ['bbs_human_post', { request: publishedRequest }],
    ]);
    expect(client.bbsHumanPost).toHaveBeenCalledTimes(2); expect(client.bbsHumanReply).not.toHaveBeenCalled();
    expect(client.agentBusSend).not.toHaveBeenCalled();
  });
  it('publishes local/cross-project/remote homonyms together with raw body, one backend command and no frontend bus', async () => {
    const { dialog, editor } = await open(); await userEvent.type(editor, 'Read the attachment');
    fireEvent.drop(editor, { dataTransfer: { files: [], getData: (kind: string) => kind === 'text/uri-list' ? 'file:///tmp/file.pdf' : '' } });
    await within(dialog).findByTestId('ib-attachment-chip');
    const snapshots = vi.mocked(client.bbsSnapshot).mock.calls.length;
    await localAndRemote(dialog); expect(within(dialog).getByTestId('input-field')).toBe(editor);
    expect(within(dialog).getByText('3 agents · 3 projects · 2 devices')).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Done' }));
    expect(stopRoster).toHaveBeenCalledOnce(); expect(client.bbsSnapshot).toHaveBeenCalledTimes(snapshots);
    await userEvent.click(within(dialog).getByRole('button', { name: 'Reply' }));
    await waitFor(() => expect(client.bbsHumanReply).toHaveBeenCalledExactlyOnceWith({ projectId: 'p', projectDisplayName: 'Kota',
      threadId: 'thread-one', body: 'Read the attachment', attachments: [{ path: '/tmp/file.pdf', name: 'file.pdf' }], mentions: targets }));
    expect(editor.textContent).toBe(''); expect(within(dialog).queryByLabelText('Selected recipients')).not.toBeInTheDocument();
    expect(client.agentBusSend).not.toHaveBeenCalled();
  });
  it('updates display names without moving IDs, keeps stale selections, and never remounts the editor on roster/progress events', async () => {
    const { dialog, editor } = await open(); await userEvent.type(editor, 'Still typing'); await localAndRemote(dialog);
    const snapshots = vi.mocked(client.bbsSnapshot).mock.calls.length;
    rows = rows.filter(row => row.deviceId !== 'peer').map(row => row.kind === 'agent' ? { ...row, name: 'Renamed Alice' } : row);
    version = 'b'.repeat(64); act(() => rosterHint());
    await within(dialog).findByRole('checkbox', { name: 'Renamed Alice, This Mac, Kota' });
    expect(within(dialog).queryByLabelText('Selected recipients')).not.toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Done' }));
    await within(dialog).findByRole('button', { name: 'Remove Renamed Alice, This Mac, Kota' });
    expect(within(dialog).getByRole('button', { name: 'Remove Alice, Other Mac, Kota' })).toBeInTheDocument();
    sync = { ...sync, phase: 'syncing', progress: { completed: 1, total: 9 } }; act(() => syncHint());
    await within(dialog).findByRole('button', { name: 'Syncing 1/9' });
    expect(within(dialog).getByTestId('input-field')).toBe(editor); expect(editor).toHaveTextContent('Still typing');
    expect(client.bbsSnapshot).toHaveBeenCalledTimes(snapshots);
    await userEvent.click(within(dialog).getByRole('button', { name: 'Reply' }));
    expect(client.bbsHumanReply).toHaveBeenCalledWith(expect.objectContaining({ mentions: targets, body: 'Still typing' }));
  });
  it('pre-disables only remote selection in a private thread, keeping other local projects available', async () => {
    board.threads[0].sharingGroupId = null;
    const { dialog, editor } = await open(); await userEvent.type(editor, 'Local only'); await picker(dialog);
    const local = within(dialog).getByRole('checkbox', { name: 'Alice, This Mac, Notes' }); expect(local).toBeEnabled(); await userEvent.click(local);
    await userEvent.click(within(dialog).getByRole('navigation', { name: 'Devices' }).querySelectorAll('button')[1]);
    expect(within(dialog).getByRole('checkbox', { name: 'Alice, Other Mac, Kota' })).toBeDisabled();
    expect(within(dialog).getByText('This thread is not shared. Start a new thread to @ this agent.')).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Reply' }));
    expect(client.bbsHumanReply).toHaveBeenCalledWith(expect.objectContaining({ mentions: [targets[1]] }));
  });
  it('keeps every selected target, text and file when the backend rejects a stale mixed target set', async () => {
    vi.mocked(client.bbsHumanReply).mockRejectedValueOnce(new BbsMentionClientError('mention_thread_not_shared'));
    const { dialog, editor } = await open(); await userEvent.type(editor, 'Do not silently send only local'); await localAndRemote(dialog);
    fireEvent.drop(editor, { dataTransfer: { files: [], getData: (kind: string) => kind === 'text/uri-list' ? 'file:///tmp/file.pdf' : '' } });
    await within(dialog).findByTestId('ib-attachment-chip');
    await userEvent.click(within(dialog).getByRole('button', { name: 'Reply' }));
    const message = 'This agent is on another device, but this thread is not shared. No reply was posted. Start a new thread to @ this agent.';
    expect(await within(dialog).findByText(message)).toBeInTheDocument();
    expect(editor).toHaveTextContent('Do not silently send only local'); expect(within(dialog).getByTestId('ib-attachment-chip')).toBeInTheDocument();
    expect(within(dialog).getByLabelText('Selected recipients').querySelectorAll('button')).toHaveLength(3);
    expect(client.bbsHumanReply).toHaveBeenCalledTimes(1); expect(client.agentBusSend).not.toHaveBeenCalled();
    expect(within(dialog).queryByText(/remove the failed attachment/)).not.toBeInTheDocument();
  });
  it('freezes a new post target snapshot during publish and does not deliver after closing the board', async () => {
    let resolve!: (value: string) => void; vi.mocked(client.bbsHumanPost).mockReturnValueOnce(new Promise(res => { resolve = res; }));
    const { dialog, editor } = await open('topic'); await userEvent.type(editor, 'New thread'); await localAndRemote(dialog);
    await userEvent.click(within(dialog).getByRole('button', { name: 'Post' }));
    expect(within(dialog).getByRole('button', { name: 'Posting…' })).toBeDisabled();
    expect(within(dialog).getByRole('button', { name: /^@ Agent/ })).toBeDisabled();
    expect([...dialog.querySelectorAll('.bbs-mentions-summary button')].every(button => (button as HTMLButtonElement).disabled)).toBe(true);
    expect(client.bbsHumanPost).toHaveBeenCalledWith(expect.objectContaining({ body: 'New thread', mentions: targets }));
    await userEvent.click(within(dialog).getByRole('button', { name: 'Close Bulletin Board' }));
    await act(async () => resolve('thread-one'));
    expect(client.bbsHumanPost).toHaveBeenCalledTimes(1); expect(client.agentBusSend).not.toHaveBeenCalled();
  });
  it.each(['reply', 'topic'] as const)('hides the selected summary only while the %s picker is open, preserving every target and the editor', async mode => {
    const { dialog, editor } = await open(mode); await userEvent.type(editor, 'Keep this'); await localAndRemote(dialog);
    expect(within(dialog).queryByLabelText('Selected recipients')).not.toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: '@ Agent · 3' })).toBeInTheDocument();
    const snapshots = vi.mocked(client.bbsSnapshot).mock.calls.length;
    await userEvent.keyboard('{Escape}'); expect(within(dialog).queryByLabelText('Mention agents')).not.toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: 'Bulletin Board' })).toBe(dialog);
    expect(within(within(dialog).getByLabelText('Selected recipients')).getAllByRole('button')).toHaveLength(3);
    await userEvent.click(within(dialog).getByRole('button', { name: /^@ Agent/ }));
    expect(within(dialog).queryByLabelText('Selected recipients')).not.toBeInTheDocument();
    expect(await within(dialog).findByRole('checkbox', { name: 'Alice, Other Mac, Kota' })).toBeChecked();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Done' }));
    const summary = within(within(dialog).getByLabelText('Selected recipients'));
    expect(summary.getAllByRole('button').map(button => button.getAttribute('aria-label'))).toEqual([
      'Remove Alice, This Mac, Kota', 'Remove Alice, This Mac, Notes', 'Remove Alice, Other Mac, Kota',
    ]);
    expect(within(dialog).getByTestId('input-field')).toBe(editor); expect(editor).toHaveTextContent('Keep this');
    expect(client.bbsSnapshot).toHaveBeenCalledTimes(snapshots); expect(bbsRosterSource.read).toHaveBeenCalledTimes(2);
    expect(client.bbsHumanPost).not.toHaveBeenCalled(); expect(client.bbsHumanReply).not.toHaveBeenCalled();
    expect(client.agentBusSend).not.toHaveBeenCalled();
  });
});
