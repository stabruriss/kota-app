import { act, fireEvent, render, renderHook, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import * as rpc from '../src/lib/local-rpc';
import * as pty from '../src/pty-client';
import * as events from '../src/chrome/violet-room-events';
import { localQueueGuardKey, readLocalQueueOriginal } from '../src/lib/local-queue';
import { useLocalQueue } from '../src/hooks/useLocalQueue';
import { LocalWorkbenchHost } from '../src/chrome/LocalWorkbenchHost';
import { applyLocalQueueRetryGuards } from '../src/chrome/VioletRoomPanel';
type VioletRoomMessage = Parameters<typeof applyLocalQueueRetryGuards>[0][number];
import type { LocalNotice, LocalQueueItem, LocalScope, LocalWorkbenchView } from '../src/lib/local-rpc';

const scope: LocalScope = { projectRoot: '/fixture/queue', agentId: 'agent-copper', sessionId: 's', epoch: 7 };
const item = (source = 'kota.user', message = 'original'): LocalQueueItem => ({ kotaId: `${message}#${scope.agentId}`,
  customType: source, senderId: null, createdAt: '2026-10-04T00:00:00Z', state: 'accepted', summary: 'Clipped summary', truncated: true });
const original = 'Full original text, including the tail that is not in the queue summary.';
const guards = vi.fn(); const send = vi.fn();
let notice: ((value: LocalNotice) => void) | undefined;
const room = (messages: pty.VioletChatMessage[] = []): pty.VioletRoomState => ({ messages, sources: [], workEvents: [], agentBusReceipts: [],
  rawLogDir: '/fixture/raw', chathistoryDir: '/fixture/history', syncedAt: '2026-10-04T00:00:00Z' });
beforeEach(() => {
  vi.restoreAllMocks(); guards.mockReset(); send.mockReset().mockResolvedValue(true); notice = undefined;
  vi.spyOn(events, 'violetComposerSentHistory').mockReturnValue([{ id: 'original', projectRoot: scope.projectRoot, text: original,
    targetAgentIds: [scope.agentId], timestamp: item().createdAt, privacy: true }]);
  vi.spyOn(pty, 'readVioletRoomCache').mockResolvedValue(room());
  vi.spyOn(rpc, 'onLocalNotice').mockImplementation(async (_id, callback) => { notice = callback; return () => {}; });
  vi.spyOn(rpc, 'controlLocalWorkbench').mockImplementation(async (_scope, request) => {
    switch (request.type) {
      case 'queue_cancel': return { cancelled: true, editToken: request.edit ? 'edit-token' : undefined, valid: true };
      case 'queue_edit_valid': return { valid: true };
      case 'queue_requeue': return { queued: true };
      case 'queue_release': return { released: true };
      case 'get_available_models': return { models: [] };
      case 'get_available_thinking_levels': return { levels: [] };
      case 'get_session_stats': return { contextUsage: null };
      default: return {};
    }
  });
});
afterEach(() => vi.restoreAllMocks());
const currentGuards = (): ReadonlySet<string> => guards.mock.calls.at(-1)?.[0] ?? new Set();

it('takes the complete Composer original before cancelling; missing text does not remove the item', async () => {
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  await act(async () => result.current.cancel(scope, item(), true));
  const draft = result.current.drafts.get(result.current.ownerKey(scope.projectRoot, scope.agentId))!;
  expect(draft.text).toBe(original); expect(draft.original.privacy).toBe(true);
  expect(currentGuards()).toContain(localQueueGuardKey(scope.projectRoot, 'original', scope.agentId));
  expect(pty.readVioletRoomCache).not.toHaveBeenCalled();
  expect(rpc.controlLocalWorkbench).toHaveBeenCalledWith(scope, { type: 'queue_cancel', kotaId: item().kotaId, edit: true });
  const second = renderHook(() => useLocalQueue(scope.projectRoot, vi.fn(), send));
  vi.mocked(rpc.controlLocalWorkbench).mockClear();
  await act(async () => second.result.current.cancel(scope, item('kota.user', 'missing'), true));
  expect(rpc.controlLocalWorkbench).not.toHaveBeenCalled();
  expect([...second.result.current.errors.values()]).toEqual(["Couldn't find the complete original message. Nothing was removed."]);
});

it('refuses Composer editing when only the room cache has its text without a privacy flag', async () => {
  vi.mocked(events.violetComposerSentHistory).mockReturnValue([]);
  vi.mocked(pty.readVioletRoomCache).mockResolvedValue(room([{ id: 'original', agentId: scope.agentId,
    role: 'user', shell: 'kota-local', kind: 'message', sessionId: scope.sessionId,
    timestamp: item().createdAt, text: original, sourcePath: null, targetAgentIds: [scope.agentId] }]));
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  await act(async () => result.current.cancel(scope, item(), true));
  expect(rpc.controlLocalWorkbench).not.toHaveBeenCalled();
  expect(pty.readVioletRoomCache).not.toHaveBeenCalled();
  expect(result.current.drafts.size).toBe(0);
  expect(currentGuards().size).toBe(0);
  expect(send).not.toHaveBeenCalled();
  expect([...result.current.errors.values()]).toEqual(["Couldn't find the complete original message. Nothing was removed."]);
});

it('reads LM original text from its durable actor record, excluding wrong projects and targets', async () => {
  const message: pty.VioletChatMessage = { id: 'actor-record', nativeEventId: 'lm-input', agentId: 'laughing-man', role: 'assistant',
    shell: 'system', kind: 'message', sessionId: 'lm', timestamp: item().createdAt, text: original,
    sourcePath: null, actorIntent: 'telegram', targetAgentIds: [scope.agentId] };
  vi.mocked(pty.readVioletRoomCache).mockResolvedValue(room([message]));
  expect(await readLocalQueueOriginal(scope, item('kota.lm', 'lm-input'))).toEqual({ text: original, privacy: false });
  expect(pty.readVioletRoomCache).toHaveBeenCalledWith({ projectRoot: scope.projectRoot, limit: 200 });
  vi.mocked(pty.readVioletRoomCache).mockResolvedValue(room([{ ...message, targetAgentIds: ['agent-silver'] }]));
  expect(await readLocalQueueOriginal(scope, item('kota.lm', 'lm-input'))).toBeNull();
});

it.each(['kota.user', 'kota.lm', 'kota.bus', 'kota.bbs', 'kota.ember', 'kota.dream', 'kota.bartender'])('precise Remove protects only that target for %s', async source => {
  const { result, rerender } = renderHook(({ root }) => useLocalQueue(root, guards, send), { initialProps: { root: scope.projectRoot } });
  await act(async () => result.current.cancel(scope, item(source), false));
  expect(currentGuards()).toEqual(new Set([localQueueGuardKey(scope.projectRoot, 'original', scope.agentId)]));
  expect(result.current.drafts.size).toBe(0); expect(send).not.toHaveBeenCalled();
  rerender({ root: '/fixture/elsewhere' });
  expect(currentGuards()).toContain(localQueueGuardKey(scope.projectRoot, 'original', scope.agentId));
  expect(currentGuards()).not.toContain(localQueueGuardKey(scope.projectRoot, 'original', 'agent-silver'));
});

it('keeps the row on too-late or failure and never opens a replacement draft', async () => {
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  vi.mocked(rpc.controlLocalWorkbench).mockResolvedValueOnce({ cancelled: false, reason: 'too_late' });
  await act(async () => result.current.cancel(scope, item(), true));
  expect(result.current.drafts.size).toBe(0); expect(currentGuards().size).toBe(0);
  expect([...result.current.errors.values()][0]).toMatch('Already sent');
  vi.mocked(rpc.controlLocalWorkbench).mockRejectedValueOnce(new Error('unexpected handler failure'));
  await act(async () => result.current.cancel(scope, item(), false));
  expect([...result.current.errors.values()][0]).toBe("Couldn't remove this message.");
  expect(currentGuards().size).toBe(0);
});

it('permits two pending removals without blocking unrelated rows or losing their identities', async () => {
  const resolvers: ((value: unknown) => void)[] = [];
  vi.mocked(rpc.controlLocalWorkbench).mockImplementation(() => new Promise(resolve => resolvers.push(resolve)));
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  let first!: Promise<void>, second!: Promise<void>;
  act(() => { first = result.current.cancel(scope, item('kota.bus', 'one'), false); second = result.current.cancel(scope, item('kota.ember', 'two'), false); });
  expect(result.current.pending.size).toBe(2);
  await act(async () => { resolvers[0]({ cancelled: true }); await first; });
  expect(result.current.pending.size).toBe(1);
  await act(async () => { resolvers[1]({ cancelled: true }); await second; });
  expect(result.current.pending.size).toBe(0);
  expect(currentGuards().size).toBe(2);
});

it('reserves one editor during the first cancellation so a second Edit cannot overwrite its draft', async () => {
  let resolve!: (value: unknown) => void;
  vi.mocked(rpc.controlLocalWorkbench).mockImplementationOnce(() => new Promise(r => { resolve = r; }));
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  let first!: Promise<void>;
  act(() => { first = result.current.cancel(scope, item(), true); });
  await waitFor(() => expect(rpc.controlLocalWorkbench).toHaveBeenCalledOnce());
  await act(async () => result.current.cancel(scope, item('kota.user', 'another'), true));
  expect(rpc.controlLocalWorkbench).toHaveBeenCalledOnce();
  await act(async () => { resolve({ cancelled: true, editToken: 'edit-token', valid: true }); await first; });
  expect([...result.current.drafts.values()][0].item.kotaId).toBe(item().kotaId);
});

it.each([true, false])('Cancel or unchanged Queue reuses the original ID without a new room bubble (%s)', async restore => {
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  await act(async () => result.current.cancel(scope, item(), true));
  await act(async () => result.current.finish(scope.projectRoot, scope.agentId, restore, original));
  expect(rpc.controlLocalWorkbench).toHaveBeenCalledWith(scope, { type: 'queue_requeue', kotaId: item().kotaId, editToken: 'edit-token' });
  expect(send).not.toHaveBeenCalled(); expect(result.current.drafts.size).toBe(0); expect(currentGuards().size).toBe(0);
});

it('edited text or target uses normal sending, retains privacy, and keeps the original target protected', async () => {
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  await act(async () => result.current.cancel(scope, item(), true));
  act(() => result.current.updateDraft(scope.projectRoot, scope.agentId, { text: 'New task', target: 'agent-silver', changed: true }));
  await act(async () => result.current.finish(scope.projectRoot, scope.agentId, false, 'New task', []));
  expect(send).toHaveBeenCalledExactlyOnceWith('agent-silver', 'New task', { broadcast: false, privacy: true, mentions: [] });
  expect(rpc.controlLocalWorkbench).not.toHaveBeenCalledWith(scope, expect.objectContaining({ type: 'queue_requeue' }));
  expect(result.current.drafts.size).toBe(0);
  expect(currentGuards()).toEqual(new Set([localQueueGuardKey(scope.projectRoot, 'original', scope.agentId)]));
});

it('keeps the draft after Wake or a failed normal Send and never chains another attempt', async () => {
  send.mockResolvedValue(false);
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  await act(async () => result.current.cancel(scope, item(), true));
  act(() => result.current.updateDraft(scope.projectRoot, scope.agentId, { text: 'New task', changed: true }));
  await act(async () => result.current.finish(scope.projectRoot, scope.agentId, false, 'New task'));
  expect(send).toHaveBeenCalledOnce(); expect(result.current.drafts.size).toBe(1);
  expect([...result.current.drafts.values()][0].busy).toBe(false);
  expect(currentGuards().size).toBe(1);
});

it.each(['fresh', 'project', 'owner'])('invalidates the token on %s, retains the text and discards late validation', async reason => {
  const { result, rerender } = renderHook(({ root }) => useLocalQueue(root, guards, send), { initialProps: { root: scope.projectRoot } });
  await act(async () => result.current.cancel(scope, item(), true));
  act(() => result.current.updateDraft(scope.projectRoot, scope.agentId, { text: 'Kept draft', changed: true }));
  let resolve!: (value: unknown) => void;
  vi.mocked(rpc.controlLocalWorkbench).mockImplementation((_scope, request) => request.type === 'queue_edit_valid'
    ? new Promise(r => { resolve = r; }) : Promise.resolve({}));
  let attempt!: Promise<void>;
  act(() => { attempt = result.current.finish(scope.projectRoot, scope.agentId, false, 'Kept draft'); });
  if (reason === 'fresh') act(() => result.current.invalidate(scope));
  if (reason === 'project') rerender({ root: '/fixture/next' });
  if (reason === 'owner') await act(async () => notice?.({ kind: 'status', scope: { ...scope, epoch: 8 }, state: 'ready', queued: 0, compacting: false }));
  await act(async () => { resolve({ valid: true }); await attempt; });
  const draft = [...result.current.drafts.values()][0]; expect(draft.text).toBe('Kept draft'); expect(draft.valid).toBe(false);
  expect(send).not.toHaveBeenCalled(); expect(currentGuards().size).toBe(0);
  await act(async () => result.current.finish(scope.projectRoot, scope.agentId, true));
  expect(rpc.controlLocalWorkbench).not.toHaveBeenCalledWith(scope, expect.objectContaining({ type: 'queue_requeue' }));
});

it.each([true, false])('ordinary sending already handed off closes on true, keeps the draft on false (%s)', async sent => {
  let resolve!: (value: boolean) => void;
  send.mockImplementation(() => new Promise(r => { resolve = r; }));
  const { result } = renderHook(() => useLocalQueue(scope.projectRoot, guards, send));
  await act(async () => result.current.cancel(scope, item(), true));
  act(() => result.current.updateDraft(scope.projectRoot, scope.agentId, { text: 'Explicit new input', changed: true }));
  let attempt!: Promise<void>;
  act(() => { attempt = result.current.finish(scope.projectRoot, scope.agentId, false, 'Explicit new input'); });
  await waitFor(() => expect(send).toHaveBeenCalledOnce());
  act(() => result.current.invalidate(scope));
  await act(async () => { resolve(sent); await attempt; });
  expect(result.current.drafts.size).toBe(sent ? 0 : 1);
});

const record = (overrides: Partial<VioletRoomMessage> = {}): VioletRoomMessage => ({ id: 'room-input', sessionId: 's', agentId: scope.agentId,
  shell: 'kota-local', role: 'user', kind: 'message', timestamp: item().createdAt, text: 'Original body', sourcePath: null,
  targetAgentIds: [scope.agentId], deliveryStatus: 'failed', deliveryReason: 'Original failure', deliveryRetryTargetAgentIds: [scope.agentId], ...overrides });
it('leaves empty or unrelated guards and failed records at the exact same references', () => {
  const messages = [record()];
  expect(applyLocalQueueRetryGuards(messages, scope.projectRoot, new Set())).toBe(messages);
  expect(applyLocalQueueRetryGuards(messages, scope.projectRoot, new Set([localQueueGuardKey('/fixture/other', 'room-input', scope.agentId)]))).toBe(messages);
});
it.each(['failed', 'unconfirmed', 'retrying'] as const)('hides a protected existing %s clock without changing its backing record', status => {
  const message = record({ deliveryStatus: status }), before = structuredClone(message), messages = [message];
  const protectedKeys = new Set([localQueueGuardKey(scope.projectRoot, message.id, scope.agentId)]);
  const projected = applyLocalQueueRetryGuards(messages, scope.projectRoot, protectedKeys);
  expect(projected[0].deliveryStatus).toBeUndefined(); expect(projected[0].text).toBe(message.text);
  expect(message).toEqual(before); expect(applyLocalQueueRetryGuards(messages, scope.projectRoot, new Set())).toBe(messages);
  expect(messages[0].deliveryStatus).toBe(status);
});
it('keeps the Terminal target clock and uses the LM native identity without crossing targets', () => {
  const message = record({ id: 'actor-record', nativeEventId: 'agentbus-lm-original', role: 'assistant', agentId: 'laughing-man', shell: 'system', actorIntent: 'telegram',
    targetAgentIds: [scope.agentId, 'agent-terminal'], deliveryRetryTargetAgentIds: [scope.agentId, 'agent-terminal'] });
  const before = structuredClone(message), messages = [message];
  const projected = applyLocalQueueRetryGuards(messages, scope.projectRoot, new Set([localQueueGuardKey(scope.projectRoot, 'agentbus-lm-original', scope.agentId)]));
  expect(projected[0].deliveryStatus).toBe('failed'); expect(projected[0].deliveryRetryTargetAgentIds).toEqual(['agent-terminal']);
  expect(message).toEqual(before);
});

it('retains an embedded draft through Hide without polling or adding a second Delete / Clear all', async () => {
  const agents = { [scope.agentId]: { name: 'Copper', emoji: '', role: '', hue: '', provider: 'kota-local' },
    'agent-silver': { name: 'Silver', emoji: '', role: '', hue: '', provider: 'codex' } };
  const snapshot: LocalWorkbenchView = { scope, state: 'busy', queued: 1, compacting: false, childPid: 42, model: null, thinkingLevel: null,
    error: null, revision: 1, runId: 1, startedAt: item().createdAt, phase: 'Output', statsRevision: 0, dropped: 0,
    queueTotal: 1, queueRevision: 1, queue: [item()], blocks: [] };
  vi.spyOn(rpc, 'readLocalWorkbench').mockResolvedValue(snapshot);
  vi.spyOn(rpc, 'readLocalAuth').mockResolvedValue({ provider: 'fixture', status: 'ready', authType: null });
  const open = { ...scope, focusRequest: 1 };
  const props = { projectRoot: scope.projectRoot, open, agents, records: {}, targets: Object.keys(agents), onGuardsChange: guards, onSend: send,
    onClose: vi.fn(), onDetails: vi.fn(), onFresh: vi.fn(), onResume: vi.fn() };
  const view = render(<LocalWorkbenchHost {...props} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Edit' }));
  const editor = await screen.findByLabelText('Edit queued message');
  expect(editor.querySelector('.input-bar-wrap.embedded')).not.toBeNull();
  expect(within(editor).getAllByRole('button', { name: 'Remove' })).toHaveLength(1);
  expect(screen.queryByRole('button', { name: /delete|clear queue/i })).not.toBeInTheDocument();
  expect(within(editor).getByText(/Other queued messages continue/)).toBeInTheDocument();
  view.rerender(<LocalWorkbenchHost {...props} open={null} />);
  const count = vi.mocked(rpc.readLocalWorkbench).mock.calls.length;
  await new Promise(resolve => setTimeout(resolve, 180));
  expect(rpc.readLocalWorkbench).toHaveBeenCalledTimes(count);
  expect(currentGuards().size).toBe(1);
  view.rerender(<LocalWorkbenchHost {...props} />);
  expect(await screen.findByLabelText('Edit queued message')).toHaveTextContent('Editing message');
  fireEvent.click(within(screen.getByLabelText('Edit queued message')).getByRole('button', { name: 'Remove' }));
  await waitFor(() => expect(screen.queryByLabelText('Edit queued message')).not.toBeInTheDocument());
  expect(currentGuards().size).toBe(1); expect(send).not.toHaveBeenCalled();
});

it('does not query Pi or subscribe to Local events when no workbench or draft exists', () => {
  const props = { projectRoot: scope.projectRoot, open: null, agents: {}, records: {}, targets: [], onGuardsChange: guards,
    onSend: send, onClose: vi.fn(), onDetails: vi.fn(), onFresh: vi.fn(), onResume: vi.fn() };
  const view = render(<LocalWorkbenchHost {...props} />);
  view.rerender(<LocalWorkbenchHost {...props} projectRoot="/fixture/another" />);
  expect(rpc.controlLocalWorkbench).not.toHaveBeenCalled(); expect(rpc.onLocalNotice).not.toHaveBeenCalled();
  expect(pty.readVioletRoomCache).not.toHaveBeenCalled(); expect(send).not.toHaveBeenCalled();
});
