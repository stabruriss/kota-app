import { createRef } from 'react';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('../src/bbs-roster-client', async original => ({ ...await original<typeof import('../src/bbs-roster-client')>(),
  bbsRosterSource: { read: vi.fn(), listen: vi.fn(async () => () => {}) } }));
import { bbsRosterSource } from '../src/bbs-roster-client';
import * as client from '../src/pty-client';
import { BbsAttachments } from '../src/chrome/BbsAttachments';
import { BbsEditor, BBS_ATTACHMENT_MAX_BYTES, type BbsEditorHandle, type BbsEditorState } from '../src/chrome/BbsEditor';
import { InputBar, escapePromptPath, type InputBarHandle } from '../src/chrome/InputBar';
import { RightColumn } from '../src/chrome/RightColumn';
import { AGENTS } from '../src/mock/fixtures';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

function dropFile(field: HTMLElement, path: string) {
  fireEvent.drop(field, { dataTransfer: { files: [], getData: (type: string) => type === 'text/uri-list' ? `file://${path}` : '' } });
}

const attachment = (id: string, ext = 'png'): client.BbsAttachment => ({
  id, name: `original.${ext}`, path: `attachments/post-one/${id}.${ext}`,
  localPath: `/tmp/bbs/threads/thread-one/attachments/post-one/${id}.${ext}`,
  sizeBytes: 123, sha256: 'a'.repeat(64), available: true,
});

beforeEach(() => {
  vi.restoreAllMocks();
  window.localStorage.clear();
  vi.spyOn(client, 'bbsValidateAttachments').mockImplementation(async (sources) => sources.map((source) => ({
    ...source, name: source.name || source.path.split('/').pop()!, sizeBytes: 123,
  })));
});

describe('BBS-only serialization and input', () => {
  it('keeps room serialization unchanged and separates only BBS pills from handwritten paths', async () => {
    const ref = createRef<InputBarHandle>();
    render(<InputBar ref={ref} value="" onChange={() => {}} />);
    const path = '/tmp/report name.pdf';
    await userEvent.type(screen.getByTestId('input-field'), `Keep ${path}`);
    act(() => ref.current!.insertAttachment({ path, name: 'Readable report.pdf', kind: 'file' }));
    expect(ref.current!.serialize().payload).toContain(escapePromptPath(path));
    expect(ref.current!.serializeBbs()).toEqual({
      body: `Keep ${path}`, mentions: [], attachments: [{ path, name: 'Readable report.pdf', kind: 'file' }],
    });
  });

  it('reports preparation, accepts an attachment-only draft, and copies nothing on drop', async () => {
    const task = deferred<client.BbsAttachmentValidation[]>();
    vi.mocked(client.bbsValidateAttachments).mockReturnValueOnce(task.promise);
    const ref = createRef<BbsEditorHandle>();
    const states: BbsEditorState[] = [];
    render(<BbsEditor ref={ref} value="" onChange={() => {}} onStateChange={(state) => states.push(state)} onError={vi.fn()} />);
    dropFile(screen.getByTestId('input-field'), '/tmp/report.pdf');
    expect(ref.current!.snapshot().preparing).toBe(true);
    expect(screen.queryByTestId('ib-attachment-chip')).not.toBeInTheDocument();
    await act(async () => task.resolve([{ path: '/tmp/report.pdf', name: 'report.pdf', sizeBytes: 5 }]));
    expect(await screen.findByTestId('ib-attachment-chip')).toHaveTextContent('report.pdf');
    expect(ref.current!.snapshot()).toEqual({ body: '', attachments: [{ path: '/tmp/report.pdf', name: 'report.pdf' }], preparing: false });
    expect(states.at(-1)).toEqual({ hasContent: true, preparing: false });
  });

  it('does not silently insert a path when preflight fails', async () => {
    vi.mocked(client.bbsValidateAttachments).mockRejectedValueOnce(new Error('directory is not a regular file'));
    const onError = vi.fn();
    render(<BbsEditor value="Keep my text" onChange={() => {}} onStateChange={() => {}} onError={onError} />);
    dropFile(screen.getByTestId('input-field'), '/tmp/folder');
    await waitFor(() => expect(onError).toHaveBeenCalledWith(expect.stringContaining('regular file')));
    expect(screen.getByTestId('input-field')).toHaveTextContent('Keep my text');
    expect(screen.queryByTestId('ib-attachment-chip')).not.toBeInTheDocument();
  });

  it('drops a late attachment result when the thread editor is replaced', async () => {
    const task = deferred<client.BbsAttachmentValidation[]>();
    vi.mocked(client.bbsValidateAttachments).mockReturnValueOnce(task.promise);
    const props = { value: '', onChange: () => {}, onStateChange: () => {}, onError: vi.fn() };
    const view = render(<BbsEditor key="thread-one" {...props} />);
    dropFile(screen.getByTestId('input-field'), '/tmp/old.pdf');
    view.rerender(<BbsEditor key="thread-two" {...props} value="New thread" />);
    await act(async () => task.resolve([{ path: '/tmp/old.pdf', name: 'old.pdf', sizeBytes: 1 }]));
    expect(screen.queryByTestId('ib-attachment-chip')).not.toBeInTheDocument();
    expect(screen.getByTestId('input-field')).toHaveTextContent('New thread');
  });

  it('rejects the tenth attachment and oversized clipboard pixels before saving', async () => {
    const ref = createRef<BbsEditorHandle>();
    const onError = vi.fn();
    const save = vi.fn();
    render(<BbsEditor ref={ref} value="" onChange={() => {}} onStateChange={() => {}} onError={onError} onPasteImage={save} />);
    for (let index = 0; index < 9; index += 1) {
      dropFile(screen.getByTestId('input-field'), `/tmp/file-${index}.pdf`);
      await waitFor(() => expect(screen.getAllByTestId('ib-attachment-chip')).toHaveLength(index + 1));
    }
    dropFile(screen.getByTestId('input-field'), '/tmp/too-many.pdf');
    await waitFor(() => expect(onError).toHaveBeenCalledWith(expect.stringContaining('9 attachments max')));
    const file = new File(['x'], 'huge.png', { type: 'image/png' });
    Object.defineProperty(file, 'size', { value: BBS_ATTACHMENT_MAX_BYTES + 1 });
    fireEvent.paste(screen.getByTestId('input-field'), { clipboardData: { files: [file], getData: () => '' } });
    await waitFor(() => expect(onError).toHaveBeenCalledWith(expect.stringContaining('exceeds 1 GiB')));
    expect(save).not.toHaveBeenCalled();
  });
});

describe('BBS attachment presentation', () => {
  it('waits without a false error, then renders the existing image style', async () => {
    const task = deferred<string>();
    vi.spyOn(client, 'fileImageDataUrl').mockReturnValueOnce(task.promise);
    const item = attachment('att-loading');
    render(<BbsAttachments postId="post-one" body="" baseRoot="/tmp/project" attachments={[item]} />);
    expect(screen.queryByText('Image unavailable')).not.toBeInTheDocument();
    await act(async () => task.resolve('data:image/png;base64,dGVzdA=='));
    expect(await screen.findByRole('img', { name: item.name })).toHaveClass('bbs-post-image');
  });

  it('shows an unavailable placeholder on read/decode failure and never loads an unavailable manifest path', async () => {
    const load = vi.spyOn(client, 'fileImageDataUrl').mockRejectedValue(new Error('not found'));
    render(<BbsAttachments postId="post-one" body="/tmp/body-missing.png" baseRoot="/tmp/project"
      attachments={[{ ...attachment('att-missing'), available: false }]} />);
    await waitFor(() => expect(screen.getAllByText('Image unavailable')).toHaveLength(2));
    expect(load).toHaveBeenCalledTimes(1);
    expect(load).toHaveBeenCalledWith('/tmp/body-missing.png');
  });

  it('deduplicates the same resolved path and rejects traversal metadata', async () => {
    const load = vi.spyOn(client, 'fileImageDataUrl').mockResolvedValue('data:image/png;base64,dGVzdA==');
    const item = attachment('att-dedupe');
    render(<BbsAttachments postId="post-one" body={item.localPath} baseRoot="/tmp/project" attachments={[
      item, { ...attachment('att-invalid'), path: '../../private.png', localPath: '/tmp/private.png' },
    ]} />);
    expect(await screen.findAllByRole('img')).toHaveLength(1);
    expect(load).toHaveBeenCalledTimes(1);
    expect(load).toHaveBeenCalledWith(item.localPath);
  });

  it('shows the name and full selectable local path for non-image attachments', () => {
    const item = attachment('att-file', 'pdf');
    render(<BbsAttachments postId="post-one" body="" baseRoot="/tmp/project" attachments={[item]} />);
    expect(screen.getByText('original.pdf')).toBeInTheDocument();
    expect(screen.getByText(item.localPath).tagName).toBe('CODE');
  });

  it('keeps all nine attachments visible without increasing the six-image decode budget', async () => {
    const load = vi.spyOn(client, 'fileImageDataUrl').mockResolvedValue('data:image/png;base64,dGVzdA==');
    const items = Array.from({ length: 9 }, (_, index) => attachment(`att-budget-${index}`));
    render(<BbsAttachments postId="post-one" body="" baseRoot="/tmp/project" attachments={items} />);
    expect(await screen.findAllByRole('img')).toHaveLength(6);
    expect(load).toHaveBeenCalledTimes(6);
    for (const item of items.slice(6)) expect(screen.getByText(item.localPath)).toBeInTheDocument();
  });

  it('shows a fallback for a recognized but unresolvable legacy image reference', () => {
    const load = vi.spyOn(client, 'fileImageDataUrl');
    render(<BbsAttachments postId="post-one" body="~/screenshots/missing.png" baseRoot="/tmp/project" />);
    expect(screen.getByText('Image unavailable')).toBeInTheDocument();
    expect(load).not.toHaveBeenCalled();
  });

  it('invalidates a resident-path preview when a promoted Fork has different bytes', async () => {
    const load = vi.spyOn(client, 'fileImageDataUrl').mockResolvedValueOnce('data:image/png;base64,b2xk')
      .mockResolvedValueOnce('data:image/png;base64,bmV3');
    const item = attachment('att-promoted');
    const view = render(<BbsAttachments postId="post-one" body="" baseRoot={null} attachments={[item]} />);
    expect(await screen.findByRole('img')).toHaveAttribute('src', 'data:image/png;base64,b2xk');
    view.rerender(<BbsAttachments postId="post-one" body="" baseRoot={null} attachments={[{ ...item, sha256: 'b'.repeat(64) }]} />);
    await waitFor(() => expect(screen.getByRole('img')).toHaveAttribute('src', 'data:image/png;base64,bmV3'));
    expect(load).toHaveBeenCalledTimes(2);
  });

  it('does not let a late preview from another version overwrite the current one', async () => {
    const old = deferred<string>();
    vi.spyOn(client, 'fileImageDataUrl').mockReturnValueOnce(old.promise).mockResolvedValueOnce('data:image/png;base64,bmV3');
    const item = attachment('att-late-version');
    const view = render(<BbsAttachments postId="post-one" body="" baseRoot={null} attachments={[item]} />);
    view.rerender(<BbsAttachments postId="post-one" body="" baseRoot={null} attachments={[{ ...item, sha256: 'b'.repeat(64) }]} />);
    expect(await screen.findByRole('img')).toHaveAttribute('src', 'data:image/png;base64,bmV3');
    await act(async () => old.resolve('data:image/png;base64,b2xk'));
    expect(screen.getByRole('img')).toHaveAttribute('src', 'data:image/png;base64,bmV3');
  });
});

const workspace: client.WorkspaceProject = {
  projectId: 'bbs-test', repoFullName: 'mock/bbs-test', remoteUrl: '', githubHtmlUrl: '', defaultBranch: 'main', baseRef: 'main',
  localRoot: '/tmp/bbs-test', localRootBytes: 0, sourceDir: '/tmp/bbs-test/source', sourceDirBytes: 0,
  sharedDir: '/tmp/bbs-test/project-memory', rulesDir: '/tmp/bbs-test/rules', agents: [],
};

function board(): client.BbsSnapshot {
  return { projectId: workspace.projectId, projectDisplayName: 'BBS Test', root: '/tmp/bbs', newCount: 0,
    threads: [{ threadId: 'thread-one', visibility: 'broadcast', projectTags: [], projectTagLabels: [],
      createdByProject: workspace.projectId, createdByProjectLabel: 'BBS Test', updatedAt: '2026-09-10T01:00:00Z',
      latestPostId: 'post-one', isNew: false, relevant: true, posts: [{ postId: 'post-one', threadId: 'thread-one',
        projectId: workspace.projectId, projectDisplayName: 'BBS Test', agentId: 'human', agentDisplayName: 'User',
        createdAt: '2026-09-10T01:00:00Z', kind: 'topic', body: 'Existing topic', preview: 'Existing topic', state: 'none', external: false,
      }] }],
  };
}

async function openReply() {
  vi.spyOn(client, 'bbsSnapshot').mockResolvedValue(board());
  const sharedMaterialize = vi.fn();
  render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot}
    roomAgents={['alice']} agentMeta={AGENTS} onMaterializeAttachments={sharedMaterialize} />);
  await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
  const dialog = await screen.findByRole('dialog', { name: 'Bulletin Board' });
  await userEvent.click(await within(dialog).findByRole('button', { name: /Existing topic/ }));
  return { dialog, field: within(dialog).getByTestId('input-field'), sharedMaterialize };
}

describe('Account-wide BBS view', () => {
  it('shows other projects without a project filter and can open and reply to their threads', async () => {
    const snapshot = board();
    const other = {
      ...snapshot.threads[0], threadId: 'thread-other', relevant: false,
      createdByProject: 'other-project', createdByProjectLabel: 'Other Project',
      posts: [{ ...snapshot.threads[0].posts[0], postId: 'post-other', threadId: 'thread-other',
        projectId: 'other-project', projectDisplayName: 'Other Project',
        body: 'A different project topic', preview: 'A different project topic', external: true }],
    };
    snapshot.threads.push(other);
    vi.spyOn(client, 'bbsSnapshot').mockResolvedValue(snapshot);
    render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />);
    await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
    const dialog = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    expect(within(dialog).getByText('All threads')).toBeInTheDocument();
    expect(dialog.querySelector('.bbs-tabs')).toBeNull();
    expect(within(dialog).queryByRole('button', { name: /^For / })).not.toBeInTheDocument();
    expect(await within(dialog).findByRole('button', { name: /Existing topic/ })).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole('button', { name: /A different project topic/ }));
    expect(within(dialog).getByTestId('input-field')).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Reply' })).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole('button', { name: /Threads/ }));
    expect(within(dialog).getByRole('button', { name: /A different project topic/ })).toBeInTheDocument();
  });

  it('shows a board-wide empty state without suggesting a hidden filter', async () => {
    vi.spyOn(client, 'bbsSnapshot').mockResolvedValue({ ...board(), threads: [] });
    render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />);
    await userEvent.click(screen.getByRole('button', { name: /^Open$/ }));
    const dialog = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    expect(await within(dialog).findByText('No BBS threads yet.')).toBeInTheDocument();
    expect(within(dialog).getByText('All threads')).toBeInTheDocument();
    expect(dialog.querySelector('.bbs-filters')).toBeNull();
  });
});

describe('BBS publication boundaries', () => {
  it('publishes a new attachment-only topic through the BBS metadata channel', async () => {
    const publish = vi.spyOn(client, 'bbsHumanPost').mockResolvedValue('thread-one');
    const { dialog, sharedMaterialize } = await openReply();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Close Bulletin Board' }));
    await userEvent.click(screen.getByRole('button', { name: 'Post' }));
    const compose = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    dropFile(within(compose).getByTestId('input-field'), '/tmp/topic.png');
    await within(compose).findByTestId('ib-attachment-chip');
    await userEvent.click(within(compose).getByRole('button', { name: 'Post' }));
    await waitFor(() => expect(publish).toHaveBeenCalledWith(expect.objectContaining({
      projectId: workspace.projectId, body: '', attachments: [{ path: '/tmp/topic.png', name: 'topic.png' }],
    })));
    await waitFor(() => expect(within(compose).queryByTestId('ib-attachment-chip')).not.toBeInTheDocument());
    expect(sharedMaterialize).not.toHaveBeenCalled();
  });

  it('keeps text and pills on archive failure; does not invoke the shared composer materializer', async () => {
    const publish = vi.spyOn(client, 'bbsHumanReply').mockRejectedValue(new Error('could not attach report.pdf: disk full'));
    const { dialog, field, sharedMaterialize } = await openReply();
    await userEvent.type(field, 'Keep this reply');
    dropFile(field, '/tmp/report.pdf');
    await within(dialog).findByTestId('ib-attachment-chip');
    await userEvent.click(within(dialog).getByRole('button', { name: 'Reply' }));
    expect(await within(dialog).findByText(/disk full/)).toBeInTheDocument();
    expect(field).toHaveTextContent('Keep this reply');
    expect(within(dialog).getByTestId('ib-attachment-chip')).toBeInTheDocument();
    expect(sharedMaterialize).not.toHaveBeenCalled();
    expect(publish).toHaveBeenCalledWith(expect.objectContaining({ body: 'Keep this reply', attachments: [{ path: '/tmp/report.pdf', name: 'report.pdf' }] }));
  });

  it('permits a pure attachment reply and disables Send while it is preparing', async () => {
    const task = deferred<client.BbsAttachmentValidation[]>();
    vi.mocked(client.bbsValidateAttachments).mockReturnValueOnce(task.promise);
    const publish = vi.spyOn(client, 'bbsHumanReply').mockResolvedValue('post-reply');
    const { dialog, field } = await openReply();
    dropFile(field, '/tmp/only.pdf');
    expect(within(dialog).getByRole('button', { name: 'Preparing…' })).toBeDisabled();
    await act(async () => task.resolve([{ path: '/tmp/only.pdf', name: 'only.pdf', sizeBytes: 1 }]));
    await userEvent.click(within(dialog).getByRole('button', { name: 'Reply' }));
    await waitFor(() => expect(publish).toHaveBeenCalledWith(expect.objectContaining({ body: '', attachments: expect.any(Array) })));
    await waitFor(() => expect(within(dialog).queryByTestId('ib-attachment-chip')).not.toBeInTheDocument());
  });

  it('clears after backend publication and never sends a duplicate frontend notification', async () => {
    const publish = vi.spyOn(client, 'bbsHumanReply').mockResolvedValue('post-reply');
    const bus = vi.spyOn(client, 'agentBusSend').mockRejectedValue(new Error('must not send here'));
    vi.mocked(bbsRosterSource.read).mockResolvedValue({ version: 'a'.repeat(64), devices: [{ deviceId: null,
      name: 'This Mac', local: true, online: true, rosterStatus: 'synced', receivedAt: null,
      projects: [{ projectId: 'bbs-test', name: 'Test', agents: [{ agentId: 'alice', name: 'Alice',
        targetRef: 'local/bbs-test/alice', avatar: { kind: 'none' } }] }] }] });
    const { dialog, field } = await openReply();
    await userEvent.type(field, 'Publish once');
    await userEvent.click(within(dialog).getByRole('button', { name: '@ Agent' }));
    await userEvent.click(await within(dialog).findByRole('checkbox', { name: 'Alice, This Mac, Test' }));
    await userEvent.click(within(dialog).getByRole('button', { name: 'Reply' }));
    await waitFor(() => expect(field.textContent).toBe(''));
    expect(within(dialog).getByRole('button', { name: 'Reply' })).toBeDisabled();
    expect(publish).toHaveBeenCalledTimes(1);
    expect(publish).toHaveBeenCalledWith(expect.objectContaining({ body: 'Publish once', mentions: [{ deviceId: 'local', projectId: 'bbs-test', agentId: 'alice' }] }));
    expect(bus).not.toHaveBeenCalled();
    expect(within(dialog).queryByText(/agent notification failed/)).not.toBeInTheDocument();
  });

  it('does not clear a new editor after an old publication completes', async () => {
    const task = deferred<string>();
    vi.spyOn(client, 'bbsHumanReply').mockReturnValueOnce(task.promise);
    const { dialog, field } = await openReply();
    await userEvent.type(field, 'Original reply');
    await userEvent.click(within(dialog).getByRole('button', { name: 'Reply' }));
    await userEvent.click(within(dialog).getByRole('button', { name: 'Close Bulletin Board' }));
    await userEvent.click(screen.getByRole('button', { name: 'Post' }));
    const newDialog = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    const newField = within(newDialog).getByTestId('input-field');
    // Native DOM text simulates an independent next draft; completion must not clear the new ref.
    newField.textContent = 'Next draft';
    fireEvent.input(newField);
    await act(async () => task.resolve('post-reply'));
    expect(newField).toHaveTextContent('Next draft');
  });
});
