import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as client from '../src/pty-client';
import { bbsRemoveDeletedItem } from '../src/bbs-post-versions';
import { RightColumn } from '../src/chrome/RightColumn';

const a = 'a'.repeat(64), b = 'b'.repeat(64), c = 'c'.repeat(64);
const workspace: client.WorkspaceProject = {
  projectId: 'bbs-test', repoFullName: 'mock/bbs-test', remoteUrl: '', githubHtmlUrl: '', defaultBranch: 'main', baseRef: 'main',
  localRoot: '/tmp/bbs-test', localRootBytes: 0, sourceDir: '/tmp/bbs-test/source', sourceDirBytes: 0,
  sharedDir: '/tmp/bbs-test/project-memory', rulesDir: '/tmp/bbs-test/rules', agents: [],
};
function post(postId: string, versionId: string | undefined, body: string, kind = 'reply'): client.BbsPost {
  return { postId, threadId: 'thread-one', versionId, body, preview: body, kind,
    projectId: workspace.projectId, projectDisplayName: 'Test', agentId: 'human', agentDisplayName: 'User',
    createdAt: '2026-09-12T00:00:00Z', state: 'none', external: false };
}
let board: client.BbsSnapshot;
let tick: () => void;
beforeEach(() => {
  vi.restoreAllMocks(); window.localStorage.clear();
  board = { projectId: workspace.projectId, projectDisplayName: 'Test', root: '/tmp/bbs', newCount: 0,
    threads: [{ threadId: 'thread-one', visibility: 'broadcast', projectTags: [], projectTagLabels: [],
      createdByProject: workspace.projectId, createdByProjectLabel: 'Test', updatedAt: '2026-09-12T00:00:00Z',
      latestPostId: 'reply-two', isNew: false, relevant: true,
      posts: [post('topic', b, 'Root B', 'topic'), post('topic', a, 'Root A', 'topic'),
        post('reply-one', b, 'Reply B'), post('reply-one', a, 'Reply A'), post('reply-two', c, 'Different reply')],
    }],
  };
  vi.spyOn(client, 'bbsSnapshot').mockImplementation(async () => board);
  vi.spyOn(client, 'bbsMarkProcessed').mockResolvedValue();
  vi.spyOn(client, 'bbsDelete').mockImplementation(async (request) => { board = bbsRemoveDeletedItem(board, request)!; });
  const setInterval = window.setInterval.bind(window);
  const polls: (() => void)[] = [];
  tick = () => polls.forEach((poll) => poll());
  vi.spyOn(window, 'setInterval').mockImplementation(((handler: TimerHandler, delay?: number, ...args: unknown[]) => {
    if (delay === 12_000 && typeof handler === 'function') { polls.push(() => handler(...args)); return 0; }
    return setInterval(handler, delay, ...args);
  }) as typeof window.setInterval);
});
async function openBoard() {
  render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />);
  await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
  return screen.findByRole('dialog', { name: 'Bulletin Board' });
}
async function openThread(body = 'Root B') {
  const dialog = await openBoard();
  await userEvent.click(await within(dialog).findByRole('button', { name: new RegExp(body) }));
  return dialog;
}
function floor(dialog: HTMLElement, body: string) {
  return within(dialog).getByText(body).closest<HTMLElement>('.bbs-msg')!;
}
async function confirm() {
  const dialog = screen.getByRole('dialog', { name: 'Delete BBS item' });
  await userEvent.click(within(dialog).getByRole('button', { name: 'Delete' }));
  await waitFor(() => expect(client.bbsDelete).toHaveBeenCalled());
}

describe('BBS Fork product presentation and deletion', () => {
  it('uses each version origin avatar in both rows and floors, not the matching local author', async () => {
    board.threads[0].posts = board.threads[0].posts.map((item) => ({ ...item, agentId: 'hero-cc', agentAvatar: 'claude',
      syncAvatar: item.versionId === a ? { kind: 'builtin' as const, id: 'violet' } : { kind: 'none' as const } }));
    const dialog = await openBoard();
    const rootA = await within(dialog).findByRole('button', { name: /Root A/ });
    const rootB = within(dialog).getByRole('button', { name: /Root B/ });
    expect(rootA.querySelector('.bbs-msg-avatar')).toHaveClass('system-violet');
    expect(rootB.querySelector('.bbs-msg-avatar')).toHaveClass('provider-codex');
    await userEvent.click(rootA);
    expect(floor(dialog, 'Root A').querySelector('.bbs-msg-avatar')).toHaveClass('system-violet');
    expect(floor(dialog, 'Reply A').querySelector('.bbs-msg-avatar')).toHaveClass('system-violet');
    expect(floor(dialog, 'Reply B').querySelector('.bbs-msg-avatar')).toHaveClass('provider-codex');
  });

  it('shows root variants as rows, shares reply floors/editor, and posts replies to the thread only', async () => {
    const publish = vi.spyOn(client, 'bbsHumanReply').mockResolvedValue('new-reply');
    const dialog = await openBoard();
    const rootA = await within(dialog).findByRole('button', { name: /Root A/ });
    const rootB = within(dialog).getByRole('button', { name: /Root B/ });
    expect(rootA).toHaveTextContent('Forked Version 1');
    expect(rootB).toHaveTextContent('Forked Version 2');
    expect(rootA).toHaveTextContent('2 Replies');
    expect(rootB).toHaveTextContent('2 Replies');
    await userEvent.click(rootB);
    expect(floor(dialog, 'Reply A')).toHaveTextContent('#2');
    expect(floor(dialog, 'Reply B')).toHaveTextContent('#2');
    expect(floor(dialog, 'Different reply')).toHaveTextContent('#3');
    const editor = within(dialog).getByTestId('input-field');
    await userEvent.type(editor, 'A shared reply');
    const switcher = within(dialog).getByRole('group', { name: 'Topic versions' });
    await userEvent.click(within(switcher).getByRole('button', { name: 'Forked Version 1' }));
    expect(floor(dialog, 'Root A')).toHaveTextContent('#1');
    expect(within(dialog).queryByText('Root B')).not.toBeInTheDocument();
    expect(within(dialog).getByTestId('input-field')).toBe(editor);
    expect(editor).toHaveTextContent('A shared reply');
    expect(floor(dialog, 'Reply A')).toHaveTextContent('#2');
    await userEvent.click(within(dialog).getByRole('button', { name: 'Reply' }));
    await waitFor(() => expect(publish).toHaveBeenCalledWith({ projectId: workspace.projectId,
      projectDisplayName: 'Test', threadId: 'thread-one', body: 'A shared reply', attachments: [] }));
  });

  it('deletes just a selected root version and keeps the survivor, replies and current draft', async () => {
    const dialog = await openThread();
    const editor = within(dialog).getByTestId('input-field');
    await userEvent.type(editor, 'Keep writing');
    await userEvent.click(within(floor(dialog, 'Root B')).getByRole('button', { name: 'Delete version' }));
    expect(screen.getByRole('dialog', { name: 'Delete BBS item' })).toHaveTextContent('Other versions and replies are kept.');
    await confirm();
    expect(client.bbsDelete).toHaveBeenCalledWith({ threadId: 'thread-one', postId: 'topic', versionId: b });
    expect(await within(dialog).findByText('Root A')).toBeInTheDocument();
    expect(within(dialog).queryByText('Root B')).not.toBeInTheDocument();
    expect(floor(dialog, 'Reply B')).toBeInTheDocument();
    expect(within(dialog).getByTestId('input-field')).toBe(editor);
    expect(editor).toHaveTextContent('Keep writing');
  });

  it('keeps the selected hash and frozen delete request when incoming snapshots renumber or remove siblings', async () => {
    const dialog = await openThread();
    await userEvent.click(within(floor(dialog, 'Root B')).getByRole('button', { name: 'Delete version' }));
    // Only the chosen hash remains now. Confirmation must NOT turn into logical Delete.
    board = { ...board, threads: [{ ...board.threads[0], posts: board.threads[0].posts.filter((item) => item.postId !== 'topic' || item.versionId === b) }] };
    await act(async () => tick());
    await waitFor(() => expect(within(dialog).queryByRole('group', { name: 'Topic versions' })).not.toBeInTheDocument());
    expect(screen.getByRole('dialog', { name: 'Delete BBS item' })).toHaveTextContent('Delete Version');
    await confirm();
    expect(client.bbsDelete).toHaveBeenCalledExactlyOnceWith({ threadId: 'thread-one', postId: 'topic', versionId: b });
  });

  it('preserves selected root identity when its display ordinal changes', async () => {
    const dialog = await openThread();
    board = { ...board, threads: [{ ...board.threads[0], posts: [post('topic', '0'.repeat(64), 'New earlier hash', 'topic'), ...board.threads[0].posts] }] };
    await act(async () => tick());
    await waitFor(() => expect(floor(dialog, 'Root B')).toHaveTextContent('Forked Version 3'));
    expect(within(within(dialog).getByRole('group', { name: 'Topic versions' })).getByRole('button', { name: 'Forked Version 3' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('offers distinct per-version and all-versions reply deletion', async () => {
    const dialog = await openThread();
    await userEvent.click(within(floor(dialog, 'Reply B')).getByRole('button', { name: 'Delete version' }));
    await confirm();
    expect(client.bbsDelete).toHaveBeenLastCalledWith({ threadId: 'thread-one', postId: 'reply-one', versionId: b });
    expect(await within(dialog).findByText('Reply A')).toBeInTheDocument();
    expect(within(dialog).queryByText('Reply B')).not.toBeInTheDocument();
    // Restore both versions as a new authoritative snapshot to exercise logical scope.
    board = { ...board, threads: [{ ...board.threads[0], posts: [...board.threads[0].posts, post('reply-one', b, 'Reply B')] }] };
    await act(async () => tick());
    await userEvent.click(await within(floor(dialog, 'Reply A')).findByRole('button', { name: 'Delete reply' }));
    expect(screen.getByRole('dialog', { name: 'Delete BBS item' })).toHaveTextContent('Delete this reply and all its versions?');
    await confirm();
    expect(client.bbsDelete).toHaveBeenLastCalledWith({ threadId: 'thread-one', postId: 'reply-one' });
    expect(within(dialog).queryByText('Reply A')).not.toBeInTheDocument();
    expect(within(dialog).queryByText('Reply B')).not.toBeInTheDocument();
    expect(floor(dialog, 'Different reply')).toBeInTheDocument();
  });

  it('keeps whole-thread deletion explicit and retains the ordinary confirmation for legacy posts', async () => {
    const dialog = await openThread();
    await userEvent.click(within(floor(dialog, 'Root B')).getByRole('button', { name: 'Delete thread' }));
    expect(screen.getByRole('dialog', { name: 'Delete BBS item' })).toHaveTextContent('Delete this thread and all its versions?');
    await confirm();
    expect(client.bbsDelete).toHaveBeenCalledWith({ threadId: 'thread-one', postId: 'topic' });
    expect(await within(dialog).findByText('No BBS threads yet.')).toBeInTheDocument();
    board = { ...board, threads: [{ threadId: 'legacy', visibility: 'broadcast', projectTags: [], projectTagLabels: [],
      createdByProject: workspace.projectId, createdByProjectLabel: 'Test', updatedAt: '2026-09-12T00:00:00Z',
      latestPostId: 'old-topic', isNew: false, relevant: true, posts: [{ ...post('old-topic', undefined, 'Legacy root', 'topic'), threadId: 'legacy' }] }] };
    await act(async () => tick());
    await userEvent.click(await within(dialog).findByRole('button', { name: /Legacy root/ }));
    expect(within(dialog).queryByText(/Forked Version/)).not.toBeInTheDocument();
    await userEvent.click(within(floor(dialog, 'Legacy root')).getByRole('button', { name: 'Delete' }));
    expect(screen.getByRole('dialog', { name: 'Delete BBS item' })).toHaveTextContent('Delete this thread?');
    expect(screen.getByRole('dialog', { name: 'Delete BBS item' })).toHaveTextContent('All replies under it are deleted too.');
  });

  it('disables malformed version deletion and marks a logical post seen only once', async () => {
    board.threads[0].posts = board.threads[0].posts.map((item) => ({ ...item, state: 'new',
      versionId: item.postId === 'topic' && item.versionId === b ? '../../bad' : item.versionId }));
    const dialog = await openThread();
    expect(within(floor(dialog, 'Root B')).getByRole('button', { name: 'Delete version' })).toBeDisabled();
    await waitFor(() => expect(client.bbsMarkProcessed).toHaveBeenCalledTimes(3));
    expect(client.bbsDelete).not.toHaveBeenCalled();
  });

  it('does not remove any version or retry when backend deletion fails', async () => {
    vi.mocked(client.bbsDelete).mockRejectedValue(new Error('Deletion failed'));
    const dialog = await openThread();
    await userEvent.click(within(floor(dialog, 'Root B')).getByRole('button', { name: 'Delete version' }));
    await confirm();
    expect(await within(dialog).findByText(/Deletion failed/)).toBeInTheDocument();
    expect(floor(dialog, 'Root B')).toBeInTheDocument();
    expect(board.threads[0].posts).toHaveLength(5);
    expect(client.bbsDelete).toHaveBeenCalledTimes(1);
  });
});
