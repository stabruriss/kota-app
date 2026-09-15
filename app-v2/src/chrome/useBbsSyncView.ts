import { useCallback, useEffect, useRef, useState } from 'react';
import type { BbsSyncView } from '../bbs-sync-view';

export interface BbsSyncViewSource {
  /** Read lightweight authoritative state; never start a round or scan BBS files. */
  read: () => Promise<BbsSyncView>;
  /** Best-effort invalidation hints, not progress or membership SoT. */
  listen: (changed: () => void) => Promise<() => void>;
}

export interface BbsSyncViewState {
  view: BbsSyncView | null;
  loading: boolean;
  error: string | null;
}

const MIN_REFRESH_MS = 500;
const VISIBLE_FALLBACK_MS = 5000;

/** View lifetime only. Closing BBS never disconnects/cancels the backend. */
export function useBbsSyncView(open: boolean, source: BbsSyncViewSource) {
  const [state, setState] = useState<BbsSyncViewState>({ view: null, loading: false, error: null });
  const requestRef = useRef<() => Promise<void>>(() => Promise.resolve());
  const refresh = useCallback(() => requestRef.current(), []);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    let listening = false;
    let reading = false;
    let dirty = false;
    let joined = false;
    let hasRead = false;
    let lastReadAt = -Infinity;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let timerAt = Infinity;
    let unlisten: (() => void) | null = null;
    let listenerError: string | null = null;
    // At most one shared waiter for the current read and one for a later read.
    // A command can keep local feedback until a read begun after its ACK settles;
    // ordinary hints still use the same dirty bit, cadence and listener lifetime.
    type Waiter = { promise: Promise<void>; resolve: () => void };
    let queuedWaiter: Waiter | null = null;
    let activeWaiter: Waiter | null = null;

    setState((current) => ({ ...current, loading: true, error: null }));

    function schedule(delay: number) {
      if (cancelled || !listening || reading) return;
      const dueAt = Math.max(Date.now() + delay, lastReadAt + MIN_REFRESH_MS);
      if (timer !== null && timerAt <= dueAt) return;
      if (timer !== null) clearTimeout(timer);
      timerAt = dueAt;
      timer = setTimeout(() => {
        timer = null;
        timerAt = Infinity;
        void read();
      }, Math.max(0, dueAt - Date.now()));
    }

    function changed() {
      if (cancelled) return;
      // One bit, not an event buffer: bounded even while hydration is stalled.
      dirty = true;
      schedule(0);
    }
    requestRef.current = () => {
      if (cancelled) return Promise.resolve();
      if (!queuedWaiter) {
        let resolve!: () => void;
        const promise = new Promise<void>((done) => { resolve = done; });
        queuedWaiter = { promise, resolve };
      }
      changed();
      return queuedWaiter.promise;
    };

    async function read() {
      if (cancelled || reading) return;
      reading = true;
      activeWaiter = queuedWaiter;
      queuedWaiter = null;
      dirty = false;
      lastReadAt = Date.now();
      try {
        const view = await source.read();
        if (cancelled) return;
        hasRead = true;
        joined = view.group !== null;
        setState({ view, loading: false, error: listenerError });
      } catch {
        if (cancelled) return;
        // The low-level client owns typed diagnostics/redaction. Never stringify
        // rejected payloads into a status surface that might contain credentials.
        setState((current) => ({ ...current, loading: false, error: 'Could not refresh device sync status.' }));
      } finally {
        reading = false;
        activeWaiter?.resolve();
        activeWaiter = null;
        if (!cancelled) {
          if (dirty) schedule(0);
          else if (joined || !hasRead || listenerError) schedule(VISIBLE_FALLBACK_MS);
        }
      }
    }

    void (async () => {
      try {
        const stop = await source.listen(changed);
        if (cancelled) { stop(); return; }
        unlisten = stop;
      } catch {
        if (cancelled) return;
        listenerError = 'Live status updates are unavailable. Refreshing periodically while BBS is open.';
      }
      // Listener first, then authoritative hydrate. Hints during read cause one
      // rate-limited follow-up; hints before read are covered by that snapshot.
      listening = true;
      await read();
    })();

    return () => {
      cancelled = true;
      requestRef.current = () => Promise.resolve();
      queuedWaiter?.resolve();
      activeWaiter?.resolve();
      if (timer !== null) clearTimeout(timer);
      unlisten?.();
    };
  }, [open, source]);

  return { ...state, refresh };
}
