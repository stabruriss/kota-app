import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { watchDreamSubmissions } from '../src/lib/ember-dream-run';
import type { EmberDreamRunStatus } from '../src/pty-client';

const participants = ['agent-a', 'agent-b'].map((agentId) => ({
  agentId, projectRoot: '/tmp/dream-fixture', deliveredAt: '2026-01-01T00:00:00.000Z',
}));
const statuses = (submitted: boolean, working: boolean): EmberDreamRunStatus[] => (
  participants.map(({ agentId, projectRoot }) => ({ agentId, projectRoot, submitted, working }))
);

describe('Dream submission deadlines', () => {
  beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(new Date(participants[0]!.deliveredAt)); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); });

  it('consolidates as soon as everyone submitted, even while mark maintenance is busy', async () => {
    let state = statuses(false, true);
    const onReady = vi.fn();
    watchDreamSubmissions({ startedAt: Date.now(), participants, readStatus: async () => state, onReady });
    await vi.advanceTimersByTimeAsync(5_000);
    expect(onReady).not.toHaveBeenCalled();
    state = statuses(true, true);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(onReady).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(300_000);
    expect(onReady).toHaveBeenCalledTimes(1);
  });

  it('collects at 120 seconds if only the unsubmitted agents are idle', async () => {
    const state = statuses(false, false);
    state[0] = { ...state[0]!, submitted: true, working: true };
    const onReady = vi.fn();
    watchDreamSubmissions({ startedAt: Date.now(), participants, readStatus: async () => state, onReady });
    await vi.advanceTimersByTimeAsync(119_999);
    expect(onReady).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(onReady).toHaveBeenCalledTimes(1);
  });

  it('defers working agents at 120 seconds to 300, even if they later become idle', async () => {
    let state = statuses(false, true);
    const onReady = vi.fn();
    watchDreamSubmissions({ startedAt: Date.now(), participants, readStatus: async () => state, onReady });
    await vi.advanceTimersByTimeAsync(120_000);
    state = statuses(false, false);
    await vi.advanceTimersByTimeAsync(179_999);
    expect(onReady).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(onReady).toHaveBeenCalledTimes(1);
  });

  it('still finishes early when the deferred agents submit', async () => {
    let state = statuses(false, true);
    const onReady = vi.fn();
    watchDreamSubmissions({ startedAt: Date.now(), participants, readStatus: async () => state, onReady });
    await vi.advanceTimersByTimeAsync(121_000);
    state = statuses(true, false);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(onReady).toHaveBeenCalledTimes(1);
  });

  it.each(['missing', 'rejected', 'stalled'] as const)('bounds an unavailable status (%s) at 300 seconds', async (mode) => {
    const onReady = vi.fn();
    const readStatus = vi.fn(async (): Promise<EmberDreamRunStatus[]> => {
      if (mode === 'rejected') throw new Error('fixture read failure');
      if (mode === 'stalled') return new Promise(() => {});
      return [];
    });
    watchDreamSubmissions({ startedAt: Date.now(), participants, readStatus, onReady });
    await vi.advanceTimersByTimeAsync(299_999);
    expect(onReady).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(onReady).toHaveBeenCalledTimes(1);
    if (mode === 'stalled') expect(readStatus).toHaveBeenCalledTimes(1);
  });

  it('ignores late IPC completion after cancellation', async () => {
    let resolve!: (value: EmberDreamRunStatus[]) => void;
    const onReady = vi.fn();
    const stop = watchDreamSubmissions({ startedAt: Date.now(), participants,
      readStatus: () => new Promise((done) => { resolve = done; }), onReady });
    stop();
    resolve(statuses(true, false));
    await vi.advanceTimersByTimeAsync(300_000);
    expect(onReady).not.toHaveBeenCalled();
  });
});
