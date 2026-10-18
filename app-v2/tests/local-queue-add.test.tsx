import { act, fireEvent, render, renderHook, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import { useLocalQueue } from '../src/hooks/useLocalQueue';
import { LocalWorkbenchHost } from '../src/chrome/LocalWorkbenchHost';
import * as rpc from '../src/lib/local-rpc';
import * as pty from '../src/pty-client';
import * as events from '../src/chrome/violet-room-events';
import type { LocalQueueItem, LocalScope, LocalWorkbenchView } from '../src/lib/local-rpc';

const scope: LocalScope = { projectRoot: '/fixture/queue-add', agentId: 'agent-copper', sessionId: 'fixture-session', epoch: 3 };
const key = JSON.stringify([scope.projectRoot, scope.agentId]);
const item: LocalQueueItem = { kotaId: 'older#agent-copper', customType: 'kota.user', senderId: null,
  createdAt: '2026-10-05T00:00:00Z', state: 'accepted', summary: 'Older task', truncated: false };
const snapshot: LocalWorkbenchView = { scope, state: 'busy', queued: 1, compacting: false, childPid: 42, model: null, thinkingLevel: null,
  error: null, revision: 1, runId: 1, startedAt: item.createdAt, phase: 'Output', statsRevision: 0, dropped: 0,
  queueTotal: 1, queueRevision: 1, queue: [item], blocks: [] };
const send = vi.fn();
const guards = vi.fn();
beforeEach(() => {
  vi.restoreAllMocks(); send.mockReset().mockResolvedValue(true); guards.mockReset();
  vi.spyOn(events, 'violetComposerSentHistory').mockReturnValue([{ id: 'older', projectRoot: scope.projectRoot, text: 'Older task',
    targetAgentIds: [scope.agentId], timestamp: item.createdAt, privacy: false }]);
  vi.spyOn(rpc, 'onLocalNotice').mockResolvedValue(() => {});
  vi.spyOn(rpc, 'readLocalWorkbench').mockResolvedValue(snapshot);
  vi.spyOn(rpc, 'readLocalAuth').mockResolvedValue({ provider: 'fixture', status: 'ready', authType: null });
  vi.spyOn(rpc, 'controlLocalWorkbench').mockImplementation(async (_scope, request) => {
    if (request.type === 'queue_cancel') return { cancelled: true, editToken: 'fixture-token', valid: true } as never;
    if (request.type === 'get_available_models') return { models: [] } as never;
    if (request.type === 'get_available_thinking_levels') return { levels: [] } as never;
    return { contextUsage: null } as never;
  });
});
afterEach(() => vi.restoreAllMocks());

it.each([false, true])('sends Add through the normal callback with current target privacy (%s)', async privacy => {
  const { result, rerender } = renderHook(({ privateAgents }) => useLocalQueue(scope.projectRoot, guards, send, privateAgents),
    { initialProps: { privateAgents: new Set(privacy ? [] : [scope.agentId]) } });
  act(() => { result.current.add(scope.projectRoot, scope.agentId); result.current.updateAddDraft(scope.projectRoot, scope.agentId, 'New task'); });
  rerender({ privateAgents: new Set(privacy ? [scope.agentId] : []) });
  const mentions = [{ agentId: 'agent-silver', aka: 'Silver' }];
  await act(async () => result.current.finishAdd(scope.projectRoot, scope.agentId, undefined, mentions));
  expect(send).toHaveBeenCalledExactlyOnceWith(scope.agentId, 'New task', { broadcast: false, privacy, mentions });
  expect(result.current.addDrafts.size).toBe(0);
  expect(rpc.controlLocalWorkbench).not.toHaveBeenCalled(); expect(rpc.onLocalNotice).not.toHaveBeenCalled();
  expect(guards).not.toHaveBeenCalled();
});

it('cancels an unsent Add draft without touching the queue or its retry guards', () => {
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  act(() => { result.current.add(scope.projectRoot, scope.agentId); result.current.updateAddDraft(scope.projectRoot, scope.agentId, 'Unsent'); });
  act(() => result.current.removeAddDraft(scope.projectRoot, scope.agentId));
  expect(result.current.addDrafts.size).toBe(0); expect(result.current.drafts.size).toBe(0);
  expect(send).not.toHaveBeenCalled(); expect(rpc.controlLocalWorkbench).not.toHaveBeenCalled();
  expect(events.violetComposerSentHistory).not.toHaveBeenCalled(); expect(guards).not.toHaveBeenCalled();
});

it.each(['not sent', 'error'])('keeps the Add draft after %s and waits for an explicit retry', async outcome => {
  if (outcome === 'error') send.mockRejectedValueOnce(new Error('fixture failure')); else send.mockResolvedValueOnce(false);
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  act(() => { result.current.add(scope.projectRoot, scope.agentId); result.current.updateAddDraft(scope.projectRoot, scope.agentId, 'Keep this'); });
  await act(async () => result.current.finishAdd(scope.projectRoot, scope.agentId));
  expect(result.current.addDrafts.get(key)).toMatchObject({ text: 'Keep this', busy: false });
  expect(send).toHaveBeenCalledOnce(); expect(rpc.controlLocalWorkbench).not.toHaveBeenCalled();
  if (outcome === 'error') expect(result.current.errors.get(key)).toMatch('Your draft is kept');
  await act(async () => result.current.finishAdd(scope.projectRoot, scope.agentId));
  expect(send).toHaveBeenCalledTimes(2); expect(result.current.addDrafts.size).toBe(0);
});

it('does not submit twice or discard the draft while normal sending is pending', async () => {
  let resolve!: (sent: boolean) => void;
  send.mockImplementation(() => new Promise(done => { resolve = done; }));
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  act(() => { result.current.add(scope.projectRoot, scope.agentId); result.current.updateAddDraft(scope.projectRoot, scope.agentId, 'Send once'); });
  let attempt!: Promise<void>;
  act(() => { attempt = result.current.finishAdd(scope.projectRoot, scope.agentId); });
  await act(async () => result.current.finishAdd(scope.projectRoot, scope.agentId));
  act(() => result.current.removeAddDraft(scope.projectRoot, scope.agentId));
  expect(send).toHaveBeenCalledOnce(); expect(result.current.addDrafts.get(key)?.busy).toBe(true);
  await act(async () => { resolve(true); await attempt; });
  expect(result.current.addDrafts.size).toBe(0); expect(guards).not.toHaveBeenCalled();
});

it('keeps an unsent draft across a project switch and cannot send it into the other project', async () => {
  const { result, rerender } = renderHook(({ root }) => useLocalQueue(root, guards, send), { initialProps: { root: scope.projectRoot } });
  act(() => { result.current.add(scope.projectRoot, scope.agentId); result.current.updateAddDraft(scope.projectRoot, scope.agentId, 'Scoped draft'); });
  rerender({ root: '/fixture/other' });
  await act(async () => result.current.finishAdd(scope.projectRoot, scope.agentId));
  expect(send).not.toHaveBeenCalled(); expect(result.current.addDrafts.get(key)?.text).toBe('Scoped draft');
  rerender({ root: scope.projectRoot });
  await act(async () => result.current.finishAdd(scope.projectRoot, scope.agentId));
  expect(send).toHaveBeenCalledOnce(); expect(result.current.addDrafts.size).toBe(0);
});

it('keeps Add and editing an existing item mutually exclusive without cancelling the item', async () => {
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  act(() => result.current.add(scope.projectRoot, scope.agentId));
  await act(async () => result.current.cancel(scope, item, true));
  expect(rpc.controlLocalWorkbench).not.toHaveBeenCalled(); expect(result.current.addDrafts.size).toBe(1);
  act(() => result.current.removeAddDraft(scope.projectRoot, scope.agentId));
  await act(async () => result.current.cancel(scope, item, true));
  expect(result.current.drafts.size).toBe(1);
  act(() => result.current.add(scope.projectRoot, scope.agentId));
  expect(result.current.addDrafts.size).toBe(0); expect(rpc.controlLocalWorkbench).toHaveBeenCalledOnce();
});

it.each([1, 20])('scrolls to the new Add textbox once, not on typing, with %s queued rows', async count => {
  const descriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollIntoView');
  const scrollIntoView = vi.fn();
  Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', { configurable: true, value: scrollIntoView });
  try {
    vi.mocked(rpc.readLocalWorkbench).mockResolvedValue({ ...snapshot, queued: count, queueTotal: count,
      queue: Array.from({ length: count }, (_, i) => ({ ...item, kotaId: `older-${i}#agent-copper` })) });
    const agents = { [scope.agentId]: { name: 'Copper', emoji: '', role: '', hue: '', provider: 'kota-local' } };
    render(<LocalWorkbenchHost projectRoot={scope.projectRoot} open={{ ...scope, focusRequest: 1 }} agents={agents}
      records={{}} targets={[scope.agentId]} onGuardsChange={guards} onSend={send}
      onClose={vi.fn()} onDetails={vi.fn()} onFresh={vi.fn()} onResume={vi.fn()} />);
    const add = await screen.findByRole('button', { name: 'Add' });
    await waitFor(() => expect(add).toBeEnabled());
    expect(scrollIntoView).not.toHaveBeenCalled();
    fireEvent.click(add);
    const editor = await screen.findByLabelText('New message');
    const textbox = within(editor).getByRole('textbox');
    await waitFor(() => expect(scrollIntoView.mock.contexts).toEqual([textbox]));
    expect(scrollIntoView).toHaveBeenLastCalledWith({ block: 'nearest' });
    expect(textbox).toHaveFocus();
    await userEvent.type(textbox, 'Unsent draft');
    expect(scrollIntoView).toHaveBeenCalledOnce();
    expect(send).not.toHaveBeenCalled();
    fireEvent.click(within(editor).getByRole('button', { name: 'Cancel' }));
    fireEvent.click(add);
    const next = within(await screen.findByLabelText('New message')).getByRole('textbox');
    await waitFor(() => expect(scrollIntoView.mock.contexts).toEqual([textbox, next]));
  } finally {
    if (descriptor) Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', descriptor);
    else Reflect.deleteProperty(HTMLElement.prototype, 'scrollIntoView');
  }
});

it('keeps the embedded Add draft through Hide, locks its target and pastes the normal attachment pill', async () => {
  const agents = { [scope.agentId]: { name: 'Copper', emoji: '', role: '', hue: '', provider: 'kota-local' },
    'agent-silver': { name: 'Silver', emoji: '', role: '', hue: '', provider: 'codex' } };
  const props = { projectRoot: scope.projectRoot, open: { ...scope, focusRequest: 1 }, agents, records: {}, targets: ['agent-silver'],
    privateAgents: new Set([scope.agentId]), onGuardsChange: guards, onSend: send,
    onClose: vi.fn(), onDetails: vi.fn(), onFresh: vi.fn(), onResume: vi.fn() };
  const view = render(<LocalWorkbenchHost {...props} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Add' }));
  let editor = await screen.findByLabelText('New message');
  expect(within(editor).getByRole('button', { name: 'Copper' })).toBeDisabled();
  expect(within(editor).queryByRole('button', { name: 'Silver' })).not.toBeInTheDocument();
  expect(editor.querySelector('.input-bar-wrap')).toHaveAttribute('data-privacy', 'true');
  expect(screen.getByRole('button', { name: 'Edit' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Remove' })).not.toBeDisabled();
  await userEvent.type(within(editor).getByTestId('input-field'), 'Screenshot task');
  view.rerender(<LocalWorkbenchHost {...props} open={null} />);
  const reads = vi.mocked(rpc.readLocalWorkbench).mock.calls.length;
  await new Promise(done => setTimeout(done, 180));
  expect(rpc.readLocalWorkbench).toHaveBeenCalledTimes(reads);
  view.rerender(<LocalWorkbenchHost {...props} />);
  editor = await screen.findByLabelText('New message');
  expect(within(editor).getByTestId('input-field')).toHaveTextContent('Screenshot task');
  const path = scope.projectRoot + '/project-memory/attachments/composer/fixture/screenshot.png';
  vi.spyOn(pty, 'saveComposerClipboardImage').mockResolvedValue(path);
  const file = new File(['synthetic pixels'], 'screenshot.png', { type: 'image/png' });
  fireEvent.paste(within(editor).getByTestId('input-field'), { clipboardData: { files: [file], getData: () => '' } });
  expect(await screen.findByTestId('ib-attachment-chip')).toHaveAttribute('data-path', path);
  expect(pty.saveComposerClipboardImage).toHaveBeenCalledWith(file, { projectRoot: scope.projectRoot });
  fireEvent.click(within(editor).getByRole('button', { name: 'Queue message' }));
  await waitFor(() => expect(send).toHaveBeenCalledExactlyOnceWith(scope.agentId, 'Screenshot task ' + path,
    { broadcast: false, privacy: true, mentions: [], eligibilityText: 'Screenshot task' }));
  expect(screen.queryByLabelText('New message')).not.toBeInTheDocument();
  expect(snapshot.queue).toEqual([item]);
  expect(vi.mocked(rpc.controlLocalWorkbench).mock.calls.some(([, request]) => typeof request.type === 'string' && request.type.startsWith('queue_'))).toBe(false);
});
