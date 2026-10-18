import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, fireEvent, render, renderHook, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { LocalAgentWorkbench, contextRemaining } from '../src/chrome/LocalAgentWorkbench';
import { LocalWorkbenchHost } from '../src/chrome/LocalWorkbenchHost';
import { useLocalWorkbench } from '../src/hooks/useLocalWorkbench';
import { useLocalQueue } from '../src/hooks/useLocalQueue';
import * as rpc from '../src/lib/local-rpc';
import * as auth from '../src/lib/local-auth';
import type { JevSettings } from '../src/lib/jev';
import type { AutoRouteGroup } from '../src/lib/string-routing';
import * as startup from '../src/lib/local-startup';
import * as clones from '../src/lib/local-shadow-clone';
import * as pty from '../src/pty-client';
import type { LocalWorkbenchView } from '../src/lib/local-rpc';

const jevIpc = vi.hoisted(() => ({settings:null as JevSettings | null, reads:0}));
vi.mock('@tauri-apps/api/core', async original => {
  const api = await original<typeof import('@tauri-apps/api/core')>();
  return {...api, invoke: (...args:Parameters<typeof api.invoke>) => {
    if (args[0] === 'local_rpc_jev_read') {jevIpc.reads += 1; return Promise.resolve(jevIpc.settings);}
    return api.invoke(...args);
  }};
});
vi.mock('@tauri-apps/api/event', () => ({listen:async () => () => {}}));

const scope = { projectRoot: '/fixture/room', agentId: 'agent-copper', sessionId: 'fixture-session', epoch: 7 };
const initial = (): LocalWorkbenchView => ({ scope, state: 'busy', queued: 1, compacting: false, childPid: 42,
  model: { provider: 'fixture', id: 'base' }, thinkingLevel: 'off', error: null,
  revision: 1, runId: 1, startedAt: new Date().toISOString(), phase: 'Output', statsRevision: 0, dropped: 0, queueTotal: 1, queueRevision: 1,
  blocks: [{ id: 'text-1', kind: 'progress', label: 'Output', text: 'Current output', active: true, error: false, truncated: false }],
  queue: [{ kotaId: 'q#agent-copper', customType: 'kota.bus', senderId: 'agent-silver', createdAt: '2026-10-03T00:00:00Z', state: 'accepted', summary: 'Check this result.', truncated: false }],
});
let snapshot: LocalWorkbenchView | null;
let calls: string[];
beforeEach(() => {
  vi.restoreAllMocks(); jevIpc.reads = 0; jevIpc.settings = {schema:1,provider:'openrouter',ready:false,keys:[{providerId:'openrouter',status:'none'}]}; snapshot = initial(); calls = [];
  Object.defineProperty(document, 'hidden', { configurable: true, value: false });
  vi.spyOn(rpc, 'readLocalWorkbench').mockImplementation(async () => snapshot ? structuredClone(snapshot) : null);
  vi.spyOn(rpc, 'setOfflineLocalModel').mockRejectedValue({code:'unexpected_offline_save'});
  vi.spyOn(rpc, 'readLocalAuth').mockResolvedValue({ provider: 'fixture', status: 'ready', authType: null });
  vi.spyOn(pty, 'loadProjectAgentDetail').mockResolvedValue({ provider: 'kota-local', model: 'fixture/base', effort: 'off' } as pty.ProjectAgentDetail);
  vi.spyOn(auth, 'readLocalAuthAvailability').mockResolvedValue({ schema: 1, summary: 'ready', providers: [{ providerId: 'fixture', status: 'signed_in', authType: 'api_key' }] });
  vi.spyOn(auth, 'readLocalModelCatalog').mockResolvedValue({ schema: 1, providers: [{ providerId: 'fixture', linked: true, models: [{ modelId: 'base', name: 'Base', thinkingLevels: ['off'] }, { modelId:'next', name:'Next', thinkingLevels:['off','low'] }] }] });
  vi.spyOn(startup, 'onLocalStartup').mockResolvedValue(() => {});
  vi.spyOn(startup, 'localLaunchAction').mockImplementation(async (_root, _id, action) => {
    if (action === 'configuration') return { model: 'fixture/base', thinkingLevel: 'off' } as never;
    if (action === 'begin') return { reused: false, view: { ...scope, attemptId: 'fixture-start', revision: 1,
      state: 'running', selection: null, saved: false, error: null, steps: [] } } as never;
    return null as never;
  });
  vi.spyOn(rpc, 'controlLocalWorkbench').mockImplementation(async (_scope, request) => {
    calls.push(String(request.type));
    switch (request.type) {
      case 'get_state': return { model: snapshot?.model, thinkingLevel: snapshot?.thinkingLevel, autoCompactionEnabled: false };
      case 'get_available_models': return { models: [{ provider: 'fixture', id: 'base' }, { provider: 'fixture', id: 'next' }] };
      case 'get_available_thinking_levels': return { levels: ['off', 'low'] };
      case 'get_session_stats': return { contextUsage: { tokens: 320, contextWindow: 1000, percent: 32 } };
      case 'queue_text': return { text: 'Expanded original text', truncated: false };
      case 'set_model': snapshot = { ...snapshot!, model: { provider: String(request.provider), id: String(request.modelId) }, thinkingLevel: 'low' }; return {};
      default: return {};
    }
  });
});
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); });
const props = () => ({ ...scope, agent: { name: 'Copper', emoji: '', role: '', hue: '', avatarId: null, provider: 'kota-local' },
  agents: { 'agent-silver': { name: 'Silver', emoji: '', role: '', hue: '' } }, record: { turns: 8, commends: 3, incarnations: 1, estimatedTokens: 0 },
  focusRequest: 1, onClose: vi.fn(), onDetails: vi.fn(), onFresh: vi.fn(), onResume: vi.fn().mockResolvedValue(undefined),
});

it('mounts the numbered B clone drawer beside MCP without replacing the parent Current turn', async () => {
  vi.spyOn(clones, 'readLocalShadowClones').mockResolvedValue({ scope, clones: [{ id: 1, state: 'running',
    startedAt: '2026-10-09T00:00:00Z', elapsedMs: 0, lastReply: '', view: { ...initial(), blocks: [
      { ...initial().blocks![0], text: 'Clone output', id: 'clone-output' },
    ] } }] });
  render(<LocalAgentWorkbench {...props()} />);
  const tab = await screen.findByRole('button', { name: 'Kage Bunshin, 1 clone' });
  expect(tab.closest('.bench-tabs')).toBe(screen.getByRole('button', { name: 'MCP servers' }).closest('.bench-tabs'));
  fireEvent.click(tab);
  const dialog = screen.getByRole('dialog', { name: 'Kage Bunshin' });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Monitor Kambei, Clone #1' }));
  await within(dialog).findByText('Clone output');
  expect(screen.getByLabelText('Current turn output')).toHaveTextContent('Current output');
  expect(within(dialog).queryByRole('heading', { name: 'Current turn' })).not.toBeInTheDocument();
  expect(within(dialog).queryByText(/Model|Thinking level|90 minutes/)).not.toBeInTheDocument();
  expect(clones.readLocalShadowClones).toHaveBeenLastCalledWith(scope.projectRoot, scope.agentId, 1);
});

it('restores the Local model trigger border after the workbench button reset', async () => {
  const style = document.createElement('style');
  style.textContent = ['local-model-picker.css', 'local-workbench.css']
    .map(file => readFileSync(resolve(__dirname, `../src/styles/${file}`), 'utf8')).join('\n');
  document.head.append(style);
  try {
    render(<LocalAgentWorkbench {...props()} />);
    const trigger = await screen.findByRole('button', {name:'Local model'});
    expect(getComputedStyle(trigger).borderTopWidth).toBe('1px');
    expect(getComputedStyle(trigger).borderTopStyle).toBe('solid');
  } finally { style.remove(); }
});

it('marks Anthropic in Workbench choices before selection and after selection, without marking other Claude models', async () => {
  const choices = [{provider:'anthropic',id:'claude-fixture'}, {provider:'magpie',id:'claude-fixture'}];
  vi.mocked(auth.readLocalModelCatalog).mockResolvedValue({schema:1, providers:choices.map(choice => ({providerId:choice.provider,linked:true,models:[{modelId:choice.id,name:'Claude fixture',thinkingLevels:['off']}]}))});
  vi.mocked(rpc.controlLocalWorkbench).mockImplementation(async (_scope, request) => {
    if (request.type === 'get_available_models') return {models:choices};
    if (request.type === 'get_state') return {model:snapshot?.model,thinkingLevel:'off'};
    if (request.type === 'get_available_thinking_levels') return {levels:['off']};
    if (request.type === 'set_model') {
      snapshot = {...snapshot!, revision:snapshot!.revision + 1, model:{provider:String(request.provider),id:String(request.modelId)}};
    }
    return {};
  });
  render(<LocalAgentWorkbench {...props()} />);
  await waitFor(() => expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled());
  const model = screen.getByRole('button', {name:'Local model'});
  await userEvent.click(model);
  const anthropic = screen.getByRole('option',{name:/Claude fixture \(Use Credit\) Anthropic/});
  const magpie = screen.getByRole('option',{name:/Magpie \/ Claude fixture/});
  expect(anthropic).toHaveTextContent('(Use Credit)');
  expect(within(anthropic).getByText('(Use Credit)')).toHaveClass('local-model-credit');
  expect(magpie).not.toHaveTextContent('(Use Credit)');
  expect(document.querySelector('.model-strip .local-model-credit')).not.toBeInTheDocument();
  await userEvent.click(anthropic);
  await waitFor(() => expect(document.querySelector('.model-strip .local-model-credit')).toHaveTextContent('(Use Credit)'));
  await userEvent.click(model);
  await userEvent.click(screen.getByRole('option',{name:/Magpie \/ Claude fixture/}));
  await waitFor(() => expect(document.querySelector('.model-strip .local-model-credit')).not.toBeInTheDocument());
});

it('times the active thinking block from its own start, alongside real streamed text, then removes its clock', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date'] });
  vi.setSystemTime(new Date('2026-10-04T12:00:08Z'));
  snapshot!.startedAt = '2026-10-04T11:53:08Z';
  snapshot!.phase = 'Thinking';
  snapshot!.blocks = [{ ...snapshot!.blocks![0], id: 'reasoning', kind: 'thinking', label: 'Thinking',
    text: '', startedAt: '2026-10-04T12:00:00Z' }];
  render(<LocalAgentWorkbench {...props()} />);
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  expect(screen.getByLabelText('Thinking elapsed')).toHaveTextContent('00:08');
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(screen.getByLabelText('Thinking elapsed')).toHaveTextContent('00:10');
  snapshot = { ...snapshot!, revision: 2, blocks: [{ ...snapshot!.blocks![0], text: 'Visible reasoning summary' }] };
  await act(async () => { await vi.advanceTimersByTimeAsync(100); });
  expect(screen.getByText('Visible reasoning summary')).toBeInTheDocument();
  expect(screen.getByLabelText('Thinking elapsed')).toHaveTextContent('00:10');
  snapshot = { ...snapshot!, revision: 3, blocks: [{ ...snapshot!.blocks![0], active: false }] };
  await act(async () => { await vi.advanceTimersByTimeAsync(100); });
  expect(screen.queryByLabelText('Thinking elapsed')).not.toBeInTheDocument();
  expect(screen.getByText('Visible reasoning summary')).toBeInTheDocument();
});

it('does not invent a thinking start for legacy blocks or add clocks to other output categories', async () => {
  snapshot!.blocks = [
    { ...snapshot!.blocks![0], id: 'legacy', kind: 'thinking', label: 'Thinking' },
    { ...snapshot!.blocks![0], id: 'tool', kind: 'tool', label: 'read', startedAt: '2026-10-04T12:00:00Z' },
  ];
  render(<LocalAgentWorkbench {...props()} />);
  await screen.findByLabelText('Current turn output');
  expect(screen.queryByLabelText('Thinking elapsed')).not.toBeInTheDocument();
});

it('unmounts the thinking clock when the workbench is hidden, without retaining polling', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date'] });
  vi.setSystemTime(new Date('2026-10-04T12:00:08Z'));
  snapshot!.blocks = [{ ...snapshot!.blocks![0], kind: 'thinking', label: 'Thinking', text: '', startedAt: '2026-10-04T12:00:00Z' }];
  const p = props();
  const host = { projectRoot: scope.projectRoot, agents: { [scope.agentId]: p.agent }, records: {}, targets: [],
    onClose: p.onClose, onDetails: p.onDetails, onFresh: p.onFresh, onResume: p.onResume,
    onSend: vi.fn().mockResolvedValue(true), onGuardsChange: vi.fn() };
  const { rerender } = render(<LocalWorkbenchHost {...host} open={{ ...scope, focusRequest: 1 }} />);
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  expect(screen.getByLabelText('Thinking elapsed')).toHaveTextContent('00:08');
  Object.defineProperty(document, 'hidden', { configurable: true, value: true });
  fireEvent(document, new Event('visibilitychange'));
  const hiddenReads = vi.mocked(rpc.readLocalWorkbench).mock.calls.length;
  await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
  expect(rpc.readLocalWorkbench).toHaveBeenCalledTimes(hiddenReads);
  expect(screen.getByLabelText('Thinking elapsed')).toHaveTextContent('00:08');
  rerender(<LocalWorkbenchHost {...host} open={null} />);
  const controls = calls.length;
  await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
  expect(screen.queryByLabelText('Thinking elapsed')).not.toBeInTheDocument();
  expect(rpc.readLocalWorkbench).toHaveBeenCalledTimes(hiddenReads);
  expect(calls).toHaveLength(controls);
});

it('uses actual model/auth/context and only supported controls, with details and Fresh delegated', async () => {
  const p = props(); render(<LocalAgentWorkbench {...p} />);
  await waitFor(() => expect(screen.getByRole('button', { name:'Local model' })).toBeEnabled());
  expect(screen.getByRole('heading', { name: 'Copper' })).toBeInTheDocument();
  expect(screen.getByText('Silver')).toBeInTheDocument();
  expect(screen.getByRole('meter')).toHaveAttribute('aria-valuenow', '68');
  expect(screen.getByText('Linked')).toBeInTheDocument();
  expect(screen.queryByText('OAuth')).not.toBeInTheDocument();
  expect(screen.getByText('Auto compaction off')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /compact/i })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /delete|edit|reassign|login/i })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', {name:'Local model'}));
  await userEvent.click(screen.getByRole('option', {name:/Next fixture/}));
  await waitFor(() => expect(screen.getByLabelText('Thinking')).toHaveValue('low'));
  expect(rpc.controlLocalWorkbench).toHaveBeenCalledWith(scope, { type: 'set_model', provider: 'fixture', modelId: 'next' });
  fireEvent.click(screen.getByRole('button', { name: 'View incarnation details' })); expect(p.onDetails).toHaveBeenCalledWith(scope.agentId);
  fireEvent.click(screen.getByRole('button', { name: 'New session' })); expect(p.onFresh).toHaveBeenCalledWith(scope.agentId);
  expect(calls).not.toContain('compact'); expect(calls).not.toContain('set_auto_compaction');
});

it('retains completed turn output and removes Stop after settled; the next run replaces it', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] });
  render(<LocalAgentWorkbench {...props()} />);
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  expect(screen.getByText('Current output')).toBeInTheDocument();
  snapshot = { ...snapshot!, revision: 2, state: 'ready', startedAt: null,
    blocks: [
      { ...snapshot!.blocks![0], active: false },
      { ...snapshot!.blocks![0], id: 'final', kind: 'end', active: false, text: 'Final response at settle' },
    ], queue: [], queueTotal: 0, queued: 0 };
  await act(async () => { await vi.advanceTimersByTimeAsync(100); });
  expect(screen.getByText('Current output')).toBeInTheDocument();
  expect(screen.getByText('Final response at settle')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Stop' })).not.toBeInTheDocument();
  expect(screen.queryByText('Ready when you are.')).not.toBeInTheDocument();
  expect(screen.getByText('No queued messages.')).toBeInTheDocument();
  snapshot = { ...snapshot, revision: 3, runId: 2, state: 'busy', startedAt: new Date().toISOString(),
    blocks: [{ ...initial().blocks![0], text: 'New run output' }] };
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
  expect(screen.getByText('New run output')).toBeInTheDocument();
  expect(screen.queryByText('Final response at settle')).not.toBeInTheDocument();
});

it('hides a retained idle run on reopen, stops hidden polling and shows the next active run', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] });
  const p = props();
  const host = { projectRoot: scope.projectRoot, agents: { [scope.agentId]: p.agent }, records: {}, targets: [],
    onClose: p.onClose, onDetails: p.onDetails, onFresh: p.onFresh, onResume: p.onResume,
    onSend: vi.fn().mockResolvedValue(true), onGuardsChange: vi.fn() };
  const { rerender } = render(<LocalWorkbenchHost {...host} open={{ ...scope, focusRequest: 1 }} />);
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  snapshot = { ...snapshot!, revision: 2, state: 'ready', startedAt: null,
    blocks: [{ ...snapshot!.blocks![0], kind: 'end', active: false, text: 'Retained final response' }] };
  await act(async () => { await vi.advanceTimersByTimeAsync(100); });
  expect(screen.getByText('Retained final response')).toBeInTheDocument();
  rerender(<LocalWorkbenchHost {...host} open={null} />);
  const reads = vi.mocked(rpc.readLocalWorkbench).mock.calls.length;
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(rpc.readLocalWorkbench).toHaveBeenCalledTimes(reads);
  rerender(<LocalWorkbenchHost {...host} open={{ ...scope, focusRequest: 2 }} />);
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  expect(screen.queryByText('Retained final response')).not.toBeInTheDocument();
  expect(screen.getByText('Ready when you are.')).toBeInTheDocument();
  snapshot = { ...snapshot, revision: 3, runId: 2, state: 'busy', startedAt: new Date().toISOString(),
    blocks: [{ ...initial().blocks![0], text: 'Active after reopening' }] };
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
  expect(screen.getByText('Active after reopening')).toBeInTheDocument();
  rerender(<LocalWorkbenchHost {...host} open={null} />);
  rerender(<LocalWorkbenchHost {...host} open={{ ...scope, focusRequest: 3 }} />);
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  expect(screen.getByText('Active after reopening')).toBeInTheDocument();
});

it('does not inherit a retained run when a replacement owner reuses its run number', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] });
  render(<LocalAgentWorkbench {...props()} />);
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  expect(screen.getByText('Current output')).toBeInTheDocument();
  snapshot = { ...snapshot!, scope: { ...scope, epoch: 8, sessionId: 'replacement' }, revision: 1,
    state: 'ready', startedAt: null,
    blocks: [{ ...snapshot!.blocks![0], kind: 'end', active: false, text: 'Older replacement run' }] };
  await act(async () => { await vi.advanceTimersByTimeAsync(100); });
  expect(screen.queryByText('Older replacement run')).not.toBeInTheDocument();
  expect(screen.queryByText('Current output')).not.toBeInTheDocument();
  expect(screen.getByText('Ready when you are.')).toBeInTheDocument();
});

it('does not offer Start before the current owner is known, then delegates an offline startup attempt', async () => {
  let resolve!: (value: LocalWorkbenchView | null) => void;
  vi.mocked(rpc.readLocalWorkbench).mockImplementationOnce(() => new Promise((r) => { resolve = r; }));
  const p = props(); render(<LocalAgentWorkbench {...p} />);
  expect(screen.getByText('Opening workbench…')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Start session' })).not.toBeInTheDocument();
  await act(async () => resolve(null));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start session' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Start session' }));
  await waitFor(() => expect(p.onResume).toHaveBeenCalledWith(scope.agentId, { attemptId: 'fixture-start' }));
});

it('uses scoped Stop without Clear all, and does not retain old context after owner replacement', async () => {
  render(<LocalAgentWorkbench {...props()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Stop' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
  await waitFor(() => expect(rpc.controlLocalWorkbench).toHaveBeenCalledWith(scope, { type: 'abort' }));
  expect(screen.queryByRole('button', { name: 'Clear queue' })).not.toBeInTheDocument();
  expect(calls).not.toContain('clear_queue');
  vi.mocked(rpc.controlLocalWorkbench).mockImplementation(() => new Promise(() => {}));
  snapshot = { ...snapshot!, scope: { ...scope, epoch: 8, sessionId: 'replacement' }, revision: 0, state: 'ready', blocks: [], queue: [] };
  await waitFor(() => expect(screen.getByRole('meter')).toHaveAttribute('aria-valuetext', 'Unavailable'));
  expect(screen.queryByText('OAuth')).not.toBeInTheDocument();
});

it('Stop preserves the queue controller and unlocks controls immediately on a failed stage', async () => {
  let queue!: ReturnType<typeof useLocalQueue>;
  function Harness() {
    queue = useLocalQueue(scope.projectRoot, vi.fn(), vi.fn());
    return <LocalAgentWorkbench {...props()} queueController={queue} />;
  }
  render(<Harness />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Stop' })).toBeEnabled());
  const invalidate = vi.spyOn(queue, 'invalidate');
  let reject!: (reason: Error) => void;
  vi.mocked(rpc.controlLocalWorkbench).mockImplementation(async (_scope, request) => {
    if (request.type === 'abort') return new Promise((_resolve, fail) => { reject = fail; });
    return {};
  });
  fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
  expect(screen.getByRole('button', { name: 'Stop' })).toBeDisabled();
  expect(invalidate).not.toHaveBeenCalled();
  await act(async () => reject(new Error('Local RPC command timed out; queued deliveries abandoned')));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Stop' })).toBeEnabled());
  expect(screen.getByText(/Local RPC command timed out; queued deliveries abandoned/)).toBeInTheDocument();
  expect(invalidate).not.toHaveBeenCalled();
});

it('retains explicit scroll position until Latest, and displays end turn without folding', async () => {
  snapshot!.blocks!.push({ ...snapshot!.blocks![0], id: 'final', kind: 'end', active: false, text: 'Final response' });
  render(<LocalAgentWorkbench {...props()} />);
  const area = await screen.findByLabelText('Current turn output');
  Object.defineProperties(area, { scrollHeight: { configurable: true, value: 2000 }, clientHeight: { configurable: true, value: 200 } });
  area.scrollTop = 100; fireEvent.scroll(area);
  snapshot = { ...snapshot!, revision: 2, blocks: [...snapshot!.blocks!, { ...snapshot!.blocks![0], id: 'more', text: 'Another block' }] };
  await screen.findByText('Another block'); expect(area.scrollTop).toBe(100);
  expect(screen.getByText('Final response').closest('details')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Latest ↓' })); expect(area.scrollTop).toBe(2000);
});

it('focuses the queue and gives modal Esc precedence over hiding the workbench', async () => {
  const p = props(); render(<LocalAgentWorkbench {...p} section="queue" />);
  expect(screen.getByLabelText('Queued messages')).toHaveFocus();
  await waitFor(() => expect(screen.getByRole('button', { name:'Local model' })).toBeEnabled());
  const dialog = document.createElement('div'); dialog.setAttribute('role', 'dialog'); document.body.append(dialog);
  fireEvent.keyDown(window, { key: 'Escape' }); expect(p.onClose).not.toHaveBeenCalled();
  dialog.remove(); fireEvent.keyDown(window, { key: 'Escape' }); expect(p.onClose).toHaveBeenCalledOnce();
  const card = screen.getByLabelText('Copper workbench');
  expect(within(card).queryByRole('img', { name: /image attachment/i })).not.toBeInTheDocument();
});

it('does not poll while hidden or unmounted, and has at most one view request in flight', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  let resolve!: (value: LocalWorkbenchView | null) => void;
  vi.mocked(rpc.readLocalWorkbench).mockImplementation(() => new Promise((r) => { resolve = r; }));
  const { unmount } = renderHook(() => useLocalWorkbench(scope.projectRoot, scope.agentId));
  await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
  expect(rpc.readLocalWorkbench).toHaveBeenCalledOnce();
  Object.defineProperty(document, 'hidden', { configurable: true, value: true });
  fireEvent(document, new Event('visibilitychange'));
  await act(async () => { resolve(snapshot); await vi.advanceTimersByTimeAsync(3000); });
  expect(rpc.readLocalWorkbench).toHaveBeenCalledOnce(); expect(calls).toEqual([]);
  Object.defineProperty(document, 'hidden', { configurable: true, value: false });
  fireEvent(document, new Event('visibilitychange'));
  expect(rpc.readLocalWorkbench).toHaveBeenCalledTimes(2);
  unmount(); await act(async () => { resolve(snapshot); await vi.advanceTimersByTimeAsync(3000); });
  expect(rpc.readLocalWorkbench).toHaveBeenCalledTimes(2); expect(calls).toEqual([]);
});

it('discards the previous project response and fences a replaced scope', async () => {
  let resolve!: (value: LocalWorkbenchView | null) => void;
  vi.mocked(rpc.readLocalWorkbench).mockImplementationOnce(() => new Promise((r) => { resolve = r; }));
  const { result, rerender } = renderHook(({ root }) => useLocalWorkbench(root, scope.agentId), { initialProps: { root: scope.projectRoot } });
  snapshot = { ...snapshot!, scope: { ...scope, projectRoot: '/fixture/other', epoch: 8 } };
  rerender({ root: '/fixture/other' });
  await waitFor(() => expect(result.current.view?.scope.epoch).toBe(8));
  await act(async () => resolve(initial()));
  expect(result.current.view?.scope.projectRoot).toBe('/fixture/other');
  snapshot = initial();
  await new Promise((r) => setTimeout(r, 160));
  expect(result.current.view?.scope.epoch).toBe(8);
});

it('shows every output category without disclosures, including completed thinking and tools', async () => {
  snapshot!.blocks = [
    { ...snapshot!.blocks![0], id: 'thinking', kind: 'thinking', active: false, text: 'Completed thinking' },
    { ...snapshot!.blocks![0], id: 'tool', kind: 'tool', active: false, text: 'line one\nline two' },
    { ...snapshot!.blocks![0], id: 'progress', kind: 'progress', active: false, text: 'Completed progress' },
    { ...snapshot!.blocks![0], id: 'answer', kind: 'end', active: false, text: 'Answer text' },
  ];
  render(<LocalAgentWorkbench {...props()} />);
  const area = await screen.findByLabelText('Current turn output');
  expect(area.querySelectorAll('details, summary')).toHaveLength(0);
  expect(area.querySelectorAll('.stream-block')).toHaveLength(4);
  expect(area.querySelectorAll('pre')).toHaveLength(4);
  expect(area.querySelector('[data-stream-block="thinking"] pre')!.textContent).toBe('Completed thinking');
  expect(area.querySelector('[data-stream-block="tool"] pre')!.textContent).toBe('line one\nline two');
  snapshot = { ...snapshot!, revision: 2, blocks: snapshot!.blocks!.map((b) => b.id === 'tool'
    ? { ...b, active: true, text: 'line one\nline two\nline three' } : b) };
  await waitFor(() => expect(area.querySelector('[data-stream-block="tool"] pre')!.textContent).toContain('line three'));
  expect(area.querySelector('[data-stream-block="tool"] .stream-append')).toBeNull();
});

it('reuses unchanged block objects and the array for queue-only revisions, never across a run or owner', async () => {
  snapshot!.blocks!.push({ ...snapshot!.blocks![0], id: 'completed', active: false, text: 'Stable block' });
  const { result } = renderHook(() => useLocalWorkbench(scope.projectRoot, scope.agentId));
  await waitFor(() => expect(result.current.view?.revision).toBe(1));
  const first = result.current.view!.blocks!;
  snapshot = { ...snapshot!, revision: 2, blocks: snapshot!.blocks!.map((b) => b.id === 'text-1' ? { ...b, text: 'Current output grows' } : b) };
  await waitFor(() => expect(result.current.view?.revision).toBe(2));
  const second = result.current.view!.blocks!;
  expect(second[0]).not.toBe(first[0]);
  expect(second[1]).toBe(first[1]);
  snapshot = { ...snapshot!, revision: 3, queueRevision: 2, queued: 0, queue: [] };
  await waitFor(() => expect(result.current.view?.revision).toBe(3));
  expect(result.current.view!.blocks).toBe(second);
  snapshot = { ...snapshot!, revision: 4, runId: 2 };
  await waitFor(() => expect(result.current.view?.revision).toBe(4));
  const nextRun = result.current.view!.blocks!;
  expect(nextRun[1]).not.toBe(second[1]);
  snapshot = { ...snapshot!, scope: { ...scope, epoch: 8, sessionId: 'new-owner' }, revision: 0 };
  await waitFor(() => expect(result.current.view?.scope.epoch).toBe(8));
  expect(result.current.view!.blocks![1]).not.toBe(nextRun[1]);
});

it('does not reuse a block when its category, label or display flags change', async () => {
  const { result } = renderHook(() => useLocalWorkbench(scope.projectRoot, scope.agentId));
  await waitFor(() => expect(result.current.view?.revision).toBe(1));
  for (const change of [{ kind: 'end' as const }, { label: 'Answer' }, { active: false }, { error: true }, { truncated: true }]) {
    const before = result.current.view!.blocks![0];
    snapshot = { ...snapshot!, revision: snapshot!.revision + 1, blocks: [{ ...snapshot!.blocks![0], ...change }] };
    await waitFor(() => expect(result.current.view?.revision).toBe(snapshot!.revision));
    expect(result.current.view!.blocks![0]).not.toBe(before);
  }
});

it('coalesces following into one frame, honors manual scroll, and cancels hidden/unmounted work', async () => {
  const frames = new Map<number, FrameRequestCallback>();
  let id = 0;
  vi.spyOn(window, 'requestAnimationFrame').mockImplementation((callback) => { frames.set(++id, callback); return id; });
  vi.spyOn(window, 'cancelAnimationFrame').mockImplementation((frame) => { frames.delete(frame); });
  const { unmount } = render(<LocalAgentWorkbench {...props()} />);
  const area = await screen.findByLabelText('Current turn output');
  const height = vi.fn(() => 2000);
  Object.defineProperties(area, { scrollHeight: { configurable: true, get: height }, clientHeight: { configurable: true, value: 200 } });
  for (let revision = 2; revision <= 3; revision++) {
    snapshot = { ...snapshot!, revision, blocks: [{ ...snapshot!.blocks![0], text: `Output ${revision}` }] };
    await waitFor(() => expect(area.querySelector('pre')!.textContent).toBe(`Output ${revision}`));
  }
  expect(frames.size).toBe(1);
  expect(height).not.toHaveBeenCalled();
  act(() => { const pending = [...frames.values()]; frames.clear(); for (const frame of pending) frame(performance.now()); });
  expect(height).toHaveBeenCalledOnce();
  expect(area.scrollTop).toBe(2000);
  area.scrollTop = 100; fireEvent.scroll(area);
  snapshot = { ...snapshot!, revision: 4, blocks: [{ ...snapshot!.blocks![0], text: 'User stays up' }] };
  await waitFor(() => expect(area.querySelector('pre')!.textContent).toBe('User stays up'));
  expect(frames.size).toBe(0); expect(area.scrollTop).toBe(100);
  fireEvent.click(screen.getByRole('button', { name: 'Latest ↓' }));
  expect(area.scrollTop).toBe(2000);
  snapshot = { ...snapshot!, revision: 5, blocks: [{ ...snapshot!.blocks![0], text: 'Next frame' }] };
  await waitFor(() => expect(area.querySelector('pre')!.textContent).toBe('Next frame'));
  expect(frames.size).toBe(1);
  Object.defineProperty(document, 'hidden', { configurable: true, value: true });
  fireEvent(document, new Event('visibilitychange')); expect(frames.size).toBe(0);
  Object.defineProperty(document, 'hidden', { configurable: true, value: false });
  fireEvent(document, new Event('visibilitychange')); expect(frames.size).toBe(1);
  unmount(); expect(frames.size).toBe(0);
});

it('keeps null context unknown and clamps only numeric remaining values', () => {
  expect(contextRemaining(null)).toBeNull();
  expect(contextRemaining({ percent: null, tokens: null, contextWindow: 1000 })).toBeNull();
  expect(contextRemaining({ percent: 0, tokens: 0, contextWindow: 1000 })).toBe(100);
  expect(contextRemaining({ percent: 100, tokens: 1000, contextWindow: 1000 })).toBe(0);
});

it('immediately saves a remembered String group to the scoped online owner and uses the canonical native thinking', async () => {
  jevIpc.settings = {schema:1,provider:'openrouter',ready:true,keys:[{providerId:'openrouter',status:'verified'}]};
  const group: AutoRouteGroup = {threshold:.85,defaultModel:'model-1',models:[
    {id:'model-1',providerId:'fixture',modelId:'base',thinkingLevel:'off',intent:'Hard tasks.'},
    {id:'model-2',providerId:'fixture',modelId:'next',thinkingLevel:'low',intent:'Easy tasks.'},
  ]};
  vi.mocked(pty.loadProjectAgentDetail).mockResolvedValue({provider:'kota-local',model:'fixture/base',effort:'off',autoRoute:group} as pty.ProjectAgentDetail);
  vi.mocked(startup.localLaunchAction).mockImplementation(async (_root,_id,action) => action === 'configuration'
    ? {model:'fixture/base',thinkingLevel:'off',autoRoute:group} as never : null as never);
  const prior = vi.mocked(rpc.controlLocalWorkbench).getMockImplementation()!;
  vi.mocked(rpc.controlLocalWorkbench).mockImplementation(async (owner,request) => {
    if (request.type !== 'set_model' || request.modelId !== 'auto') return prior(owner,request);
    const saved = request.autoRoute as AutoRouteGroup;
    const canonical = {...saved,models:saved.models.map(model => ({...model,thinkingLevel:'off'}))};
    snapshot = {...snapshot!,revision:snapshot!.revision + 1,model:{provider:'kota',id:'auto'},thinkingLevel:'off',autoRoute:{
      currentModel:null,turn:null,events:[],error:null,
    }};
    return {autoRoute:canonical};
  });
  render(<LocalAgentWorkbench {...props()} />);
  await waitFor(() => expect(screen.getByRole('button',{name:'Local model'})).toBeEnabled());
  await waitFor(() => expect(startup.localLaunchAction).toHaveBeenCalledWith(scope.projectRoot,scope.agentId,'configuration'));
  await userEvent.click(screen.getByRole('button',{name:'Local model'}));
  await userEvent.click(await screen.findByRole('option',{name:/^Auto String/}));
  await waitFor(() => expect(rpc.controlLocalWorkbench).toHaveBeenCalledWith(scope,{type:'set_model',provider:'kota',modelId:'auto',autoRoute:group}));
  expect(pty.loadProjectAgentDetail).toHaveBeenCalledWith({projectRoot:scope.projectRoot,agentId:scope.agentId},{readOnly:true});
  await waitFor(() => expect(screen.getByRole('combobox',{name:'Model 2 thinking'})).toHaveValue('off'));
  expect(screen.getByRole('combobox',{name:'Thinking'})).toBeDisabled();
  expect(screen.getByRole('combobox',{name:'Thinking'})).toHaveDisplayValue('By Auto String');
  expect(screen.queryByRole('button',{name:'Save SHELL'})).not.toBeInTheDocument();
  expect(rpc.setOfflineLocalModel).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Auto String log')).toHaveTextContent('Waiting for an Auto String event…');
});

it.each(['jev_auth','jev_timeout','jev_unreachable','route_empty','route_exhausted'] as const)('shows the controlled %s error in the existing response frame and rereads Jev only for auth rejection', async code => {
  jevIpc.settings = {schema:1,provider:'openrouter',ready:code !== 'jev_auth',keys:[{providerId:'openrouter',status:code === 'jev_auth' ? 'rejected' : 'verified'}]};
  snapshot = {...initial(),model:{provider:'kota',id:'auto'},thinkingLevel:'off',autoRoute:{currentModel:'model-1',turn:null,events:[],error:{code}},
    blocks:[{id:'route-error',kind:'end',label:'Response error',text:'RAW_PROVIDER_ERROR',active:false,error:true,truncated:false}]};
  render(<LocalAgentWorkbench {...props()} />);
  const expected = {
    jev_auth:'Jev authentication failed. Update the key in Puppeteer.',
    jev_timeout:'Jev did not respond within 3 seconds.',
    jev_unreachable:'Jev could not be reached.',
    route_empty:'No models are configured for this Auto String.',
    route_exhausted:'The selected model failed for this turn.',
  }[code];
  expect(await screen.findByText(expected)).toBeVisible();
  expect(within(screen.getByText(expected).closest('.stream-block') as HTMLElement).getByRole('heading')).toHaveTextContent('Response error');
  expect(screen.queryByText('RAW_PROVIDER_ERROR')).not.toBeInTheDocument();
  await waitFor(() => expect(jevIpc.reads).toBe(code === 'jev_auth' ? 2 : 1));
  await act(async () => {snapshot = {...snapshot!,revision:2};});
  expect(jevIpc.reads).toBe(code === 'jev_auth' ? 2 : 1);
  expect(screen.getByRole('combobox',{name:'Thinking'})).toBeDisabled();
});

it('keeps the original provider error with the selected-model failure in the response frame', async () => {
  jevIpc.settings = {schema:1,provider:'openrouter',ready:true,keys:[{providerId:'openrouter',status:'verified'}]};
  snapshot = {...initial(),model:{provider:'kota',id:'auto'},thinkingLevel:'off',autoRoute:{currentModel:'model-1',turn:null,
    events:[{id:1,kind:'model_error',provider:'openai',model:'gpt-6-astra',message:'Connection error.',occurredAt:'2026-10-10T01:50:54Z'}],error:{code:'route_exhausted',eventId:1}},
    blocks:[{id:'route-error',kind:'end',label:'Response error',text:'Connection error.',active:false,error:true,truncated:false}]};
  render(<LocalAgentWorkbench {...props()} />);
  const text = await screen.findByText(/The selected model failed for this turn/);
  expect(text.closest('.stream-block')).toHaveTextContent('Connection error.');
  expect(text.closest('.stream-block')).toHaveTextContent('Response error');
});


it('carries omitted log state only in its Auto String scope and hides it for a concrete model', async () => {
  const route = {currentModel:null,turn:null,events:[],error:null};
  snapshot = {...initial(),model:{provider:'kota',id:'auto'},autoRoute:route};
  const {result} = renderHook(() => useLocalWorkbench(scope.projectRoot,scope.agentId));
  await waitFor(() => expect(result.current.view?.autoRoute).toEqual(route));
  snapshot = {...snapshot!,revision:2}; delete snapshot.autoRoute;
  act(() => result.current.refresh());
  await waitFor(() => expect(result.current.view?.revision).toBe(2));
  expect(result.current.view?.autoRoute).toEqual(route);
  snapshot = {...snapshot!,revision:3,model:{provider:'fixture',id:'base'}};
  act(() => result.current.refresh());
  await waitFor(() => expect(result.current.view?.autoRoute).toBeNull());
  snapshot = {...snapshot!,revision:1,scope:{...scope,epoch:8},model:{provider:'kota',id:'auto'}};
  act(() => result.current.refresh());
  await waitFor(() => expect(result.current.view?.scope.epoch).toBe(8));
  expect(result.current.view?.autoRoute).toBeUndefined();
});

it('does not put a previous run error into Response error when eventId is absent', async () => {
  snapshot = {...initial(),model:{provider:'kota',id:'auto'},autoRoute:{currentModel:null,turn:null,
    events:[{id:1,kind:'model_error',provider:'fixture',model:'base',message:'OLD_FAILURE'}],error:{code:'route_exhausted'}}};
  render(<LocalAgentWorkbench {...props()} />);
  const text = await screen.findByText('The selected model failed for this turn.');
  expect(text.closest('.stream-block')).not.toHaveTextContent('OLD_FAILURE');
  expect(screen.getByLabelText('Auto String log')).toHaveTextContent('OLD_FAILURE');
});

it('shows no log or Jev footer when a concrete model is selected', async () => {
  render(<LocalAgentWorkbench {...props()} />);
  await screen.findByRole('button',{name:'Local model'});
  expect(screen.queryByLabelText('Auto String log')).not.toBeInTheDocument();
  expect(screen.queryByText(/Jev ·/)).not.toBeInTheDocument();
});

it.each(['external', 'control'] as const)('fetches the retained log on %s reentry without a changed block revision', async source => {
  const route = {currentModel:null,turn:null,events:[{id:1,kind:'model_error' as const,provider:'fixture',model:'base',message:'Retained failure'}],error:null};
  vi.mocked(rpc.readLocalWorkbench).mockImplementation(async (_root,_id,cursor) => {
    const value=structuredClone(snapshot!);
    if(cursor?.revision===value.revision) {delete value.blocks;delete value.queue;delete value.autoRoute;}
    return value;
  });
  const {result}=renderHook(()=>useLocalWorkbench(scope.projectRoot,scope.agentId));
  await waitFor(()=>expect(result.current.view?.model?.id).toBe('base'));
  await waitFor(()=>expect(result.current.busy).toBe(false));
  snapshot={...snapshot!,autoRoute:route};
  if (source === 'control') {
    await act(async()=>{await result.current.control({type:'set_model',provider:'kota',modelId:'auto'});});
    expect(result.current.view?.model?.id).toBe('auto');
  } else snapshot={...snapshot!,model:{provider:'kota',id:'auto'}};
  act(()=>result.current.refresh());
  await waitFor(()=>expect(result.current.view?.autoRoute).toEqual(route));
  expect(result.current.view?.revision).toBe(1);
  expect(rpc.readLocalWorkbench).toHaveBeenLastCalledWith(scope.projectRoot,scope.agentId);
});
