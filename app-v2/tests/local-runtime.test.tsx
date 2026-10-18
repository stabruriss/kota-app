import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { LocalNotice, LocalSnapshot } from '../src/lib/local-rpc';
import type { AgentId } from '../src/types/scene';

const m = vi.hoisted(() => ({
  notices: new Map<string, (event: LocalNotice) => void>(),
  snapshot: vi.fn(), spawn: vi.fn(), close: vi.fn(), output: vi.fn(), status: vi.fn(), work: vi.fn(), exit: vi.fn(),
}));
vi.mock('../src/lib/local-rpc', () => ({
  localSnapshot: m.snapshot,
  onLocalNotice: vi.fn(async (id: string, callback: (event: LocalNotice) => void) => {
    m.notices.set(id, callback);
    return () => { if (m.notices.get(id) === callback) m.notices.delete(id); };
  }),
}));
vi.mock('../src/pty-client', () => ({
  spawnAgentPty: m.spawn, closeAgentPty: m.close, onAgentOutput: m.output,
  onAgentStatus: m.status, onAgentWorkState: m.work, onAgentExit: m.exit,
  submitAgentPromptPty: vi.fn(), writeAgentPty: vi.fn(),
}));
import { useAgentRuntime } from '../src/hooks/useAgentRuntime';

const id = 'agent-fixture-local' as AgentId;
const request = { agentId: id, cli: 'kota-local' as const, cwd: '/fixture/local', projectRoot: '/fixture/project' };
const scope = { agentId: id, projectRoot: request.projectRoot, sessionId: 'fixture-session', epoch: 7 };
const snapshot: LocalSnapshot = { scope, state: 'ready', queued: 0, compacting: false, childPid: 42,
  model: null, thinkingLevel: null, error: null };

beforeEach(() => {
  vi.clearAllMocks(); m.notices.clear(); m.snapshot.mockResolvedValue(snapshot); m.spawn.mockResolvedValue({});
});
afterEach(() => vi.useRealTimers());

it('subscribes without a terminal grid and never times out an authoritative long tool', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date'] });
  const { result, unmount } = renderHook(() => useAgentRuntime());
  await act(async () => { await result.current.recruit(request); });
  expect(result.current.liveAgents.has(id)).toBe(true);
  expect(result.current.gridStore.getSnapshot(id)).toBeUndefined();
  expect(m.output).not.toHaveBeenCalled(); expect(m.status).not.toHaveBeenCalled();
  expect(m.work).not.toHaveBeenCalled(); expect(m.exit).not.toHaveBeenCalled();
  act(() => m.notices.get(id)!({ kind: 'status', scope, state: 'busy', queued: 2, compacting: false }));
  act(() => vi.advanceTimersByTime(6 * 60 * 1000));
  expect(result.current.workState.get(id)?.state).toBe('working');
  expect(result.current.status.get(id)?.cli).toBe('kota-local');
  act(() => m.notices.get(id)!({ kind: 'status', scope, state: 'ready', queued: 0, compacting: false }));
  expect(result.current.workState.has(id)).toBe(false);
  unmount(); expect(m.notices.size).toBe(0);
});

it('attaches to an existing Local process without spawning and fences stale/cross-project events', async () => {
  m.snapshot.mockResolvedValue({ ...snapshot, state: 'busy' });
  const { result, unmount } = renderHook(() => useAgentRuntime());
  act(() => result.current.observeLocal([{ ...request, running: true }]));
  await waitFor(() => expect(result.current.workState.get(id)?.state).toBe('working'));
  expect(m.spawn).not.toHaveBeenCalled();
  const callback = m.notices.get(id)!;
  act(() => callback({ kind: 'status', scope: { ...scope, epoch: 6 }, state: 'ready', queued: 0, compacting: false }));
  act(() => callback({ kind: 'status', scope: { ...scope, projectRoot: '/other' }, state: 'ready', queued: 0, compacting: false }));
  expect(result.current.workState.get(id)?.state).toBe('working');
  act(() => callback({ kind: 'status', scope, state: 'offline', queued: 1, compacting: false }));
  expect(result.current.liveAgents.has(id)).toBe(false);
  expect(result.current.workState.has(id)).toBe(false);
  unmount();
});

it('does no discovery or Local subscription for the six terminal-provider summaries', () => {
  const { result, unmount } = renderHook(() => useAgentRuntime());
  act(() => result.current.observeLocal((['claude', 'codex', 'antigravity', 'opencode', 'pi', 'kimi'] as const)
    .map((cli) => ({ ...request, agentId: `fixture-${cli}`, cli, running: true }))));
  expect(m.snapshot).not.toHaveBeenCalled(); expect(m.notices.size).toBe(0); expect(m.spawn).not.toHaveBeenCalled();
  unmount();
});

it('retires a Local subscription when the registered runtime changes back to a terminal', async () => {
  const { result, unmount } = renderHook(() => useAgentRuntime());
  await act(async () => { await result.current.recruit(request); });
  const old = m.notices.get(id)!;
  act(() => result.current.observeLocal([{ ...request, cli: 'pi', running: true }]));
  expect(result.current.isLocal(id)).toBe(false);
  expect(m.notices.has(id)).toBe(false);
  act(() => old({ kind: 'status', scope, state: 'busy', queued: 0, compacting: false }));
  expect(result.current.status.get(id)?.cli).not.toBe('kota-local');
  expect(result.current.workState.has(id)).toBe(false);
  expect(m.spawn).toHaveBeenCalledTimes(1);
  unmount();
});
