import type { LocalStartupOptions } from '../lib/local-startup';
import { useCallback, useRef, type Dispatch, type MutableRefObject, type SetStateAction } from 'react';
import { localSnapshot, onLocalNotice, type LocalScope, type LocalSnapshot, type LocalNotice } from '../lib/local-rpc';
import { spawnAgentPty } from '../pty-client';
import { startLocalWorkbench } from '../lib/local-startup';
import type { AgentId } from '../types/scene';
import type { AgentSpawnRequest, AgentStatusEvent, AgentSummary, AgentWorkStateEvent } from '../types/agent-pty';
import type { AgentGridStore } from '../lib/agent-grid-store';

interface RuntimeState {
  liveAgents: Set<AgentId>;
  status: Map<AgentId, AgentStatusEvent>;
  workState: Map<AgentId, AgentWorkStateEvent>;
}
type Subscription = { generation: number; fns: Array<() => void | Promise<void>> };
interface Context {
  setState: Dispatch<SetStateAction<RuntimeState>>;
  generations: MutableRefObject<Map<AgentId, number>>;
  subscriptions: MutableRefObject<Map<AgentId, Subscription>>;
  clearWorkTimer: (id: AgentId) => void;
  gridStore: AgentGridStore;
}
type StatusNotice = Extract<LocalNotice, { kind: 'status' }>;

/** Parallel to the terminal subscription, sharing only incarnation identity and
 * online/work maps. No terminal grid, silence timeout, keystrokes or ANSI. */
export function useLocalAgentRuntime(context: Context) {
  const contextRef = useRef(context);
  contextRef.current = context;
  const agents = useRef(new Set<string>());
  const watching = useRef(new Map<string, number>());

  const attach = useCallback(async (req: AgentSpawnRequest, generation: number, spawn: boolean, startup?: LocalStartupOptions) => {
    const { generations, subscriptions, setState, clearWorkTimer, gridStore } = contextRef.current;
    const id = req.agentId as AgentId;
    const current = () => generations.current.get(id) === generation;
    agents.current.add(id);
    watching.current.set(id, generation);
    gridStore.deleteSnapshot(id);
    clearWorkTimer(id);
    let epoch = -1;
    let eventSerial = 0;
    const apply = (scope: LocalScope, notice: Pick<StatusNotice, 'state' | 'queued' | 'compacting' | 'error'>) => {
      if (!current() || scope.projectRoot !== req.projectRoot || scope.agentId !== id || scope.epoch < epoch) return;
      epoch = scope.epoch;
      const at = new Date().toISOString();
      setState((s) => {
        if (!current()) return s;
        const liveAgents = new Set(s.liveAgents);
        const status = new Map(s.status);
        const workState = new Map(s.workState);
        const running = notice.state === 'ready' || notice.state === 'busy';
        if (running || notice.state === 'starting') liveAgents.add(id); else liveAgents.delete(id);
        status.set(id, { agentId: id, cli: 'kota-local', cwd: req.cwd, running,
          sessionId: scope.sessionId, forkable: false, detail: notice.error,
          phase: notice.state === 'offline' || notice.state === 'error' ? 'exited' : 'spawned' });
        if (notice.state === 'busy' || notice.compacting) {
          const previous = workState.get(id);
          workState.set(id, { agentId: id, cli: 'kota-local', sessionId: scope.sessionId,
            state: 'working', turnId: `local:${scope.sessionId}:${scope.epoch}`,
            timestamp: at, lastActivityAt: at,
            startedAt: previous?.sessionId === scope.sessionId ? previous.startedAt ?? previous.timestamp : at });
        } else { workState.delete(id); }
        return { liveAgents, status, workState };
      });
    };
    const off = await onLocalNotice(id, (notice) => {
      if (notice.kind !== 'status') return;
      eventSerial += 1;
      apply(notice.scope, notice);
    });
    if (!current()) { off(); return; }
    subscriptions.current.set(id, { generation, fns: [off] });
    try {
      if (spawn) { if (startup) await startLocalWorkbench(req, startup); else await spawnAgentPty(req); }
      const readAt = eventSerial;
      const snapshot: LocalSnapshot = await localSnapshot(id);
      if (readAt === eventSerial) apply(snapshot.scope, { ...snapshot, error: snapshot.error ?? undefined });
    } catch (error) {
      if (current()) {
        apply({ agentId: id, projectRoot: req.projectRoot, sessionId: req.sessionId ?? '', epoch: Math.max(epoch, 0) },
          { state: 'error', queued: 0, compacting: false, error: String(error) });
      }
      if (spawn) throw error;
    }
  }, []);

  const observe = useCallback((summaries: readonly AgentSummary[]) => {
    for (const summary of summaries) {
      const id = summary.agentId as AgentId;
      const { generations, subscriptions } = contextRef.current;
      if (summary.cli !== 'kota-local') {
        // A backend-triggered replacement can finish before the next UI
        // recruit. Retire only our Local subscription; terminal subscriptions
        // and terminal-only incarnations still follow the existing hook.
        if (agents.current.delete(id)) {
          const localGeneration = watching.current.get(id);
          watching.current.delete(id);
          if (localGeneration !== undefined && subscriptions.current.get(id)?.generation === localGeneration) {
            generations.current.set(id, (generations.current.get(id) ?? 0) + 1);
            for (const off of subscriptions.current.get(id)?.fns ?? []) void Promise.resolve(off()).catch(() => {});
            subscriptions.current.delete(id);
            contextRef.current.setState((s) => {
              const status = new Map(s.status); const workState = new Map(s.workState);
              const liveAgents = new Set(s.liveAgents);
              if (status.get(id)?.cli === 'kota-local') { status.delete(id); workState.delete(id); liveAgents.delete(id); }
              return { status, workState, liveAgents };
            });
          }
        }
        continue;
      }
      if (watching.current.get(id) === generations.current.get(id) && subscriptions.current.has(id)) continue;
      const generation = (generations.current.get(id) ?? 0) + 1;
      generations.current.set(id, generation);
      for (const off of subscriptions.current.get(id)?.fns ?? []) void Promise.resolve(off()).catch(() => {});
      subscriptions.current.delete(id);
      // Reserve while listen() is pending so periodic snapshots cannot stack listeners.
      subscriptions.current.set(id, { generation, fns: [] });
      watching.current.set(id, generation);
      void attach({ ...summary, projectId: summary.projectId ?? undefined }, generation, false).catch(() => {
        if (generations.current.get(id) === generation) subscriptions.current.delete(id);
      });
    }
  }, [attach]);

  return {
    recruit: (req: AgentSpawnRequest, generation: number, startup?: LocalStartupOptions) => attach(req, generation, true, startup),
    observe,
    isLocal: (id: string) => agents.current.has(id),
    forget: (id: string) => { agents.current.delete(id); watching.current.delete(id); },
  };
}
