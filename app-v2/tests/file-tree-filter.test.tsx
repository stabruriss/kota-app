import { StrictMode } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { FileTree } from '../src/chrome/FileTree';
import type { WorkspaceNameSearchRequest, WorkspaceNameSearchResult, WorkspaceTreeEntry, WorkspaceTreeListing, WorkspaceTreePathRequest } from '../src/types/tree';

const ipc = vi.hoisted(() => ({ list: vi.fn(), search: vi.fn(), open: vi.fn(), reveal: vi.fn() }));
vi.mock('../src/pty-client', () => ({ workspaceListTreePath: ipc.list, workspaceSearchNames: ipc.search, workspaceOpenTreePath: ipc.open, workspaceRevealTreePath: ipc.reveal }));
vi.mock('../src/hooks/useKotaUpdateCheck', () => ({ useKotaUpdateCheck: () => ({ updateAvailable: false, latestVersion: null, openUpdateHome: vi.fn() }) }));

type Entry = Omit<WorkspaceTreeEntry, 'absolutePath'>;
const entry = (path: string, kind: WorkspaceTreeEntry['kind'] = 'file', extra = {}): Entry => ({
  path, name: path.split('/').at(-1)!, kind, isHidden: path.split('/').some(part => part.startsWith('.')), ...extra,
});
let folders: Map<string, Entry[]>;
const props = { projectId: 'project-one', repoName: 'Example', sourceDir: '/fixture/project-one/projectFiles', workspaceDir: '/fixture/project-one/projectWorkspace' };
function listing(request: WorkspaceTreePathRequest): WorkspaceTreeListing {
  const absolutePath = `/fixture/${request.projectId}/${request.rootKind}`;
  const entries = request.projectId === 'project-two' ? [entry('second-project.txt')]
    : folders.get(`${request.rootKind}:${request.relativePath ?? ''}`);
  if (!entries) throw new Error('Unreadable fixture folder');
  return { root: { kind: request.rootKind, label: request.rootKind, absolutePath }, entries: entries.map(item => ({ ...item, absolutePath: `${absolutePath}/${item.path}` })) };
}
// This simulates the IPC's immutable native catalogue, not a production JS index.
let snapshots: Map<number, { root: WorkspaceTreeListing['root']; entries: WorkspaceTreeEntry[]; readFailed: boolean }>;
function nativeResult(request: WorkspaceNameSearchRequest): WorkspaceNameSearchResult {
  const response: WorkspaceNameSearchResult = { generation: request.generation, queryRevision: request.queryRevision,
    root: null, entries: [], indexedCount: 0, matchedCount: 0, truncated: false, indexTruncated: false,
    readFailed: false, gitFailed: false, cancelled: !!request.cancel };
  if (request.cancel) { snapshots.delete(request.generation); return response; }
  let snapshot = snapshots.get(request.generation);
  if (!snapshot) {
    snapshot = { root: { kind: request.rootKind, label: request.rootKind, absolutePath: `/fixture/${request.projectId}/${request.rootKind}` }, entries: [], readFailed: false };
    const pending = [''];
    for (const path of pending) {
      try {
        for (const item of listing({ ...request, relativePath: path }).entries) {
          if (!request.includeIgnored && item.name === 'node_modules') continue; // fixture's .gitignore
          snapshot.entries.push(item);
          if (item.kind === 'folder' && !item.symlinkTarget && !['.git', '.hg', '.svn'].includes(item.name.toLowerCase())) pending.push(item.path);
        }
      } catch { snapshot.readFailed = true; }
    }
    snapshots.set(request.generation, snapshot);
  }
  const hits = snapshot.entries.filter(item => item.name.toLowerCase().includes(request.query.trim().toLowerCase()));
  const retained = new Set<string>();
  for (const item of hits.slice(0, 2000)) {
    let path = item.path;
    while (path && !retained.has(path)) {
      retained.add(path); const slash = path.lastIndexOf('/'); path = slash < 0 ? '' : path.slice(0, slash);
    }
  }
  return { ...response, root: snapshot.root, entries: snapshot.entries.filter(item => retained.has(item.path)),
    readFailed: snapshot.readFailed, indexedCount: snapshot.entries.length, matchedCount: Math.min(hits.length, 2000), truncated: hits.length > 2000 };
}
const listRequests = () => ipc.list.mock.calls.map(([request]) => request as WorkspaceTreePathRequest);
const requests = (cancel = false) => ipc.search.mock.calls.map(([request]) => request as WorkspaceNameSearchRequest).filter(r => !!r.cancel === cancel);
const search = () => screen.getByRole('searchbox');
const ignored = () => screen.getByRole('checkbox', { name: 'Include ignored files' });
const type = (value: string) => fireEvent.change(search(), { target: { value } });
const rowId = (path: string, root = 'projectFiles') => `tree-row-${root}-${path || 'root'}`;
const row = (path: string, root?: string) => screen.getByTestId(rowId(path, root));
const missing = (path: string, root?: string) => expect(screen.queryByTestId(rowId(path, root))).not.toBeInTheDocument();
const finish = () => waitFor(() => expect(document.querySelector('.tree-body')).toHaveAttribute('aria-busy', 'false'));
async function mount() { const view = render(<FileTree {...props} />); await screen.findByTestId(rowId('README.md')); return view; }
const settle = () => act(async () => { await Promise.resolve(); await Promise.resolve(); });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(yes => { resolve = yes; }); return { promise, resolve }; }
beforeEach(() => {
  snapshots = new Map();
  folders = new Map([
    ['projectFiles:', [entry('src', 'folder'), entry('docs', 'folder'), entry('.hidden', 'folder'), entry('node_modules', 'folder'), entry('linked', 'symlink', { symlinkTarget: '/fixture/other-project' }), entry('README.md'), entry('规划.md')]],
    ['projectFiles:src', [entry('src/deep', 'folder'), entry('src/other.ts')]],
    ['projectFiles:src/deep', [entry('src/deep/Needle.ts'), entry('src/deep/unrelated.txt')]],
    ['projectFiles:docs', [entry('docs/guide.md')]],
    ['projectFiles:.hidden', [entry('.hidden/Needle.md')]],
    ['projectFiles:node_modules', [entry('node_modules/pkg', 'folder')]],
    ['projectFiles:node_modules/pkg', [entry('node_modules/pkg/Needle.d.ts')]],
    ['projectWorkspace:', [entry('.agent-workspaces', 'folder'), entry('workspace-only.txt')]],
    ['projectWorkspace:.agent-workspaces', [entry('.agent-workspaces/agent-example', 'folder', { agentDisplayName: 'Friendly alias' })]],
    ['projectWorkspace:.agent-workspaces/agent-example', [entry('.agent-workspaces/agent-example/SHELL.yaml'), entry('.agent-workspaces/agent-example/skills', 'symlink', { symlinkTarget: '/fixture/account-skills' })]],
  ]);
  ipc.list.mockReset().mockImplementation(async (request: WorkspaceTreePathRequest) => listing(request));
  ipc.search.mockReset().mockImplementation(async (request: WorkspaceNameSearchRequest) => nativeResult(request));
  ipc.open.mockReset(); ipc.reveal.mockReset();
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe('file tree name filter', () => {
  it('hides the ignore option on empty input, but shows it for raw whitespace without searching', async () => {
    await mount(); expect(screen.queryByRole('checkbox', { name: 'Include ignored files' })).not.toBeInTheDocument();
    expect(search()).toHaveAttribute('placeholder', 'Search files and folders');
    expect(screen.queryByRole('button', { name: 'Clear search' })).not.toBeInTheDocument();
    expect(listRequests()).toEqual([{ projectId: props.projectId, rootKind: 'projectFiles', relativePath: '' }]);
    type('   '); await settle(); expect(ipc.search).not.toHaveBeenCalled();
    expect(ignored()).not.toBeChecked();
    expect(row('src')).toHaveAttribute('aria-expanded', 'false');
  });

  it.each(['button', 'Escape', 'delete'])('hides the ignore option on clear via %s and restores its checked state on retype', async method => {
    await mount(); type('needle'); await finish(); fireEvent.click(ignored()); await finish();
    expect(ignored()).toBeChecked(); const previous = requests().at(-1)!;
    if (method === 'button') fireEvent.click(screen.getByRole('button', { name: 'Clear search' }));
    else if (method === 'Escape') fireEvent.keyDown(search(), { key: 'Escape' });
    else type('');
    await settle(); expect(search()).toHaveValue('');
    expect(screen.queryByRole('checkbox', { name: 'Include ignored files' })).not.toBeInTheDocument();
    expect(requests(true)).toContainEqual(expect.objectContaining({ generation: previous.generation }));
    type('needle'); await finish(); expect(ignored()).toBeChecked();
    expect(requests().at(-1)).toMatchObject({ includeIgnored: true });
    expect(requests().at(-1)!.generation).toBeGreaterThan(previous.generation);
    expect(row('node_modules/pkg/Needle.d.ts')).toBeInTheDocument();
  });

  it('uses one native query for deep/hidden hits and ancestors, never per-directory IPC', async () => {
    await mount(); type('  NEEDLE  '); await finish();
    expect(row('src/deep/Needle.ts')).toBeInTheDocument(); expect(row('.hidden/Needle.md')).toBeInTheDocument();
    missing('node_modules/pkg/Needle.d.ts'); missing('src/other.ts'); missing('src/deep/unrelated.txt'); missing('docs'); missing('README.md');
    for (const path of ['src', 'src/deep', '.hidden']) {
      expect(row(path)).toHaveAttribute('aria-expanded', 'true');
      expect(within(row(path)).getByRole('button', { name: 'Collapse folder' })).toBeDisabled();
    }
    expect(requests()).toHaveLength(1);
    expect(requests()[0]).toMatchObject({ projectId: props.projectId, rootKind: 'projectFiles', query: 'needle', includeIgnored: false, queryRevision: 1 });
    expect(listRequests()).toHaveLength(1); expect(ipc.open).not.toHaveBeenCalled();
  });

  it('keeps the native generation for nonempty edits but increments query revisions', async () => {
    await mount(); type('docs'); await finish(); expect(row('docs')).toBeInTheDocument(); missing('docs/guide.md');
    for (const value of ['src/deep', '.*', 'workspace-only', 'Friendly alias', '<img src=x>']) {
      type(value); await finish(); expect(screen.getByRole('status')).toHaveTextContent('No matching files or folders.');
    }
    type('规划'); await finish(); expect(row('规划.md')).toBeInTheDocument();
    expect(new Set(requests().map(r => r.generation)).size).toBe(1);
    expect(requests().map(r => r.queryRevision)).toEqual([1, 2, 3, 4, 5, 6, 7]);
    expect(listRequests()).toHaveLength(1); expect(document.querySelector('img[src="x"]')).not.toBeInTheDocument();
  });

  it('changes ignore scope with a new generation, and has no 3000-directory budget in the client', async () => {
    const dirs = Array.from({ length: 3005 }, (_, i) => entry(`dir-${i}`, 'folder'));
    folders.get('projectFiles:')!.push(...dirs);
    for (const dir of dirs) folders.set(`projectFiles:${dir.path}`, []);
    folders.set('projectFiles:dir-3004', [entry('dir-3004/LateNeedle.txt')]);
    await mount(); type('needle'); await finish(); expect(row('dir-3004/LateNeedle.txt')).toBeInTheDocument();
    const first = requests()[0]; fireEvent.click(ignored()); await finish();
    expect(ignored()).toBeChecked(); expect(row('node_modules/pkg/Needle.d.ts')).toBeInTheDocument();
    expect(requests().at(-1)!.generation).toBeGreaterThan(first.generation);
    expect(requests(true)).toContainEqual(expect.objectContaining({ generation: first.generation }));
    type('node_modules'); await finish(); expect(row('node_modules')).toBeInTheDocument();
    fireEvent.click(ignored()); await finish(); missing('node_modules');
    expect(listRequests()).toHaveLength(1);
  });

  it.each([
    [{ truncated: true }, 'Search incomplete: showing first 2,000 matches'],
    [{ indexTruncated: true }, 'Search incomplete: 200,000-entry index limit reached.'],
    [{ readFailed: true }, 'Search incomplete: some folders could not be read.'],
    [{ gitFailed: true }, 'Search incomplete: Git metadata could not be loaded.'],
    [{ truncated: true, indexTruncated: true, readFailed: true, gitFailed: true }, 'Search incomplete: showing first 2,000 matches; 200,000-entry index limit reached; some folders could not be read; Git metadata could not be loaded.'],
  ])('distinguishes incomplete states %j and retains partial matches', async (flags, message) => {
    ipc.search.mockImplementation(async (r: WorkspaceNameSearchRequest) => ({ ...nativeResult(r), ...flags }));
    await mount(); type('needle'); await finish(); expect(row('src/deep/Needle.ts')).toBeInTheDocument();
    expect(screen.getByRole('status').textContent).toBe(message);
    type('absent'); await finish(); expect(screen.getByRole('status').textContent).toBe(message);
    expect(screen.queryByText('No matching files or folders.')).not.toBeInTheDocument();
  });

  it('caps primary hits at 2000 while keeping their ancestors, without confusing that with index truncation', async () => {
    folders.set('projectFiles:src/deep', Array.from({ length: 2002 }, (_, i) => entry(`src/deep/Needle-${i}.ts`)));
    await mount(); type('needle-'); await finish();
    expect(document.querySelectorAll('[data-testid^="tree-row-projectFiles-src/deep/Needle-"]')).toHaveLength(2000);
    expect(row('src')).toBeInTheDocument(); expect(row('src/deep')).toBeInTheDocument();
    expect(screen.getByRole('status').textContent).toBe('Search incomplete: showing first 2,000 matches');
    expect(screen.queryByText(/200,000-entry/)).not.toBeInTheDocument();
  });

  it.each(['Search snapshot expired. Clear search and try again.', 'arbitrary backend detail'])('ends a rejected query truthfully (%s)', async error => {
    ipc.search.mockRejectedValue(error); await mount(); type('needle'); await finish();
    expect(screen.getByRole('status').textContent).toBe(error.startsWith('Search snapshot') ? error : 'Search unavailable. Clear search and try again.');
    expect(screen.queryByText('No matching files or folders.')).not.toBeInTheDocument();
  });

  it('restores pre-search selection and expansion on clear, keeping browsing cache separate', async () => {
    await mount(); fireEvent.click(within(row('src')).getByRole('button', { name: 'Expand folder' }));
    await screen.findByTestId(rowId('src/deep')); fireEvent.click(row('README.md'));
    type('needle'); await finish(); fireEvent.click(row('src/deep/Needle.ts'));
    fireEvent.click(screen.getByRole('button', { name: 'Clear search' }));
    expect(search()).toHaveFocus(); expect(row('README.md')).toHaveClass('active');
    expect(row('src')).toHaveAttribute('aria-expanded', 'true'); expect(row('src/deep')).toHaveAttribute('aria-expanded', 'false');
    missing('src/deep/Needle.ts'); expect(requests(true)).toHaveLength(1);
    const count = listRequests().length; fireEvent(window, new Event('kota:file-tree-refresh')); await settle();
    expect(listRequests().slice(count).map(r => r.relativePath).sort()).toEqual(['', 'src']);
  });

  it('keeps tab queries/options independent; tab changes rebuild and Escape respects composition', async () => {
    await mount(); fireEvent.click(row('README.md')); type('needle'); await finish(); fireEvent.click(ignored()); await finish();
    const first = requests().at(-1)!.generation;
    fireEvent.click(screen.getByTestId('tree-tab-projectWorkspace')); await screen.findByTestId(rowId('workspace-only.txt', 'projectWorkspace'));
    expect(search()).toHaveValue(''); expect(screen.queryByRole('checkbox', { name: 'Include ignored files' })).not.toBeInTheDocument();
    type('shell'); await finish(); expect(ignored()).not.toBeChecked();
    expect(row('.agent-workspaces/agent-example/SHELL.yaml', 'projectWorkspace')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('tree-tab-projectFiles')); await finish();
    expect(search()).toHaveValue('needle'); expect(ignored()).toBeChecked(); expect(requests().at(-1)!.generation).toBeGreaterThan(first);
    fireEvent.keyDown(search(), { key: 'Escape', isComposing: true }); expect(search()).toHaveValue('needle');
    fireEvent.keyDown(search(), { key: 'Escape' }); expect(search()).toHaveValue(''); expect(search()).toHaveFocus(); expect(row('README.md')).toHaveClass('active');
  });

  it('does not follow symlink targets or treat displayed aliases as basenames', async () => {
    await mount(); type('linked'); await finish(); expect(row('linked')).toHaveTextContent('symlink');
    fireEvent.click(screen.getByTestId('tree-tab-projectWorkspace')); await settle(); type('skills'); await finish();
    expect(row('.agent-workspaces/agent-example/skills', 'projectWorkspace')).toBeInTheDocument();
    type('Friendly alias'); await finish(); expect(screen.getByRole('status')).toHaveTextContent('No matching files or folders.');
    expect(listRequests().every(r => !r.relativePath)).toBe(true);
  });

  it('shows Searching until terminal response and discards an older keyword reply', async () => {
    await mount(); const held = deferred<WorkspaceNameSearchResult>(); let first!: WorkspaceNameSearchRequest;
    ipc.search.mockImplementation((r: WorkspaceNameSearchRequest) => {
      if (r.query === 'needle') { first = r; return held.promise; } return Promise.resolve(nativeResult(r));
    });
    type('needle'); await settle(); expect(screen.getByRole('status').textContent).toBe('Searching names…');
    type('guide'); await finish(); expect(row('docs/guide.md')).toBeInTheDocument();
    await act(async () => held.resolve(nativeResult(first))); missing('src/deep/Needle.ts'); expect(row('docs/guide.md')).toBeInTheDocument();
    expect(requests()[0].generation).toBe(requests()[1].generation);
  });

  it.each(['clear', 'tab', 'unmount', 'owner'])('cancels a pending native session and discards its late reply on %s', async action => {
    const view = await mount(); const held = deferred<WorkspaceNameSearchResult>(); let first!: WorkspaceNameSearchRequest;
    ipc.search.mockImplementation((r: WorkspaceNameSearchRequest) => {
      if (!r.cancel) { first = r; return held.promise; } return Promise.resolve(nativeResult(r));
    });
    type('needle'); await settle();
    if (action === 'clear') fireEvent.click(screen.getByRole('button', { name: 'Clear search' }));
    if (action === 'tab') fireEvent.click(screen.getByTestId('tree-tab-projectWorkspace'));
    if (action === 'unmount') view.unmount();
    if (action === 'owner') view.rerender(<FileTree {...props} projectId="project-two" sourceDir="/fixture/project-two/projectFiles" />);
    await settle(); expect(requests(true)).toContainEqual(expect.objectContaining({ generation: first.generation }));
    await act(async () => held.resolve(nativeResult(first)));
    missing('src/deep/Needle.ts');
    if (action === 'owner') { expect(row('second-project.txt')).toBeInTheDocument(); expect(search()).toHaveValue(''); }
  });

  it('fences a source-root change even with the same project ID', async () => {
    const view = await mount(); type('needle'); await finish(); const first = requests()[0].generation;
    view.rerender(<FileTree {...props} sourceDir="/fixture/moved" />); await settle();
    expect(search()).toHaveValue(''); type('needle'); await finish();
    expect(requests().at(-1)!.generation).toBeGreaterThan(first);
    expect(requests(true)).toContainEqual(expect.objectContaining({ generation: first }));
  });

  it.each(['matches', 'none', 'read failure'])('does not requery native search on any browsing refresh (%s)', async state => {
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    const view = await mount(); if (state === 'read failure') folders.delete('projectFiles:docs');
    type(state === 'none' ? 'absent' : 'needle'); await finish();
    const terminal = screen.queryByRole('status')?.textContent; let token = 0;
    for (const time of [0, 1999, 2000, 60000]) {
      vi.spyOn(performance, 'now').mockReturnValue(time);
      const before = listRequests().length;
      view.rerender(<FileTree {...props} refreshToken={++token} />);
      fireEvent.focus(window); fireEvent(document, new Event('visibilitychange')); fireEvent(window, new Event('kota:file-tree-refresh')); await settle();
      expect(listRequests().slice(before).map(r => r.relativePath)).toEqual(['', '', '', '']);
      expect(requests()).toHaveLength(1); expect(screen.queryByRole('status')?.textContent).toBe(terminal);
    }
  });

  it('keeps the same native name snapshot through edits/refreshes, clear/retype gets a new one', async () => {
    const view = await mount(); type('needle'); await finish(); const first = requests()[0].generation;
    folders.set('projectFiles:src/deep', [entry('src/deep/NewNeedle.ts')]);
    view.rerender(<FileTree {...props} refreshToken={1} />); await settle();
    expect(row('src/deep/Needle.ts')).toBeInTheDocument(); missing('src/deep/NewNeedle.ts');
    type('NewNeedle'); await finish(); expect(screen.getByRole('status')).toHaveTextContent('No matching files or folders.');
    expect(requests().at(-1)!.generation).toBe(first);
    fireEvent.click(screen.getByRole('button', { name: 'Clear search' })); type('NewNeedle'); await finish();
    expect(row('src/deep/NewNeedle.ts')).toBeInTheDocument(); expect(requests().at(-1)!.generation).toBeGreaterThan(first);
  });

  it('does not schedule native search after overlapping browsing replies or a refresh storm while pending', async () => {
    const view = await mount(); const held = deferred<WorkspaceNameSearchResult>(); let first!: WorkspaceNameSearchRequest;
    ipc.search.mockImplementation((r: WorkspaceNameSearchRequest) => { first = r; return held.promise; });
    type('needle'); await settle();
    for (let token = 1; token <= 3; token++) { view.rerender(<FileTree {...props} refreshToken={token} />); fireEvent.focus(window); await settle(); }
    expect(requests()).toHaveLength(1); expect(screen.getByRole('status')).toHaveTextContent('Searching names…');
    await act(async () => held.resolve(nativeResult(first))); await finish();
    const browse = deferred<WorkspaceTreeListing>(); ipc.list.mockImplementation(() => browse.promise);
    view.rerender(<FileTree {...props} refreshToken={4} />); await settle(); view.rerender(<FileTree {...props} refreshToken={5} />); await settle();
    await act(async () => browse.resolve(listing({ projectId: props.projectId, rootKind: 'projectFiles' })));
    expect(requests()).toHaveLength(1); expect(row('src/deep/Needle.ts')).toBeInTheDocument();
  });

  it('preserves matched entry metadata, dragging and project-scoped open', async () => {
    folders.set('projectFiles:src/deep', [entry('src/deep/Needle.ts', 'file', { fileChange: { status: 'modified', addedLines: 2, deletedLines: 1, participants: [] } })]);
    await mount(); type('needle'); await finish(); const match = row('src/deep/Needle.ts');
    expect(match).toHaveTextContent('+2'); expect(match).toHaveTextContent('-1'); expect(match).toHaveAttribute('draggable', 'true');
    expect(match).toHaveAttribute('data-path', '/fixture/project-one/projectFiles/src/deep/Needle.ts');
    fireEvent.doubleClick(match); await waitFor(() => expect(ipc.open).toHaveBeenCalledWith({ projectId: props.projectId, rootKind: 'projectFiles', relativePath: 'src/deep/Needle.ts' }));
  });

  it('preserves ghost/diff markers and never system-opens a ghost', async () => {
    folders.set('projectFiles:src/deep', [entry('src/deep/GhostNeedle.ts', 'file', { isGhost: true, fileChange: { status: 'added', addedLines: 2, deletedLines: 0, participants: [] } })]);
    await mount(); type('needle'); await finish(); const match = row('src/deep/GhostNeedle.ts');
    expect(match).toHaveClass('ghost', 'change-added'); expect(within(match).getByRole('button', { name: 'Open file diff' })).toHaveTextContent('added');
    expect(match).toHaveAttribute('draggable', 'true'); fireEvent.doubleClick(match); expect(ipc.open).not.toHaveBeenCalled();
    fireEvent.contextMenu(match); expect(screen.getByText('Agent Worktree only file')).toBeInTheDocument();
  });

  it('handles StrictMode replay and ends its native generation on unmount', async () => {
    const view = render(<StrictMode><FileTree {...props} /></StrictMode>); await screen.findByTestId(rowId('README.md'));
    type('needle'); await finish(); expect(row('src/deep/Needle.ts')).toBeInTheDocument();
    const generation = requests().at(-1)!.generation; view.unmount();
    expect(requests(true)).toContainEqual(expect.objectContaining({ generation }));
  });
});
