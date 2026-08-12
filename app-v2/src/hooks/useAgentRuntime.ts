/** useAgentRuntime — M6.A frontend bookkeeping for live agent PTYs.
 *
 *  Maintains:
 *    - `liveAgents`: which agentIds have a running PTY
 *    - `gridStore`: per-agent latest GridSnapshot (subscribed by visible terminal leaves)
 *    - `recruit(req)`: spawn a CLI in `pty/agent.rs`, subscribe to events
 *    - `send(agentId, input)`: write to that agent's stdin
 *    - `dismiss(agentId)`: kill the PTY
 *
 *  Per ARCHITECTURE-INVARIANTS:
 *    I-4 PTY = real terminal, alacritty grid render
 *    I-5 zero protocol parsing — backend's `GridSnapshot` is the SoT
 *    I-6 InputBar → write to selected agent's stdin                       */

import { useCallback, useEffect, useRef, useState } from 'react';
import {
  closeAgentPty,
  onAgentExit as subscribeAgentExit,
  onAgentOutput,
  onAgentStatus,
  onAgentWorkState,
  spawnAgentPty,
  submitAgentPromptPty,
  writeAgentPty,
} from '../pty-client';
import {
  linesToGridSnapshot,
  type AgentSpawnRequest,
  type AgentExitEvent,
  type AgentStatusEvent,
  type AgentWorkStateEvent,
  type GridSnapshot,
} from '../types/agent-pty';
import type { AgentId } from '../types/scene';
import {
  createAgentGridStore,
  type AgentGridStore,
} from '../lib/agent-grid-store';

const AGENT_WORK_INACTIVITY_TIMEOUT_MS = 5 * 60 * 1000;
const AGENT_WORK_PRUNE_INTERVAL_MS = 30 * 1000;

function normalizedWorkTurnId(event: AgentWorkStateEvent | undefined): string | null {
  const turnId = event?.turnId?.trim();
  return turnId || null;
}

// These projections provide an explicit lifecycle and a stable ID for the whole turn.
function hasAuthoritativeActiveTurn(event: AgentWorkStateEvent | undefined): boolean {
  return (
    (event?.cli === 'codex' || event?.cli === 'kimi')
    && normalizedWorkTurnId(event) != null
    && (event.state === 'working' || event.state === 'maybeIdle')
  );
}

function sharesLifecycleContext(
  existing: AgentWorkStateEvent,
  incoming: AgentWorkStateEvent,
): boolean {
  const cliMatches = !incoming.cli || !existing.cli || incoming.cli === existing.cli;
  const sessionMatches = (
    !incoming.sessionId
    || !existing.sessionId
    || incoming.sessionId === existing.sessionId
  );
  return cliMatches && sessionMatches;
}

function workEventRemainingMs(timestamp: string | null | undefined): number | null {
  if (!timestamp) return null;
  const parsed = Date.parse(timestamp);
  if (!Number.isFinite(parsed)) return null;
  const elapsed = Math.max(0, Date.now() - parsed);
  return Math.max(0, AGENT_WORK_INACTIVITY_TIMEOUT_MS - elapsed);
}

function compactPath(path: string): string {
  const marker = '/.agent-workspaces/';
  const idx = path.indexOf(marker);
  if (idx >= 0) return `…${path.slice(idx)}`;
  return path.length > 82 ? `…${path.slice(-81)}` : path;
}

function statusPhaseLabel(phase: AgentStatusEvent['phase'] | undefined): string {
  if (phase === 'spawned') return 'process spawned';
  if (phase === 'firstBytes') return 'first PTY bytes received';
  if (phase === 'terminalQuery') return 'terminal query answered';
  if (phase === 'exited') return 'process exited';
  return 'launch requested';
}

/** Synthetic grid shown the moment recruit is fired, replaced as soon
 *  as the first real snapshot arrives from the PTY reader thread. CC
 *  cold start can be 1-3 s — without this the seat looks frozen. */
function spawningPlaceholderGrid(
  cli: string,
  progress?: Partial<AgentStatusEvent>,
): GridSnapshot {
  const phase = statusPhaseLabel(progress?.phase);
  const elapsed =
    typeof progress?.elapsedMs === 'number' ? ` · ${progress.elapsedMs}ms` : '';
  const lines = [
    { text: '' },
    { text: `  · spawning ${cli} …` },
    { text: '' },
    { text: `  phase: ${phase}${elapsed}` },
    progress?.detail ? { text: `  detail: ${progress.detail}` } : null,
    progress?.resolvedBin ? { text: `  bin: ${compactPath(progress.resolvedBin)}` } : null,
    progress?.ttyName ? { text: `  tty: ${progress.ttyName}` } : null,
    progress?.cwd ? { text: `  cwd: ${compactPath(progress.cwd)}` } : null,
    { text: '  waiting for the first visible terminal frame' },
  ];
  return linesToGridSnapshot(lines.filter((line): line is { text: string } => line != null));
}

interface AgentRuntimeState {
  liveAgents: Set<AgentId>;
  status: Map<AgentId, AgentStatusEvent>;
  workState: Map<AgentId, AgentWorkStateEvent>;
}

export interface AgentRuntime {
  liveAgents: Set<AgentId>;
  gridStore: AgentGridStore;
  status: Map<AgentId, AgentStatusEvent>;
  workState: Map<AgentId, AgentWorkStateEvent>;
  recruit: (req: AgentSpawnRequest) => Promise<void>;
  send: (agentId: AgentId, input: string) => Promise<void>;
  submitPrompt: (agentId: AgentId, input: string) => Promise<void>;
  dismiss: (agentId: AgentId) => Promise<void>;
}

export interface AgentRuntimeExitContext {
  event: AgentExitEvent;
  request: AgentSpawnRequest;
}

export interface AgentRuntimeOptions {
  onExit?: (context: AgentRuntimeExitContext) => void | Promise<void>;
}

export function useAgentRuntime(options: AgentRuntimeOptions = {}): AgentRuntime {
  const [state, setState] = useState<AgentRuntimeState>(() => ({
    liveAgents: new Set(),
    status: new Map(),
    workState: new Map(),
  }));
  const [gridStore] = useState(createAgentGridStore);

  // unlisten functions, keyed by agentId, so re-recruiting an agent
  // tears down the old subscriptions cleanly.
  const unlistenRefs = useRef<Map<AgentId, {
    generation: number;
    fns: Array<() => void | Promise<void>>;
  }>>(new Map());
  const generationRefs = useRef<Map<AgentId, number>>(new Map());
  /** Keep the spawning placeholder until the first non-blank real frame.
   *  This is intentionally not React state: output frames must not wake App. */
  const seenContentRef = useRef<Set<AgentId>>(new Set());
  const workTimeoutRefs = useRef<Map<AgentId, number>>(new Map());
  const onExitRef = useRef(options.onExit);
  onExitRef.current = options.onExit;

  const clearWorkTimer = useCallback((agentId: AgentId) => {
    const timer = workTimeoutRefs.current.get(agentId);
    if (timer !== undefined) {
      window.clearTimeout(timer);
      workTimeoutRefs.current.delete(agentId);
    }
  }, []);

  const scheduleWorkTimeout = useCallback((agentId: AgentId, lastActivityAt?: string | null) => {
    clearWorkTimer(agentId);
    const remainingMs = workEventRemainingMs(lastActivityAt);
    const timer = window.setTimeout(() => {
      workTimeoutRefs.current.delete(agentId);
      setState((s) => {
        const current = s.workState.get(agentId);
        if (!current || hasAuthoritativeActiveTurn(current)) return s;
        const currentRemainingMs = workEventRemainingMs(
          current.lastActivityAt ?? current.timestamp,
        );
        if (currentRemainingMs != null && currentRemainingMs !== 0) return s;
        const workState = new Map(s.workState);
        workState.delete(agentId);
        return { ...s, workState };
      });
    }, remainingMs ?? AGENT_WORK_INACTIVITY_TIMEOUT_MS);
    workTimeoutRefs.current.set(agentId, timer);
  }, [clearWorkTimer]);

  const pruneExpiredWorkStates = useCallback(() => {
    setState((s) => {
      let changed = false;
      const workState = new Map(s.workState);
      for (const [agentId, evt] of workState) {
        if (hasAuthoritativeActiveTurn(evt)) continue;
        const remainingMs = workEventRemainingMs(evt.lastActivityAt ?? evt.timestamp);
        if (remainingMs !== 0) continue;
        workState.delete(agentId);
        clearWorkTimer(agentId);
        changed = true;
      }
      return changed ? { ...s, workState } : s;
    });
  }, [clearWorkTimer]);

  const applyWorkEvent = useCallback((evt: AgentWorkStateEvent) => {
    const agentId = evt.agentId as AgentId;
    setState((s) => {
      if (!s.liveAgents.has(agentId)) return s;

      const existing = s.workState.get(agentId);
      const timestamp = evt.timestamp || new Date().toISOString();
      const isTerminalState =
        evt.state === 'idle' || evt.state === 'failed' || evt.state === 'interrupted';
      const incomingTurnId = normalizedWorkTurnId(evt);
      const workState = new Map(s.workState);
      if (isTerminalState && existing && hasAuthoritativeActiveTurn(existing)) {
        if (
          !sharesLifecycleContext(existing, evt)
          || (
            incomingTurnId != null
            && incomingTurnId !== normalizedWorkTurnId(existing)
          )
        ) {
          return s;
        }
        workState.delete(agentId);
        clearWorkTimer(agentId);
        return { ...s, workState };
      }

      const existingTime = existing?.lastActivityAt ?? existing?.timestamp;
      const eventParsed = Date.parse(timestamp);
      const eventOlderThanExisting =
        !!existingTime &&
        Number.isFinite(eventParsed) &&
        eventParsed < Date.parse(existingTime) - 1000;
      if (eventOlderThanExisting) {
        return s;
      }

      if (isTerminalState) {
        workState.delete(agentId);
        clearWorkTimer(agentId);
        return { ...s, workState };
      }

      const inheritedTurnId = (
        !incomingTurnId
        && existing
        && hasAuthoritativeActiveTurn(existing)
        && sharesLifecycleContext(existing, evt)
      )
        ? normalizedWorkTurnId(existing)
        : null;
      const nextEvent: AgentWorkStateEvent = {
        ...evt,
        turnId: incomingTurnId ?? inheritedTurnId,
      };
      const authoritativeActiveTurn = hasAuthoritativeActiveTurn(nextEvent);
      const remainingMs = workEventRemainingMs(timestamp);
      if (authoritativeActiveTurn && remainingMs === 0) {
        return s;
      }
      if (!authoritativeActiveTurn && remainingMs === 0) {
        if (!existing) return s;
        workState.delete(agentId);
        clearWorkTimer(agentId);
        return { ...s, workState };
      }

      if (
        existing &&
        existing.state === nextEvent.state &&
        existing.timestamp === nextEvent.timestamp &&
        existing.sessionId === nextEvent.sessionId &&
        existing.nativeEventId === nextEvent.nativeEventId &&
        existing.turnId === nextEvent.turnId
      ) {
        return s;
      }

      const resetStartedAt =
        !existing ||
        (nextEvent.sessionId && nextEvent.sessionId !== existing.sessionId) ||
        (nextEvent.turnId && nextEvent.turnId !== existing.turnId) ||
        nextEvent.reason === 'prompt-submitted';
      workState.set(agentId, {
        ...nextEvent,
        agentId,
        timestamp,
        startedAt: resetStartedAt
          ? timestamp
          : (existing.startedAt ?? existing.timestamp ?? timestamp),
        lastActivityAt: timestamp,
      });
      if (authoritativeActiveTurn) {
        clearWorkTimer(agentId);
      } else {
        scheduleWorkTimeout(agentId, timestamp);
      }
      return { ...s, workState };
    });
  }, [clearWorkTimer, scheduleWorkTimeout]);

  useEffect(() => {
    pruneExpiredWorkStates();
    const timer = window.setInterval(pruneExpiredWorkStates, AGENT_WORK_PRUNE_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [pruneExpiredWorkStates]);

  // On unmount, drop all subscriptions.
  useEffect(() => {
    return () => {
      for (const [, subscription] of unlistenRefs.current) {
        for (const fn of subscription.fns) Promise.resolve(fn()).catch(() => {});
      }
      unlistenRefs.current.clear();
      generationRefs.current.clear();
      seenContentRef.current.clear();
      for (const [, timer] of workTimeoutRefs.current) window.clearTimeout(timer);
      workTimeoutRefs.current.clear();
      gridStore.dispose();
    };
  }, [gridStore]);

  const recruit = useCallback(async (req: AgentSpawnRequest) => {
    const agentId = req.agentId as AgentId;
    const generation = (generationRefs.current.get(agentId) ?? 0) + 1;
    generationRefs.current.set(agentId, generation);
    const isCurrentGeneration = () => generationRefs.current.get(agentId) === generation;

    // Tear down any prior subscriptions for this agentId.
    const prior = unlistenRefs.current.get(agentId);
    if (prior) {
      unlistenRefs.current.delete(agentId);
      await Promise.allSettled(
        prior.fns.map((fn) => Promise.resolve().then(() => fn())),
      );
    }
    if (!isCurrentGeneration()) return;

    // Reset accumulated state + show "spawning {cli}…" placeholder so
    // the seat doesn't look stuck during the 1-3 s CC/Codex cold start.
    seenContentRef.current.delete(agentId);
    gridStore.setSnapshot(
      agentId,
      spawningPlaceholderGrid(req.cli, {
        cwd: req.cwd,
        detail: 'output/status/exit listeners armed',
      }),
    );
    setState((s) => {
      const live = new Set(s.liveAgents);
      const st = new Map(s.status);
      const workState = new Map(s.workState);
      live.add(agentId);
      st.delete(agentId);
      workState.delete(agentId);
      clearWorkTimer(agentId);
      return { liveAgents: live, status: st, workState };
    });

    // ── Subscribe BEFORE spawn ──
    // Tauri's `emit` is fire-and-forget — listeners must be in place
    // before the reader thread first emits, or the welcome banner is
    // lost. Spawn returns the AgentRoute synchronously, so we set up
    // listeners against the predictable topic names first, then spawn.
    const newUnlistenFns: Array<() => void | Promise<void>> = [];
    const offOutput = await onAgentOutput(req.agentId, (evt) => {
      if (!isCurrentGeneration()) return;
      const snap = evt.snapshot;
      // Suppress blank snapshots until real content appears. After that
      // first frame we neither scan the grid nor touch React root state.
      if (!seenContentRef.current.has(agentId)) {
        const hasContent = snap.cells.some((cell) => cell.ch !== ' ' && cell.ch !== '');
        if (!hasContent) return;
        seenContentRef.current.add(agentId);
      }
      gridStore.setSnapshot(agentId, snap);
    });
    newUnlistenFns.push(offOutput);
    const offStatus = await onAgentStatus(req.agentId, (evt) => {
      if (!isCurrentGeneration()) return;
      console.log(`[agent:${agentId}] status evt`, evt);
      if (!seenContentRef.current.has(agentId)) {
        gridStore.setSnapshot(agentId, spawningPlaceholderGrid(evt.cli, evt));
      }
      setState((s) => {
        const st = new Map(s.status);
        st.set(agentId, evt);
        return { ...s, status: st };
      });
    });
    newUnlistenFns.push(offStatus);
    const offExit = await subscribeAgentExit(req.agentId, (evt) => {
      if (!isCurrentGeneration()) return;
      console.warn(`[agent:${agentId}] exit evt`, evt);
      generationRefs.current.set(agentId, generation + 1);
      seenContentRef.current.delete(agentId);
      gridStore.deleteSnapshot(agentId);
      const subscription = unlistenRefs.current.get(agentId);
      if (subscription?.generation === generation) {
        unlistenRefs.current.delete(agentId);
        for (const fn of subscription.fns) Promise.resolve(fn()).catch(() => {});
      }
      setState((s) => {
        const live = new Set(s.liveAgents);
        const workState = new Map(s.workState);
        live.delete(agentId);
        workState.delete(agentId);
        clearWorkTimer(agentId);
        return { ...s, liveAgents: live, workState };
      });
      const onExit = onExitRef.current;
      if (onExit) {
        void Promise.resolve(onExit({ event: evt, request: req })).catch((err) => {
          console.warn(`[agent:${agentId}] exit reconciliation failed`, err);
        });
      }
    });
    newUnlistenFns.push(offExit);
    const offWork = await onAgentWorkState(req.agentId, (evt) => {
      if (isCurrentGeneration()) applyWorkEvent(evt);
    });
    newUnlistenFns.push(offWork);

    if (!isCurrentGeneration()) {
      await Promise.allSettled(
        newUnlistenFns.map((fn) => Promise.resolve().then(() => fn())),
      );
      return;
    }
    unlistenRefs.current.set(agentId, { generation, fns: newUnlistenFns });

    // Backend kills any existing PTY for this agent_id and starts fresh.
    console.log(`[agent:${agentId}] calling pty_agent_spawn…`);
    try {
      await spawnAgentPty(req);
    } catch (err) {
      const subscription = unlistenRefs.current.get(agentId);
      if (subscription?.generation === generation) {
        for (const fn of subscription.fns) Promise.resolve(fn()).catch(() => {});
        unlistenRefs.current.delete(agentId);
      }
      if (isCurrentGeneration()) {
        generationRefs.current.set(agentId, generation + 1);
        seenContentRef.current.delete(agentId);
        gridStore.deleteSnapshot(agentId);
        setState((s) => {
          const live = new Set(s.liveAgents);
          const st = new Map(s.status);
          const workState = new Map(s.workState);
          live.delete(agentId);
          st.delete(agentId);
          workState.delete(agentId);
          clearWorkTimer(agentId);
          return { liveAgents: live, status: st, workState };
        });
      }
      throw err;
    }
    console.log(`[agent:${agentId}] pty_agent_spawn returned`);
  }, [applyWorkEvent, clearWorkTimer, gridStore]);

  const send = useCallback(async (agentId: AgentId, input: string) => {
    await writeAgentPty(agentId, input);
  }, []);

  const submitPrompt = useCallback(async (agentId: AgentId, input: string) => {
    await submitAgentPromptPty(agentId, input);
  }, []);

  const dismiss = useCallback(async (agentId: AgentId) => {
    const generation = generationRefs.current.get(agentId);
    await closeAgentPty(agentId);
    if (generation != null && generationRefs.current.get(agentId) !== generation) return;
    generationRefs.current.set(agentId, (generation ?? 0) + 1);
    const subscription = unlistenRefs.current.get(agentId);
    if (subscription && (generation == null || subscription.generation === generation)) {
      for (const fn of subscription.fns) Promise.resolve(fn()).catch(() => {});
      unlistenRefs.current.delete(agentId);
    }
    seenContentRef.current.delete(agentId);
    gridStore.deleteSnapshot(agentId);
    setState((s) => {
      const live = new Set(s.liveAgents);
      const workState = new Map(s.workState);
      live.delete(agentId);
      workState.delete(agentId);
      clearWorkTimer(agentId);
      return { ...s, liveAgents: live, workState };
    });
  }, [clearWorkTimer, gridStore]);

  return {
    liveAgents: state.liveAgents,
    gridStore,
    status: state.status,
    workState: state.workState,
    recruit,
    send,
    submitPrompt,
    dismiss,
  };
}
