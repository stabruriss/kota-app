import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as client from '../src/pty-client';
import { BBS_UNAVAILABLE_MESSAGE, bbsRemoveDeletedItem } from '../src/bbs-post-versions';
import { RightColumn } from '../src/chrome/RightColumn';
import nativeProjection from './fixtures/bbs-unavailable-snapshot-show.json';

const workspace: client.WorkspaceProject = {
  projectId: 'bbs-test', repoFullName: 'mock/bbs-test', remoteUrl: '', githubHtmlUrl: '', defaultBranch: 'main', baseRef: 'main',
  localRoot: '/tmp/bbs-test', localRootBytes: 0, sourceDir: '/tmp/bbs-test/source', sourceDirBytes: 0,
  sharedDir: '/tmp/bbs-test/project-memory', rulesDir: '/tmp/bbs-test/rules', agents: [],
};
function post(postId: string, body: string, kind = 'reply'): client.BbsPost {
  return { postId, threadId: 'thread-one', versionId: 'a'.repeat(64), body, preview: body, kind,
    projectId: workspace.projectId, projectDisplayName: 'Test', agentId: 'human', agentDisplayName: 'User',
    createdAt: '2026-09-12T00:00:00Z', state: 'none', external: false };
}
let board: client.BbsSnapshot;
let tick: () => void;
beforeEach(() => {
  vi.restoreAllMocks(); window.localStorage.clear();
  // Typed projection fixture only; no native sync, parsing or file IO is mocked as an end-to-end pass.
  board = { projectId: workspace.projectId, projectDisplayName: 'Test', root: '/tmp/bbs', newCount: 0,
    threads: [{ threadId: 'thread-one', visibility: 'broadcast', projectTags: [], projectTagLabels: [],
      createdByProject: workspace.projectId, createdByProjectLabel: 'Test', updatedAt: '2026-09-12T00:00:00Z',
      latestPostId: 'oversize', isNew: false, relevant: true, posts: [],
      unavailablePosts: [{ postId: 'oversize', reason: 'too_large_to_sync', kind: 'topic' }],
    }],
  };
  vi.spyOn(client, 'bbsSnapshot').mockImplementation(async () => structuredClone(board));
  vi.spyOn(client, 'bbsMarkProcessed').mockResolvedValue();
  vi.spyOn(client, 'bbsHumanReply').mockResolvedValue('unused');
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
async function openThread(name = /Post unavailable/) {
  const dialog = await openBoard();
  await userEvent.click(await within(dialog).findByRole('button', { name }));
  return dialog;
}
function notice(dialog: HTMLElement) {
  return within(dialog).getByRole('note', { name: 'Unavailable BBS post' });
}

describe('BBS oversize placeholders at the real board entry', () => {
  it('renders actual Rust snapshot/show projection with an empty latestPostId and then an ordinary reply', async () => {
    // Unmodified KOTA_BBS_UNAVAILABLE_FIXTURE output from
    // aa17ad97cb57c08b48f6e9a2299f99617aa105a2, bbs/sync/tests/unavailable_tests.rs:
    // unknown_root_is_visible_without_inventing_a_post_and_show_reuses_projection.
    // Fixture SHA-256: 4007daf4da1417d13c2bbdfd88a08d2e142e62d99c9cc133d496765a963ce11f.
    // Only snapshot delivery is stubbed. The fixture's temporary paths are never read,
    // and this does not run the Rust CLI, native IPC or two-device synchronization.
    board = structuredClone(nativeProjection.emptyRoot) as client.BbsSnapshot;
    const actualNotice = nativeProjection.emptyRoot.threads[0].unavailablePosts[0];
    expect(Object.keys(actualNotice).sort()).toEqual(['postId', 'reason']);
    expect(board.threads[0].latestPostId).toBe('');
    expect(board.threads[0].relevant).toBe(false);
    const actualWorkspace = { ...workspace, projectId: board.projectId };
    render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={actualWorkspace} projectRoot={actualWorkspace.localRoot} />);
    await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
    const dialog = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    const row = await within(dialog).findByRole('button', { name: /Post unavailable/ });
    expect(row).toHaveTextContent(actualNotice.postId);
    expect(row).toHaveTextContent(BBS_UNAVAILABLE_MESSAGE);
    expect(row.querySelector('.bbs-msg-avatar')).toBeNull();
    await userEvent.click(row);
    expect(notice(dialog)).toHaveTextContent(actualNotice.postId);
    expect(dialog.querySelector('.bbs-msg, .bbs-op-badge')).toBeNull();

    board = structuredClone(nativeProjection.withReply) as client.BbsSnapshot;
    await act(async () => tick());
    const actualReply = nativeProjection.withReply.threads[0].posts[0];
    const reply = (await within(dialog).findByText(actualReply.body)).closest<HTMLElement>('.bbs-msg')!;
    expect(reply).toHaveAttribute('data-bbs-post-id', actualReply.postId);
    expect(reply).toHaveAttribute('data-bbs-version-id', actualReply.versionId);
    expect(reply).toHaveTextContent('#2');
    expect(reply.querySelector('.bbs-op-badge')).toBeNull();
    expect(notice(dialog)).toHaveTextContent(BBS_UNAVAILABLE_MESSAGE);
    expect(notice(dialog)).not.toHaveAttribute('data-bbs-version-id');
    expect(notice(dialog).querySelector('button, a, img, .bbs-msg-avatar, .bbs-time')).toBeNull();
    expect(within(dialog).queryByText(/Forked Version/)).not.toBeInTheDocument();
    expect(within(dialog).queryByTestId('input-field')).not.toBeInTheDocument();
    expect(nativeProjection.show.split('## Unavailable post\n')[1]).toBe(
      `post: ${actualNotice.postId}\n\n${BBS_UNAVAILABLE_MESSAGE}\n\n`,
    );
    expect(nativeProjection.show).toContain(`post: ${actualReply.postId}\nversion: ${actualReply.versionId}\n\n${actualReply.body}`);
    expect(client.bbsMarkProcessed).not.toHaveBeenCalled();
    expect(client.bbsHumanReply).not.toHaveBeenCalled();
    expect(client.bbsDelete).not.toHaveBeenCalled();
  });

  it.each(['topic', undefined] as const)('opens a neutral thread for a %s notice without inventing a post or actions', async (kind) => {
    board.threads[0].unavailablePosts = [{ postId: 'not-a-minted-author-id', reason: 'too_large_to_sync', ...(kind ? { kind } : {}) }];
    const dialog = await openThread();
    const item = notice(dialog);
    expect(item).toHaveTextContent(BBS_UNAVAILABLE_MESSAGE);
    expect(item).toHaveTextContent('not-a-minted-author-id');
    expect(item).not.toHaveAttribute('data-bbs-version-id');
    expect(item.querySelector('button, a, img, .bbs-msg-avatar, .bbs-time')).toBeNull();
    expect(dialog.querySelector('.bbs-msg, .bbs-op-badge')).toBeNull();
    expect(within(dialog).queryByText(/Forked Version/)).not.toBeInTheDocument();
    expect(within(dialog).queryByTestId('input-field')).not.toBeInTheDocument();
    expect(within(dialog).queryByRole('button', { name: 'Reply' })).not.toBeInTheDocument();
    expect(client.bbsMarkProcessed).not.toHaveBeenCalled();
    expect(client.bbsHumanReply).not.toHaveBeenCalled();
    expect(client.bbsDelete).not.toHaveBeenCalled();
  });

  it('keeps real replies readable as replies and retains the root notice when the last reply is deleted', async () => {
    board.threads[0].posts = [post('reply-one', 'An available reply')];
    const dialog = await openThread();
    const reply = within(dialog).getByText('An available reply').closest<HTMLElement>('.bbs-msg')!;
    expect(reply).toHaveTextContent('#2');
    expect(reply.querySelector('.bbs-op-badge')).toBeNull();
    expect(dialog.querySelectorAll('.bbs-msg')).toHaveLength(1);
    expect(dialog.querySelector('.bbs-replies-divider')).toHaveTextContent('1 available reply');
    expect(notice(dialog)).toHaveTextContent('oversize');
    await userEvent.click(within(reply).getByRole('button', { name: 'Delete' }));
    const confirmation = screen.getByRole('dialog', { name: 'Delete BBS item' });
    expect(confirmation).toHaveTextContent('Delete this reply?');
    await userEvent.click(within(confirmation).getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(client.bbsDelete).toHaveBeenCalledExactlyOnceWith({ threadId: 'thread-one', postId: 'reply-one' }));
    expect(notice(dialog)).toHaveTextContent('oversize');
    expect(within(dialog).queryByText('An available reply')).not.toBeInTheDocument();
    expect(within(dialog).queryByText('This thread is no longer available.')).not.toBeInTheDocument();
  });

  it('shows a missing reply beside normal content without creating a floor, Fork, or altering the editor', async () => {
    board.threads[0].posts = [post('topic', 'Normal root', 'topic'), post('normal', 'Normal reply')];
    board.threads[0].unavailablePosts = [{ postId: 'missing-reply', reason: 'too_large_to_sync', kind: 'reply' }];
    const dialog = await openThread(/Normal root/);
    expect(notice(dialog)).toHaveTextContent('missing-reply');
    expect(dialog.querySelectorAll('.bbs-msg')).toHaveLength(2);
    expect(dialog.querySelectorAll('.bbs-op-badge')).toHaveLength(1);
    expect(within(dialog).queryByText(/Forked Version/)).not.toBeInTheDocument();
    const editor = within(dialog).getByTestId('input-field');
    await userEvent.type(editor, 'Keep this input');
    board.threads[0].posts.push(post('missing-reply', 'The body has arrived'));
    // Keep the stale marker to prove UI real-body priority even before durable cleanup.
    await act(async () => tick());
    await within(dialog).findByText('The body has arrived');
    expect(within(dialog).queryByRole('note')).not.toBeInTheDocument();
    expect(within(dialog).getByTestId('input-field')).toBe(editor);
    expect(editor).toHaveTextContent('Keep this input');
    expect(dialog.querySelectorAll('.bbs-msg')).toHaveLength(3);
    expect(within(dialog).queryByText(/Forked Version/)).not.toBeInTheDocument();
  });

  it('keeps all unknown notices neutral and adopts a real root without a fake Fork when it arrives', async () => {
    board.threads[0].unavailablePosts = [
      { postId: 'oversize', reason: 'too_large_to_sync' },
      { postId: 'also-missing', reason: 'too_large_to_sync' },
    ];
    const dialog = await openThread();
    expect(within(dialog).getAllByRole('note')).toHaveLength(2);
    expect(dialog.querySelectorAll('.bbs-msg')).toHaveLength(0);
    board.threads[0].posts = [post('oversize', 'Recovered root', 'topic')];
    await act(async () => tick());
    await within(dialog).findByText('Recovered root');
    expect(notice(dialog)).toHaveTextContent('also-missing');
    expect(dialog.querySelectorAll('.bbs-op-badge')).toHaveLength(1);
    expect(within(dialog).getByTestId('input-field')).toBeInTheDocument();
    expect(within(dialog).queryByText(/Forked Version/)).not.toBeInTheDocument();
  });

  it('only offers the explicit whole-thread delete for a root-only notice', async () => {
    const dialog = await openThread();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Delete thread' }));
    const confirmation = screen.getByRole('dialog', { name: 'Delete BBS item' });
    expect(confirmation).toHaveTextContent('Delete this thread and all its versions?');
    expect(confirmation).toHaveTextContent('All replies under it are deleted too.');
    await userEvent.click(within(confirmation).getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(client.bbsDelete).toHaveBeenCalledExactlyOnceWith({ threadId: 'thread-one' }));
    expect(await within(dialog).findByText('No BBS threads yet.')).toBeInTheDocument();
  });

  it('leaves legacy snapshots with no unavailable field unchanged', async () => {
    board.threads[0].posts = [post('topic', 'Legacy normal root', 'topic')];
    delete board.threads[0].unavailablePosts;
    const dialog = await openThread(/Legacy normal root/);
    expect(within(dialog).queryByText(BBS_UNAVAILABLE_MESSAGE)).not.toBeInTheDocument();
    expect(within(dialog).getByTestId('input-field')).toBeInTheDocument();
    expect(dialog.querySelectorAll('.bbs-op-badge')).toHaveLength(1);
  });
});
