import { useCallback, useEffect, useRef, useState } from 'react';
import { BbsRosterClientError, bbsRosterSource } from '../bbs-roster-client';
import type { BbsRosterView } from '../types/bbs-roster';

export interface BbsRosterSource {
  read: (cancelled: () => boolean) => Promise<BbsRosterView>;
  listen: (changed: () => void) => Promise<() => void>;
}

/** Open-picker lifetime only. A stable source, one read at a time, one dirty bit.
 * No network refresh or roster poll; hints/manual retry read memory at ≤2 Hz.
 * Partial/version-invalid pages never replace the last complete view. */
export function useBbsRoster(open: boolean, source: BbsRosterSource = bbsRosterSource) {
  const [state, setState] = useState<{ view: BbsRosterView | null; loading: boolean; error: string | null }>({ view: null, loading: false, error: null });
  const request = useRef(() => {});
  const refresh = useCallback(() => request.current(), []);
  useEffect(() => {
    if (!open) return;
    let cancelled = false, ready = false, reading = false, dirty = false;
    let retryChanged = 1, lastReadAt = -Infinity;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let unlisten: (() => void) | undefined;
    let listenerError: string | null = null;
    setState(current => ({ ...current, loading: true, error: null }));
    function schedule() {
      if (cancelled || !ready || reading || timer !== null) return;
      timer = setTimeout(() => { timer = null; void read(); }, Math.max(0, lastReadAt + 500 - Date.now()));
    }
    function changed() { dirty = true; retryChanged = 1; schedule(); }
    request.current = changed;
    async function read() {
      if (cancelled || reading) return;
      reading = true; dirty = false; lastReadAt = Date.now();
      try {
        const view = await source.read(() => cancelled);
        if (!cancelled) setState({ view, loading: false, error: listenerError });
      } catch (error) {
        if (cancelled) return;
        const changedVersion = error instanceof BbsRosterClientError && error.code === 'roster_changed';
        if (changedVersion && retryChanged > 0) { retryChanged--; dirty = true; }
        setState(current => ({ ...current, loading: false, error: changedVersion
          ? 'The agent roster changed. Please retry.' : 'Could not refresh the agent roster.' }));
      } finally { reading = false; if (dirty) schedule(); }
    }
    function start() { if (!ready && !cancelled) { ready = true; void read(); } }
    // Listen-first without allowing a stalled plugin registration to keep the
    // picker empty indefinitely. A late listener causes a fresh authoritative read.
    const listenDeadline = setTimeout(() => {
      listenerError = 'Live roster updates are unavailable. Reopen the picker or retry.';
      start();
    }, 2000);
    void (async () => {
      try {
        const stop = await source.listen(changed);
        if (cancelled) { stop(); return; }
        unlisten = stop; listenerError = null;
        if (ready) changed();
      } catch {
        if (cancelled) return;
        listenerError = 'Live roster updates are unavailable. Reopen the picker or retry.';
      } finally {
        clearTimeout(listenDeadline);
        start();
      }
    })();
    return () => { cancelled = true; request.current = () => {}; clearTimeout(listenDeadline); if (timer !== null) clearTimeout(timer); unlisten?.(); };
  }, [open, source]);
  return { ...state, refresh };
}
