import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { App } from '../src/App';
import * as tavern from '../src/chrome/TavernModal';
import * as pty from '../src/pty-client';

const local: pty.WorkspaceProject = {
  projectId: 'archived-fixture', displayName: 'Copper Notes', connection: { kind: 'local' },
  repoFullName: '', remoteUrl: '', githubHtmlUrl: '', defaultBranch: 'main', baseRef: 'main',
  sourceDir: '/fixture/source', localRoot: '/fixture/state', sharedDir: '/fixture/memory',
  rulesDir: '/fixture/rules', sourceDirBytes: 0, localRootBytes: 0, agents: [], archived: true,
};
const copy = 'This deletes Kota account state for the project but leaves local project files on disk.';
let archived: pty.WorkspaceProject[];

beforeEach(() => {
  window.localStorage.clear();
  pty.__resetMockSmartPtyForTests();
  archived = [{ ...local }];
  // Skip only the image preloader; exercise App's actual confirmation and Tavern.
  vi.spyOn(tavern, 'prepareTavernForOpen').mockResolvedValue();
  vi.spyOn(pty, 'listArchivedWorkspaceProjects').mockImplementation(async () => [...archived]);
  vi.spyOn(pty, 'inspectWorkspaceProject').mockResolvedValue({ dirty: false, dirtySummary: '' });
  vi.spyOn(pty, 'removeWorkspaceProject').mockImplementation(async () => {
    archived = [];
    return { ok: true, dirty: false, dirtySummary: '', project: null };
  });
  vi.spyOn(window, 'confirm').mockReturnValue(false);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

async function requestRemoval() {
  render(<App />);
  await userEvent.click(await screen.findByRole('button', { name: 'Open Tavern — Recruit, Roster, Settings' }));
  const page = await screen.findByTestId('tavern-page');
  await userEvent.click(within(page).getByRole('button', { name: 'Archived' }));
  await userEvent.click(await within(page).findByRole('button', { name: 'Remove' }));
  const name = archived[0].displayName || archived[0].repoFullName;
  return screen.findByRole('dialog', { name: `Remove ${name} from Kota?` });
}

it.each(['local', 'github'] as const)('confirms in App before removing an archived %s project, then refreshes the list', async kind => {
  if (kind === 'github') archived = [{ ...local, displayName: undefined,
    connection: { kind: 'github', accountLogin: 'fixture-owner' }, repoFullName: 'fixture-owner/notes' }];
  const dialog = await requestRemoval();
  expect(dialog.querySelector('pre')?.textContent).toBe(copy);
  expect(window.confirm).not.toHaveBeenCalled();
  expect(pty.removeWorkspaceProject).not.toHaveBeenCalled();
  expect(pty.inspectWorkspaceProject).toHaveBeenCalledWith(local.projectId);
  await userEvent.click(within(dialog).getByRole('button', { name: 'Remove' }));
  await screen.findByText('No archived projects.');
  expect(pty.removeWorkspaceProject).toHaveBeenCalledExactlyOnceWith({ projectId: local.projectId, forceDirty: false });
  expect(pty.listArchivedWorkspaceProjects).toHaveBeenCalledTimes(2);
});

it.each(['Cancel', 'Escape', 'backdrop'])('%s cancels removal and keeps the Tavern archive open', async action => {
  const dialog = await requestRemoval();
  if (action === 'Cancel') await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  else if (action === 'Escape') await userEvent.keyboard('{Escape}');
  else fireEvent.mouseDown(dialog.parentElement!);
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
  const page = screen.getByTestId('tavern-page');
  expect(within(page).getByText('Copper Notes')).toBeInTheDocument();
  await waitFor(() => expect(within(page).getByRole('button', { name: 'Remove' })).toBeEnabled());
  expect(pty.removeWorkspaceProject).not.toHaveBeenCalled();
  expect(pty.listArchivedWorkspaceProjects).toHaveBeenCalledTimes(1);
});

it.each(['dirty', 'inspection failed'])('keeps the original warning for %s and sends forceDirty only after confirmation', async state => {
  const summary = state === 'dirty' ? ' M notes.txt\n?? draft.txt' : 'Could not verify Git sync status: Error: synthetic inspect failure';
  if (state === 'dirty') vi.mocked(pty.inspectWorkspaceProject).mockResolvedValue({ dirty: true, dirtySummary: summary });
  else vi.mocked(pty.inspectWorkspaceProject).mockRejectedValue(new Error('synthetic inspect failure'));
  const dialog = await requestRemoval();
  expect(dialog.querySelector('pre')?.textContent).toBe(`${copy}\n\nUnsynced or dirty Git state was detected:\n\n${summary}`);
  expect(pty.removeWorkspaceProject).not.toHaveBeenCalled();
  await userEvent.click(within(dialog).getByRole('button', { name: 'Remove' }));
  await screen.findByText('No archived projects.');
  expect(pty.removeWorkspaceProject).toHaveBeenCalledExactlyOnceWith({ projectId: local.projectId, forceDirty: true });
});

it('keeps the archived project and exposes a backend rejection without reporting removal', async () => {
  vi.mocked(pty.removeWorkspaceProject).mockResolvedValue({ ok: false, dirty: true, dirtySummary: 'Synthetic removal failed.' });
  const dialog = await requestRemoval();
  await userEvent.click(within(dialog).getByRole('button', { name: 'Remove' }));
  await screen.findByText('Error: Synthetic removal failed.');
  expect(screen.getByText('Copper Notes')).toBeInTheDocument();
  expect(pty.listArchivedWorkspaceProjects).toHaveBeenCalledTimes(1);
  expect(within(screen.getByTestId('tavern-page')).getByRole('button', { name: 'Remove' })).toBeEnabled();
});
