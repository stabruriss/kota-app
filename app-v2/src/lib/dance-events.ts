import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export interface DanceEventAgent { agentId: string; name: string }
export type DanceEvent = { id: number; occurredAt: string } & ({
  kind: 'selection'; outcome: 'selected' | 'sentinel' | 'direct'; target: DanceEventAgent;
  candidate?: DanceEventAgent & { probability: number }; threshold: number; strategy: 'context' | 'rule';
  reason?: 'below_threshold' | 'empty_text' | 'no_candidates' | 'target_left' | 'only_seat' | 'only_candidate';
} | { kind: 'jev_error' | 'dance_error'; code: string });

const merge = (before: DanceEvent[], added: DanceEvent[]) => [...new Map([...before, ...added].map(event => [event.id, event])).values()]
  .sort((a, b) => b.id - a.id).slice(0, 15);

/** Subscribe before the one-time snapshot; merge the race by ID. Mounted only in the dialog. */
export function useDanceEvents(projectRoot: string | null | undefined) {
  const [state, setState] = useState<{ root: string; events: DanceEvent[]; loading: boolean; failed: boolean } | null>(null);
  const [revision, retry] = useState(0);
  useEffect(() => {
    if (!projectRoot) return;
    let active = true; let stop: (() => void) | undefined;
    const empty = { root: projectRoot, events: [] as DanceEvent[], loading: true, failed: false };
    setState(before => ({ ...(before?.root === projectRoot ? before : empty), loading: true, failed: false }));
    const accept = (events: DanceEvent[], loaded = false) => {
      if (!active) return;
      setState(before => { const current = before?.root === projectRoot ? before : empty;
        return { ...current, events: merge(current.events, events), loading: loaded ? false : current.loading, failed: false }; });
    };
    void (async () => {
      try {
        const unlisten = await listen<{ projectRoot: string; event: DanceEvent }>('puppeteer-dance://event', ({ payload }) => {
          if (payload.projectRoot === projectRoot) accept([payload.event]);
        });
        if (!active) { unlisten(); return; }
        stop = unlisten;
        const events = await invoke<DanceEvent[]>('puppeteer_dance_events', { projectRoot });
        if (!Array.isArray(events)) throw new Error('invalid snapshot');
        accept(events, true);
      } catch {
        if (active) setState(before => ({ ...(before?.root === projectRoot ? before : empty), loading: false, failed: true }));
      }
    })();
    return () => { active = false; stop?.(); };
  }, [projectRoot, revision]);
  const current = state?.root === projectRoot ? state : null;
  return { events: current?.events ?? [], loading: !!projectRoot && (current?.loading ?? true), failed: current?.failed ?? false,
    retry: () => retry(n => n + 1) };
}
