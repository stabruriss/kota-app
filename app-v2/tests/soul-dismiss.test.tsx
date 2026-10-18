import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { App } from '../src/App';
import * as pty from '../src/pty-client';
import { __clearVioletComposerSentHistoryForTests } from '../src/chrome/violet-room-events';

const root = '/tmp/soul-dismiss-fixture';
const errorText = "Could not save this incarnation's soul to the Tavern.";
let archived: pty.ProjectAgentDetail[];
let detail: pty.ProjectAgentDetail;

beforeEach(async () => {
  window.localStorage.clear();
  window.localStorage.setItem('kota-v2.dev.project-root', root);
  pty.__resetMockSmartPtyForTests();
  __clearVioletComposerSentHistoryForTests();
  detail = {
    ...await pty.loadProjectAgentDetail({ agentId: 'soul-fixture', projectRoot: root }),
    displayName: 'Rowan', status: 'archived', projectName: 'Atlas',
  };
  archived = [detail];
  vi.spyOn(pty, 'listArchivedProjectAgents').mockImplementation(async () => [...archived]);
  vi.spyOn(window, 'alert').mockImplementation(() => {});
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

function failure(): pty.ProjectAgentLifecycleResult {
  return { ok: false, dirty: false, dirtySummary: '', detail, soulError: `${errorText} (synthetic write failure)` };
}
function success(): pty.ProjectAgentLifecycleResult {
  archived = [];
  return { ok: true, dirty: false, dirtySummary: '', detail };
}
async function openArchive() {
  render(<App />);
  await userEvent.click(await screen.findByRole('button', { name: 'Archived agents' }));
  const archive = await screen.findByRole('dialog', { name: 'Archived agents' });
  await userEvent.click(await within(archive).findByRole('button', { name: 'Dismiss' }));
  return screen.findByRole('dialog', { name: 'Delete archived agent?' });
}
async function beginDeletion() {
  const confirmation = await openArchive();
  await userEvent.click(within(confirmation).getByRole('button', { name: 'Delete' }));
}

it('keeps the initial Cancel before any dismissal request', async () => {
  const dismiss = vi.spyOn(pty, 'dismissProjectAgent').mockResolvedValue(failure());
  const confirmation = await openArchive();
  await userEvent.click(within(confirmation).getByRole('button', { name: 'Cancel' }));
  expect(dismiss).not.toHaveBeenCalled();
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
});

it('shows soul failure with Cancel focused; cancellation leaves the archived agent intact', async () => {
  const dismiss = vi.spyOn(pty, 'dismissProjectAgent').mockResolvedValue(failure());
  await beginDeletion();
  const error = await screen.findByRole('alertdialog', { name: 'Delete archived agent?' });
  const q = within(error);
  expect(q.getByText(errorText)).toBeInTheDocument();
  expect(q.getByRole('button', { name: 'Cancel' })).toHaveFocus();
  expect(q.getByRole('button', { name: 'Retry' })).toBeEnabled();
  expect(q.getByRole('button', { name: 'Dismiss anyway' })).toBeEnabled();
  await userEvent.click(q.getByRole('button', { name: 'Cancel' }));
  expect(dismiss).toHaveBeenCalledTimes(1);
  expect(within(screen.getByRole('dialog', { name: 'Archived agents' })).getByText('Rowan')).toBeInTheDocument();
  expect(window.alert).not.toHaveBeenCalled();
});

it('retries only after an explicit click and reuses the unforced request', async () => {
  const dismiss = vi.spyOn(pty, 'dismissProjectAgent')
    .mockResolvedValueOnce(failure()).mockImplementationOnce(async () => success());
  await beginDeletion();
  const error = await screen.findByRole('alertdialog');
  await act(async () => { await new Promise(resolve => setTimeout(resolve,80)); });
  expect(dismiss).toHaveBeenCalledTimes(1);
  await userEvent.click(within(error).getByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(screen.getByText('No archived agents.')).toBeInTheDocument());
  expect(dismiss).toHaveBeenCalledTimes(2);
  expect(dismiss.mock.calls[0][0]).toMatchObject({ agentId: detail.agentId, projectRoot: root, forceDirty: false, force: false });
  expect(dismiss.mock.calls[1][0]).toEqual(dismiss.mock.calls[0][0]);
});

it('adds force only for Dismiss anyway and preserves an already confirmed dirty-workspace choice', async () => {
  const dismiss = vi.spyOn(pty, 'dismissProjectAgent')
    .mockResolvedValueOnce({ ok: false, dirty: true, dirtySummary: 'M fixture.txt', detail })
    .mockResolvedValueOnce(failure())
    .mockImplementationOnce(async () => success());
  await beginDeletion();
  const dirty = await screen.findByRole('dialog', { name: 'Unsaved work in workspace' });
  await userEvent.click(within(dirty).getByRole('button', { name: 'Delete anyway' }));
  const error = await screen.findByRole('alertdialog');
  await userEvent.click(within(error).getByRole('button', { name: 'Dismiss anyway' }));
  await waitFor(() => expect(screen.getByText('No archived agents.')).toBeInTheDocument());
  expect(dismiss.mock.calls.map(([request]) => [request.forceDirty, request.force])).toEqual([
    [false, false], [true, false], [true, true],
  ]);
});

it('a failed Retry opens a fresh actionable error; an in-flight retry cannot start another deletion', async () => {
  let resolveRetry!: (value: pty.ProjectAgentLifecycleResult) => void;
  const retry = new Promise<pty.ProjectAgentLifecycleResult>(resolve => { resolveRetry = resolve; });
  const dismiss = vi.spyOn(pty, 'dismissProjectAgent')
    .mockResolvedValueOnce(failure()).mockReturnValueOnce(retry)
    .mockImplementationOnce(async () => success());
  await beginDeletion();
  await userEvent.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Retry' }));
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  await userEvent.click(within(screen.getByRole('dialog', { name: 'Archived agents' })).getByRole('button', { name: 'Dismiss' }));
  expect(screen.queryByRole('dialog', { name: 'Delete archived agent?' })).not.toBeInTheDocument();
  expect(dismiss).toHaveBeenCalledTimes(2);
  await act(async () => { resolveRetry(failure()); });
  const error = await screen.findByRole('alertdialog');
  expect(within(error).getByRole('button', { name: 'Cancel' })).toHaveFocus();
  await userEvent.click(within(error).getByRole('button', { name: 'Dismiss anyway' }));
  await waitFor(() => expect(screen.getByText('No archived agents.')).toBeInTheDocument());
  expect(dismiss).toHaveBeenCalledTimes(3);
  expect(dismiss.mock.calls[2][0]).toMatchObject({ force: true, forceDirty: false });
});

it('uses the original success path when the backend reports no soul error', async () => {
  const dismiss = vi.spyOn(pty, 'dismissProjectAgent').mockImplementation(async () => success());
  await beginDeletion();
  await waitFor(() => expect(screen.getByText('No archived agents.')).toBeInTheDocument());
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  expect(dismiss).toHaveBeenCalledTimes(1);
  expect(window.alert).not.toHaveBeenCalled();
});
