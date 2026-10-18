import { act, cleanup, fireEvent, render, renderHook, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { LocalAgentWorkbench, contextReading } from '../src/chrome/LocalAgentWorkbench';
import { useLocalWorkbench } from '../src/hooks/useLocalWorkbench';
import * as rpc from '../src/lib/local-rpc';
import * as auth from '../src/lib/local-auth';
import * as startup from '../src/lib/local-startup';
import type { LocalContext, LocalWorkbenchView } from '../src/lib/local-rpc';

const scope = { projectRoot: '/fixture/room', agentId: 'agent-copper', sessionId: 'fixture-session', epoch: 7 };
const unknownUsage = (contextWindow = 1000): LocalContext => ({ tokens: null, percent: null, contextWindow });
const initial = (): LocalWorkbenchView => ({ scope, state: 'busy', compacting: false, contextEstimate: null,
  childPid: 42, queued: 0, model: { provider: 'fixture', id: 'base' }, thinkingLevel: 'off', error: null,
  revision: 1, runId: 1, startedAt: null, phase: 'Output', statsRevision: 0, dropped: 0,
  queueTotal: 0, queueRevision: 0, blocks: [], queue: [] });
const props = () => ({ ...scope, agent: { name: 'Copper', emoji: '', role: '', hue: '', avatarId: null, provider: 'kota-local' },
  agents: {}, focusRequest: 1, onClose: vi.fn(), onDetails: vi.fn(), onFresh: vi.fn(), onResume: vi.fn().mockResolvedValue(undefined) });
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}
let snapshot: LocalWorkbenchView;
let usage: LocalContext | undefined;
let calls: string[];
function response(type: string) {
  switch (type) {
    case 'get_state': return { model: snapshot.model, thinkingLevel: snapshot.thinkingLevel,
      isStreaming: snapshot.state === 'busy', isCompacting: snapshot.compacting, autoCompactionEnabled: true };
    case 'get_available_models': return { models: [{ provider: 'fixture', id: 'base' }, { provider: 'fixture', id: 'next' }] };
    case 'get_available_thinking_levels': return { levels: ['off'] };
    case 'get_session_stats': return { contextUsage: usage };
    default: throw new Error(`Unexpected control in context test: ${type}`);
  }
}
const tick = async (ms = 0) => { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); };
const area = () => screen.getByRole('complementary', { name: 'Context remaining' });
const number = () => area().querySelector('.context-number')!.textContent;
beforeEach(() => {
  vi.restoreAllMocks(); vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] });
  snapshot = initial(); usage = { tokens: 320, percent: 32, contextWindow: 1000 }; calls = [];
  Object.defineProperty(document, 'hidden', { configurable: true, value: false });
  vi.spyOn(rpc, 'readLocalWorkbench').mockImplementation(async () => structuredClone(snapshot));
  vi.spyOn(rpc, 'readLocalAuth').mockResolvedValue({ provider: 'fixture', status: 'ready', authType: null });
  vi.spyOn(startup, 'onLocalStartup').mockResolvedValue(() => {});
  vi.spyOn(startup, 'localLaunchAction').mockImplementation(async (_root, _id, action) =>
    (action === 'configuration' ? { model: 'fixture/base', thinkingLevel: 'off' } : null) as never);
  vi.spyOn(auth, 'readLocalAuthAvailability').mockResolvedValue({ schema: 1, summary: 'ready', providers: [{ providerId: 'fixture', status: 'signed_in', authType: 'api_key' }] });
  vi.spyOn(auth, 'readLocalModelCatalog').mockResolvedValue({ schema: 1, providers: [{ providerId: 'fixture', linked: true, models: [{ modelId: 'base', name: 'Base', thinkingLevels: ['off'] }] }] });
  vi.spyOn(rpc, 'controlLocalWorkbench').mockImplementation(async (_scope, request) => {
    calls.push(String(request.type)); return response(String(request.type));
  });
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); });

it('uses canonical usage ahead of an estimate and removes the automatic-compaction hover', async () => {
  snapshot.contextEstimate = 400;
  render(<LocalAgentWorkbench {...props()} />); await tick();
  expect(number()).toBe('68%');
  expect(screen.getByRole('meter')).toHaveAttribute('aria-valuenow', '68');
  expect(area()).not.toHaveAttribute('title');
  expect(screen.queryByTitle('Pi manages automatic compaction.')).not.toBeInTheDocument();
  expect(calls.every((type) => ['get_state', 'get_available_models', 'get_available_thinking_levels', 'get_session_stats'].includes(type))).toBe(true);
});

it('shows Compacting only for a live Pi flag, even without a streaming assistant', async () => {
  snapshot = { ...snapshot, state: 'ready', compacting: true, contextEstimate: 400 };
  render(<LocalAgentWorkbench {...props()} />); await tick();
  expect(within(area()).getByText('Compacting')).toBeInTheDocument();
  expect(screen.getByText('Compacting context.')).toBeInTheDocument();
  expect(screen.getByRole('meter')).toHaveAttribute('aria-valuetext', 'Compacting');
  expect(screen.getByRole('meter')).not.toHaveAttribute('aria-valuenow');
  expect(area().querySelector('.context-number')).toBeNull();
  expect(area().querySelector('.remaining')).toBeNull();
  expect(area().querySelector('.context-tokens')).toBeNull();
  expect(within(area()).getByText('Compacting')).toHaveClass('compacting-caption');
  expect(screen.getByRole('meter')).toHaveClass('compacting');
  expect(screen.getByRole('button', { name: 'Stop' })).toBeInTheDocument();
});

it('displays Pi successful-end estimates with ~, an explicit tooltip and approximate aria text', async () => {
  usage = unknownUsage(); snapshot.contextEstimate = 400;
  render(<LocalAgentWorkbench {...props()} />); await tick();
  expect(number()).toBe('~60%');
  expect(within(area()).getByText('~600 / 1,000')).toBeInTheDocument();
  expect(screen.getByRole('meter')).toHaveAttribute('aria-valuetext', 'Approximately 60% remaining (estimated after compaction)');
  expect(area().getAttribute('title')).toContain('Estimated from Pi');
  expect(screen.queryByText('Compacting')).not.toBeInTheDocument();
  expect(screen.queryByText('Waiting for usage')).not.toBeInTheDocument();
});

it('replaces a successful-end estimate when fresh usage returns', async () => {
  usage = unknownUsage(); snapshot.contextEstimate = 400;
  render(<LocalAgentWorkbench {...props()} />); await tick();
  expect(number()).toBe('~60%');
  usage = { tokens: 160, percent: 16, contextWindow: 1000 };
  snapshot = { ...snapshot, revision: 2, statsRevision: 1 };
  await tick(100);
  expect(number()).toBe('84%');
  expect(screen.getByRole('meter')).toHaveAttribute('aria-valuetext', '84% remaining');
  expect(area()).not.toHaveAttribute('title');
});

it('handles start → successful end → settled without inferring compaction from null usage', async () => {
  render(<LocalAgentWorkbench {...props()} />); await tick();
  expect(number()).toBe('68%');
  snapshot = { ...snapshot, revision: 2, compacting: true, phase: 'Compacting' };
  await tick(100);
  expect(within(area()).getByText('Compacting')).toBeInTheDocument();
  usage = unknownUsage();
  snapshot = { ...snapshot, revision: 3, compacting: false, contextEstimate: 400, statsRevision: 1, phase: 'Working' };
  await tick(100);
  expect(number()).toBe('~60%');
  snapshot = { ...snapshot, revision: 4, state: 'ready', statsRevision: 2, phase: '' };
  await tick(100);
  expect(number()).toBe('~60%');
  expect(screen.getByText('Ready when you are.')).toBeInTheDocument();
});

it.each(['Retrying', 'Working', 'Compacting'])('does not call %s + unknown usage real compaction', async (phase) => {
  usage = unknownUsage(); snapshot.phase = phase;
  render(<LocalAgentWorkbench {...props()} />); await tick();
  expect(within(area()).getByText('Usage unavailable')).toBeInTheDocument();
  expect(screen.queryByText('Compacting')).not.toBeInTheDocument();
  expect(screen.queryByText('Compacting context.')).not.toBeInTheDocument();
  expect(number()).toBe('—');
});

it('keeps missing window/model neutral rather than filling 100% or an estimate', async () => {
  snapshot.model = null; snapshot.contextEstimate = 400; usage = undefined;
  render(<LocalAgentWorkbench {...props()} />); await tick();
  expect(within(area()).getByText('Usage unavailable')).toBeInTheDocument();
  expect(number()).toBe('—');
  expect(screen.getByRole('meter')).not.toHaveAttribute('aria-valuenow');
});

it('does not turn a stats command failure into Compacting or estimated usage', async () => {
  snapshot.contextEstimate = 400;
  vi.mocked(rpc.controlLocalWorkbench).mockImplementation(async (_scope, request) => {
    if (request.type === 'get_session_stats') throw new Error('Synthetic stats failure');
    return response(String(request.type));
  });
  render(<LocalAgentWorkbench {...props()} />); await tick();
  expect(screen.getByRole('status')).toHaveTextContent('Synthetic stats failure');
  expect(within(area()).getByText('Usage unavailable')).toBeInTheDocument();
  expect(screen.queryByText('Compacting')).not.toBeInTheDocument();
  expect(number()).toBe('—');
});

it('does not display a stale compacting flag or meter after the child fails', async () => {
  render(<LocalAgentWorkbench {...props()} />); await tick();
  expect(number()).toBe('68%');
  snapshot = { ...snapshot, revision: 2, state: 'error', compacting: true, error: 'Synthetic child exit' };
  await tick(100);
  // The new sign-in hint has its own status; the child failure must still be shown.
  expect(screen.getByText('Synthetic child exit', { exact: true })).toHaveAttribute('role', 'status');
  expect(screen.getByText('Session is offline.')).toBeInTheDocument();
  expect(screen.queryByText('Compacting')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Stop' })).not.toBeInTheDocument();
  expect(number()).toBe('—');
});

it('refreshes on same-scope model change and never mixes the previous window with the new model', async () => {
  render(<LocalAgentWorkbench {...props()} />); await tick();
  const pending = deferred<{ contextUsage: LocalContext }>();
  vi.mocked(rpc.controlLocalWorkbench).mockImplementation(async (_scope, request) =>
    request.type === 'get_session_stats' ? pending.promise : response(String(request.type)));
  snapshot = { ...snapshot, revision: 2, model: { provider: 'fixture', id: 'next' }, contextEstimate: 400 };
  await tick(100);
  expect(screen.getByRole('button', {name:'Local model'})).toHaveTextContent('fixture/next');
  expect(number()).toBe('—');
  await act(async () => pending.resolve({ contextUsage: unknownUsage(2000) }));
  expect(number()).toBe('~80%');
});

it('does not roll back a model from a late get_state response', async () => {
  const old = response('get_state');
  const pending = deferred<unknown>();
  vi.mocked(rpc.controlLocalWorkbench).mockImplementationOnce(() => pending.promise);
  const { result } = renderHook(() => useLocalWorkbench(scope.projectRoot, scope.agentId)); await tick();
  snapshot = { ...snapshot, revision: 2, model: { provider: 'fixture', id: 'next' } };
  usage = unknownUsage(2000);
  await tick(100);
  await act(async () => pending.resolve(old));
  expect(result.current.view?.model?.id).toBe('next');
  expect(result.current.info.context).toBeNull();
  await tick(100);
  expect(result.current.info.context?.contextWindow).toBe(2000);
});

it('does not resurrect a pre-compaction count from a late stats reply', async () => {
  const pending = deferred<{ contextUsage: LocalContext }>();
  let reads = 0;
  vi.mocked(rpc.controlLocalWorkbench).mockImplementation(async (_scope, request) =>
    request.type === 'get_session_stats' && ++reads === 1 ? pending.promise : response(String(request.type)));
  render(<LocalAgentWorkbench {...props()} />); await tick();
  usage = unknownUsage();
  snapshot = { ...snapshot, revision: 2, statsRevision: 1, contextEstimate: 400 };
  await tick(100);
  await act(async () => pending.resolve({ contextUsage: { tokens: 320, percent: 32, contextWindow: 1000 } }));
  expect(number()).toBe('—');
  await tick(100);
  expect(number()).toBe('~60%');
});

it('fences a late old-owner stats response and does not carry its estimate across scope', async () => {
  const pending = deferred<{ contextUsage: LocalContext }>();
  let reads = 0;
  vi.mocked(rpc.controlLocalWorkbench).mockImplementation(async (_scope, request) =>
    request.type === 'get_session_stats' && ++reads === 1 ? pending.promise : response(String(request.type)));
  snapshot.contextEstimate = 400;
  const { result } = renderHook(() => useLocalWorkbench(scope.projectRoot, scope.agentId)); await tick();
  snapshot = { ...snapshot, scope: { ...scope, epoch: 8, sessionId: 'next-session' }, revision: 0, contextEstimate: null };
  usage = unknownUsage(2000);
  await tick(100);
  await act(async () => pending.resolve({ contextUsage: unknownUsage() }));
  expect(result.current.info.context).toBeNull();
  expect(result.current.view?.contextEstimate).toBeNull();
  await tick(100);
  expect(result.current.info.context?.contextWindow).toBe(2000);
});

it('ignores hidden metadata replies and makes no hidden context requests', async () => {
  const pending = deferred<{ contextUsage: LocalContext }>();
  vi.mocked(rpc.controlLocalWorkbench).mockImplementation(async (_scope, request) =>
    request.type === 'get_session_stats' ? pending.promise : response(String(request.type)));
  const { result } = renderHook(() => useLocalWorkbench(scope.projectRoot, scope.agentId)); await tick();
  const count = vi.mocked(rpc.controlLocalWorkbench).mock.calls.length;
  Object.defineProperty(document, 'hidden', { configurable: true, value: true });
  fireEvent(document, new Event('visibilitychange'));
  await act(async () => pending.resolve({ contextUsage: unknownUsage() })); await tick(5000);
  expect(result.current.info.context).toBeNull();
  expect(rpc.controlLocalWorkbench).toHaveBeenCalledTimes(count);
});

it.each([undefined, null, -1, NaN, Infinity])('leaves unknown usage unknown for invalid estimate %s', (estimate) => {
  expect(contextReading(unknownUsage(), estimate)).toEqual({ remaining: null, estimated: false });
});
it('requires a real current window and explicit null usage before estimating; valid zero stays valid', () => {
  expect(contextReading(null, 400)).toEqual({ remaining: null, estimated: false });
  expect(contextReading(unknownUsage(0), 400)).toEqual({ remaining: null, estimated: false });
  expect(contextReading(unknownUsage(NaN), 400)).toEqual({ remaining: null, estimated: false });
  expect(contextReading({ tokens: 100, percent: NaN, contextWindow: 1000 }, 400)).toEqual({ remaining: null, estimated: false });
  expect(contextReading(unknownUsage(), 0)).toEqual({ remaining: 100, estimated: true });
  expect(contextReading(unknownUsage(), 1200)).toEqual({ remaining: 0, estimated: true });
  expect(contextReading({ tokens: 0, percent: 0, contextWindow: 1000 }, 400)).toEqual({ remaining: 100, estimated: false });
});
