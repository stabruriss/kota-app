import type { GridSnapshot } from '../types/agent-pty';
import type { AgentId } from '../types/scene';

type AgentGridListener = () => void;

export interface AgentGridStore {
  getSnapshot: (agentId: AgentId) => GridSnapshot | undefined;
  subscribe: (agentId: AgentId, listener: AgentGridListener) => () => void;
  setSnapshot: (agentId: AgentId, snapshot: GridSnapshot) => void;
  deleteSnapshot: (agentId: AgentId) => void;
  dispose: () => void;
}

/**
 * Per-agent latest-only terminal frames. Writes replace the current snapshot
 * immediately so a newly mounted terminal can read the newest frame without
 * waiting for an animation frame. Visible subscribers are notified at most
 * once per browser frame; hidden agents have no subscribers and schedule no
 * renderer work.
 */
export function createAgentGridStore(): AgentGridStore {
  const snapshots = new Map<AgentId, GridSnapshot>();
  const listeners = new Map<AgentId, Set<AgentGridListener>>();
  const dirtyAgentIds = new Set<AgentId>();
  let frameId: number | null = null;

  const cancelFrameIfIdle = () => {
    if (frameId == null || dirtyAgentIds.size > 0) return;
    window.cancelAnimationFrame(frameId);
    frameId = null;
  };

  const scheduleNotification = (agentId: AgentId) => {
    if ((listeners.get(agentId)?.size ?? 0) > 0) {
      dirtyAgentIds.add(agentId);
    }
    if (dirtyAgentIds.size === 0 || frameId != null) return;
    frameId = window.requestAnimationFrame(() => {
      frameId = null;
      const changedAgentIds = Array.from(dirtyAgentIds);
      dirtyAgentIds.clear();
      for (const changedAgentId of changedAgentIds) {
        for (const listener of Array.from(listeners.get(changedAgentId) ?? [])) {
          listener();
        }
      }
    });
  };

  return {
    getSnapshot(agentId) {
      return snapshots.get(agentId);
    },
    subscribe(agentId, listener) {
      let agentListeners = listeners.get(agentId);
      if (!agentListeners) {
        agentListeners = new Set();
        listeners.set(agentId, agentListeners);
      }
      agentListeners.add(listener);
      return () => {
        const current = listeners.get(agentId);
        if (!current) return;
        current.delete(listener);
        if (current.size > 0) return;
        listeners.delete(agentId);
        dirtyAgentIds.delete(agentId);
        cancelFrameIfIdle();
      };
    },
    setSnapshot(agentId, snapshot) {
      snapshots.set(agentId, snapshot);
      scheduleNotification(agentId);
    },
    deleteSnapshot(agentId) {
      if (!snapshots.delete(agentId)) return;
      scheduleNotification(agentId);
    },
    dispose() {
      if (frameId != null) window.cancelAnimationFrame(frameId);
      frameId = null;
      dirtyAgentIds.clear();
      listeners.clear();
      snapshots.clear();
    },
  };
}
