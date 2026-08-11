import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest';
import { createAgentGridStore } from '../src/lib/agent-grid-store';
import type { GridSnapshot } from '../src/types/agent-pty';

function snapshot(text: string): GridSnapshot {
  return {
    cols: 1,
    rows: 1,
    cells: [{ ch: text }],
    cursorRow: 0,
    cursorCol: 0,
    cursorVisible: false,
  };
}

describe('AgentGridStore', () => {
  let nextFrameId = 1;
  let frameCallbacks: Map<number, FrameRequestCallback>;
  let requestFrame: MockInstance<typeof window.requestAnimationFrame>;
  let cancelFrame: MockInstance<typeof window.cancelAnimationFrame>;

  beforeEach(() => {
    nextFrameId = 1;
    frameCallbacks = new Map();
    requestFrame = vi.spyOn(window, 'requestAnimationFrame').mockImplementation((callback) => {
      const frameId = nextFrameId++;
      frameCallbacks.set(frameId, callback);
      return frameId;
    });
    cancelFrame = vi.spyOn(window, 'cancelAnimationFrame').mockImplementation((frameId) => {
      frameCallbacks.delete(frameId);
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  const flushFrame = () => {
    const callbacks = Array.from(frameCallbacks.values());
    frameCallbacks.clear();
    for (const callback of callbacks) callback(16);
  };

  it('keeps only the latest hidden snapshot without scheduling a frame', () => {
    const store = createAgentGridStore();
    for (let index = 0; index < 100; index += 1) {
      store.setSnapshot('alice', snapshot(String(index)));
    }

    expect(store.getSnapshot('alice')?.cells[0]?.ch).toBe('99');
    expect(requestFrame).not.toHaveBeenCalled();
  });

  it('coalesces interleaved writes to one notification per visible agent', () => {
    const store = createAgentGridStore();
    const alice = vi.fn();
    const bob = vi.fn();
    store.subscribe('alice', alice);
    store.subscribe('bob', bob);

    store.setSnapshot('alice', snapshot('a1'));
    store.setSnapshot('bob', snapshot('b1'));
    store.setSnapshot('alice', snapshot('a2'));
    store.setSnapshot('bob', snapshot('b2'));
    store.setSnapshot('alice', snapshot('a3'));

    expect(requestFrame).toHaveBeenCalledTimes(1);
    expect(alice).not.toHaveBeenCalled();
    expect(bob).not.toHaveBeenCalled();
    expect(store.getSnapshot('alice')?.cells[0]?.ch).toBe('a3');
    expect(store.getSnapshot('bob')?.cells[0]?.ch).toBe('b2');

    flushFrame();
    expect(alice).toHaveBeenCalledTimes(1);
    expect(bob).toHaveBeenCalledTimes(1);
  });

  it('drops queued work after unsubscribe or dispose', () => {
    const store = createAgentGridStore();
    const listener = vi.fn();
    const unsubscribe = store.subscribe('alice', listener);
    store.setSnapshot('alice', snapshot('queued'));
    const staleFrame = Array.from(frameCallbacks.values())[0];

    unsubscribe();
    expect(cancelFrame).toHaveBeenCalledTimes(1);
    staleFrame?.(16);
    expect(listener).not.toHaveBeenCalled();

    store.subscribe('alice', listener);
    store.setSnapshot('alice', snapshot('new'));
    const disposedFrame = Array.from(frameCallbacks.values())[0];
    store.dispose();
    disposedFrame?.(16);
    expect(listener).not.toHaveBeenCalled();
    expect(store.getSnapshot('alice')).toBeUndefined();
  });

  it('notifies a visible leaf when its snapshot is deleted', () => {
    const store = createAgentGridStore();
    store.setSnapshot('alice', snapshot('live'));
    const listener = vi.fn();
    store.subscribe('alice', listener);

    store.deleteSnapshot('alice');
    expect(store.getSnapshot('alice')).toBeUndefined();
    flushFrame();
    expect(listener).toHaveBeenCalledTimes(1);
  });
});
