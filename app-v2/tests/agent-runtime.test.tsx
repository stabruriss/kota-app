import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AgentWorkStateEvent } from '../src/types/agent-pty';

const mocks = vi.hoisted(() => {
  const exitListeners = new Map<string, (event: { agentId: string; code: number | null }) => void>();
  const workListeners = new Map<string, (event: AgentWorkStateEvent) => void>();
  return {
    exitListeners,
    workListeners,
    closeAgentPty: vi.fn(),
    onAgentOutput: vi.fn(async () => async () => {}),
    onAgentExit: vi.fn(async (
      agentId: string,
      listener: (event: { agentId: string; code: number | null }) => void,
    ) => {
      exitListeners.set(agentId, listener);
      return async () => exitListeners.delete(agentId);
    }),
    onAgentStatus: vi.fn(async () => async () => {}),
    onAgentWorkState: vi.fn(async (
      agentId: string,
      listener: (event: AgentWorkStateEvent) => void,
    ) => {
      workListeners.set(agentId, listener);
      return async () => workListeners.delete(agentId);
    }),
    spawnAgentPty: vi.fn(),
    submitAgentPromptPty: vi.fn(),
    writeAgentPty: vi.fn(),
  };
});

vi.mock('../src/pty-client', () => ({
  closeAgentPty: mocks.closeAgentPty,
  onAgentOutput: mocks.onAgentOutput,
  onAgentExit: mocks.onAgentExit,
  onAgentStatus: mocks.onAgentStatus,
  onAgentWorkState: mocks.onAgentWorkState,
  spawnAgentPty: mocks.spawnAgentPty,
  submitAgentPromptPty: mocks.submitAgentPromptPty,
  writeAgentPty: mocks.writeAgentPty,
}));

import { useAgentRuntime } from '../src/hooks/useAgentRuntime';
import type { AgentSpawnRequest } from '../src/types/agent-pty';

describe('useAgentRuntime work and exit handling', () => {
  beforeEach(() => {
    mocks.exitListeners.clear();
    mocks.workListeners.clear();
    vi.clearAllMocks();
    mocks.spawnAgentPty.mockResolvedValue({
      agentId: 'alice',
      outputEvent: 'pty://agent/alice/output',
      exitEvent: 'pty://agent/alice/exit',
      statusEvent: 'pty://agent/alice/status',
      workEvent: 'pty://agent/alice/work',
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('reports the definitive PTY exit with its original project context', async () => {
    const onExit = vi.fn(async () => {});
    const request: AgentSpawnRequest = {
      agentId: 'alice',
      cli: 'codex',
      cwd: '/tmp/kota/project/.agent-workspaces/alice',
      projectRoot: '/tmp/kota/project',
    };
    const view = renderHook(() => useAgentRuntime({ onExit }));

    await act(async () => {
      await view.result.current.recruit(request);
    });
    expect(view.result.current.liveAgents.has('alice')).toBe(true);
    act(() => {
      mocks.workListeners.get('alice')?.({
        agentId: 'alice',
        state: 'working',
        timestamp: new Date().toISOString(),
        cli: 'codex',
        sessionId: 'session-1',
        turnId: 'turn-1',
        reason: 'task_started',
      });
    });
    expect(view.result.current.workState.has('alice')).toBe(true);

    await act(async () => {
      mocks.exitListeners.get('alice')?.({ agentId: 'alice', code: 0 });
      await Promise.resolve();
    });

    expect(view.result.current.liveAgents.has('alice')).toBe(false);
    expect(view.result.current.workState.has('alice')).toBe(false);
    expect(onExit).toHaveBeenCalledTimes(1);
    expect(onExit).toHaveBeenCalledWith({
      event: { agentId: 'alice', code: 0 },
      request,
    });
    view.unmount();
  });

  it.each(['codex', 'kimi'] as const)(
    'keeps an active %s turn working past the inactivity timeout until its terminal event',
    async (cli) => {
      vi.useFakeTimers();
      vi.setSystemTime('2026-07-22T10:00:00.000Z');
      const request: AgentSpawnRequest = {
        agentId: 'alice',
        cli,
        cwd: '/tmp/kota/project/.agent-workspaces/alice',
        projectRoot: '/tmp/kota/project',
      };
      const view = renderHook(() => useAgentRuntime());

      await act(async () => {
        await view.result.current.recruit(request);
      });
      const baselineTimerCount = vi.getTimerCount();
      act(() => {
        const listener = mocks.workListeners.get('alice');
        listener?.({
          agentId: 'alice',
          state: 'working',
          timestamp: '2026-07-22T10:00:00.000Z',
          cli,
          sessionId: 'session-1',
          reason: 'prompt-submitted',
        });
      });
      expect(vi.getTimerCount()).toBe(baselineTimerCount + 1);

      act(() => {
        mocks.workListeners.get('alice')?.({
          agentId: 'alice',
          state: 'working',
          timestamp: '2026-07-22T10:00:01.000Z',
          cli,
          sessionId: 'session-1',
          turnId: 'turn-1',
          reason: cli === 'codex' ? 'task_started' : 'step.begin',
        });
      });
      expect(vi.getTimerCount()).toBe(baselineTimerCount);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(6 * 60 * 1000);
      });
      expect(view.result.current.workState.get('alice')?.turnId).toBe('turn-1');

      act(() => {
        mocks.workListeners.get('alice')?.({
          agentId: 'alice',
          state: 'idle',
          timestamp: '2026-07-22T10:06:01.000Z',
          cli,
          sessionId: 'session-1',
          turnId: 'turn-1',
          reason: cli === 'codex' ? 'task_complete' : 'end_turn',
        });
      });
      expect(view.result.current.workState.has('alice')).toBe(false);
      view.unmount();
    },
  );

  it('keeps the active Kimi turn id across lifecycle-scoped events that omit it', async () => {
    vi.useFakeTimers();
    vi.setSystemTime('2026-07-22T10:00:00.000Z');
    const request: AgentSpawnRequest = {
      agentId: 'alice',
      cli: 'kimi',
      cwd: '/tmp/kota/project/.agent-workspaces/alice',
      projectRoot: '/tmp/kota/project',
    };
    const view = renderHook(() => useAgentRuntime());

    await act(async () => {
      await view.result.current.recruit(request);
    });
    act(() => {
      const listener = mocks.workListeners.get('alice');
      listener?.({
        agentId: 'alice',
        state: 'working',
        timestamp: '2026-07-22T10:00:00.000Z',
        cli: 'kimi',
        sessionId: 'session-1',
        turnId: 'turn-7',
        reason: 'step.begin',
      });
      listener?.({
        agentId: 'alice',
        state: 'working',
        timestamp: '2026-07-22T10:01:00.000Z',
        cli: 'kimi',
        sessionId: 'session-1',
        reason: 'content.part',
      });
    });

    expect(view.result.current.workState.get('alice')).toMatchObject({
      turnId: 'turn-7',
      startedAt: '2026-07-22T10:00:00.000Z',
      lastActivityAt: '2026-07-22T10:01:00.000Z',
    });
    act(() => {
      mocks.workListeners.get('alice')?.({
        agentId: 'alice',
        state: 'working',
        timestamp: '2026-07-22T10:02:00.000Z',
        cli: 'kimi',
        sessionId: 'session-1',
        turnId: 'turn-8',
        reason: 'step.begin',
      });
    });
    expect(view.result.current.workState.get('alice')).toMatchObject({
      turnId: 'turn-8',
      startedAt: '2026-07-22T10:02:00.000Z',
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6 * 60 * 1000);
    });
    expect(view.result.current.workState.get('alice')?.turnId).toBe('turn-8');
    view.unmount();
  });

  it('matches authoritative terminal events by turn even when timestamps are close or stale', async () => {
    vi.useFakeTimers();
    vi.setSystemTime('2026-07-22T10:00:00.000Z');
    const request: AgentSpawnRequest = {
      agentId: 'alice',
      cli: 'codex',
      cwd: '/tmp/kota/project/.agent-workspaces/alice',
      projectRoot: '/tmp/kota/project',
    };
    const view = renderHook(() => useAgentRuntime());

    await act(async () => {
      await view.result.current.recruit(request);
    });
    act(() => {
      const listener = mocks.workListeners.get('alice');
      listener?.({
        agentId: 'alice',
        state: 'working',
        timestamp: '2026-07-22T10:00:00.000Z',
        cli: 'codex',
        sessionId: 'session-1',
        turnId: 'turn-2',
        reason: 'task_started',
      });
      listener?.({
        agentId: 'alice',
        state: 'idle',
        timestamp: '2026-07-22T10:00:00.500Z',
        cli: 'codex',
        sessionId: 'session-1',
        turnId: 'turn-1',
        reason: 'task_complete',
      });
    });

    expect(view.result.current.workState.get('alice')?.turnId).toBe('turn-2');
    act(() => {
      mocks.workListeners.get('alice')?.({
        agentId: 'alice',
        state: 'idle',
        timestamp: '2026-07-22T09:59:58.000Z',
        cli: 'codex',
        sessionId: 'session-1',
        turnId: 'turn-2',
        reason: 'task_complete',
      });
    });
    expect(view.result.current.workState.has('alice')).toBe(false);
    view.unmount();
  });

  it('does not revive stale authoritative history or let its replay clear a live turn', async () => {
    vi.useFakeTimers();
    vi.setSystemTime('2026-07-22T10:00:00.000Z');
    const request: AgentSpawnRequest = {
      agentId: 'alice',
      cli: 'codex',
      cwd: '/tmp/kota/project/.agent-workspaces/alice',
      projectRoot: '/tmp/kota/project',
    };
    const view = renderHook(() => useAgentRuntime());

    await act(async () => {
      await view.result.current.recruit(request);
    });
    act(() => {
      mocks.workListeners.get('alice')?.({
        agentId: 'alice',
        state: 'working',
        timestamp: '2026-07-19T10:00:00.000Z',
        cli: 'codex',
        sessionId: 'dead-session',
        turnId: 'stale-turn',
        reason: 'task_started',
      });
    });
    expect(view.result.current.workState.has('alice')).toBe(false);

    const liveTurn = {
      agentId: 'alice',
      state: 'working' as const,
      timestamp: '2026-07-22T10:00:00.000Z',
      cli: 'codex' as const,
      sessionId: 'live-session',
      turnId: 'live-turn',
      reason: 'task_started',
    };
    act(() => {
      mocks.workListeners.get('alice')?.(liveTurn);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6 * 60 * 1000);
    });
    act(() => {
      mocks.workListeners.get('alice')?.(liveTurn);
    });
    expect(view.result.current.workState.get('alice')?.turnId).toBe('live-turn');
    view.unmount();
  });

  it.each([
    { cli: 'claude', turnId: 'request-1' },
    { cli: 'codex', turnId: null },
    { cli: 'kimi', turnId: null },
  ] as const)(
    'keeps the inactivity timeout without authoritative lifecycle evidence ($cli)',
    async ({ cli, turnId }) => {
      vi.useFakeTimers();
      vi.setSystemTime('2026-07-22T10:00:00.000Z');
      const request: AgentSpawnRequest = {
        agentId: 'alice',
        cli,
        cwd: '/tmp/kota/project/.agent-workspaces/alice',
        projectRoot: '/tmp/kota/project',
      };
      const view = renderHook(() => useAgentRuntime());

      await act(async () => {
        await view.result.current.recruit(request);
      });
      act(() => {
        mocks.workListeners.get('alice')?.({
          agentId: 'alice',
          state: 'working',
          timestamp: '2026-07-22T10:00:00.000Z',
          cli,
          sessionId: 'session-1',
          turnId,
          reason: 'assistant_activity',
        });
      });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(5 * 60 * 1000);
      });
      expect(view.result.current.workState.has('alice')).toBe(false);
      view.unmount();
    },
  );
});
