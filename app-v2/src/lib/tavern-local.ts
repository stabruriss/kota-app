import { useCallback, useEffect, useRef, useState } from 'react';
import {
  readLocalAuthAvailability,
  readLocalModelCatalog,
  type LocalAuthAvailability,
} from './local-auth';
import { probeMagpie } from './local-magpie';

function useLocalRead<T>(read: () => Promise<T>, enabled: boolean) {
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const sequence = useRef(0);
  const refresh = useCallback(async () => {
    if (!enabled) return;
    const current = ++sequence.current;
    setLoading(true);
    setFailed(false);
    try {
      const next = await read();
      if (current === sequence.current) setData(next);
    } catch {
      if (current === sequence.current) setFailed(true);
    } finally {
      if (current === sequence.current) setLoading(false);
    }
  }, [enabled, read]);
  useEffect(() => {
    void refresh();
    return () => { sequence.current += 1; };
  }, [refresh]);
  return { data, loading, failed, refresh };
}

/** Read-only Tavern projections. Authentication and native validation belong to D4. */
export function useTavernLocal(enabled: boolean) {
  // Each read settles independently. D4 owns the availability IPC's one retry.
  const auth = useLocalRead(readLocalAuthAvailability, enabled);
  const catalog = useLocalRead(readLocalModelCatalog, enabled);
  const active = useRef(enabled); active.current = enabled;
  useEffect(() => { active.current = enabled; return () => { active.current = false; }; }, [enabled]);
  const openModels = useCallback(async () => {
    if (!active.current) return;
    await probeMagpie().catch(() => {});
    if (active.current) await Promise.all([auth.refresh(), catalog.refresh()]);
  }, [auth.refresh, catalog.refresh]);
  return { auth, catalog, openModels };
}
export type TavernLocal = ReturnType<typeof useTavernLocal>;
export type TavernLocalState = 'ready' | 'not_ready' | 'checking' | 'invalid' | 'error';

export function tavernLocalState(auth: TavernLocal['auth'], model = 'default'): TavernLocalState {
  if (auth.loading || (!auth.data && !auth.failed)) return 'checking';
  if (auth.failed || !auth.data) return 'error';
  const availability: LocalAuthAvailability = auth.data;
  if (availability.summary === 'error' && availability.providers.length === 0) return 'error';
  const specific = model.trim() && model.trim().toLowerCase() !== 'default';
  const entries = specific
    ? availability.providers.filter((provider) => provider.providerId === model.trim().split('/')[0])
    : availability.providers;
  if (entries.some((provider) => provider.status === 'signed_in')) return 'ready';
  if (entries.some((provider) => provider.status === 'error') || (!specific && availability.summary === 'error')) return 'error';
  if (entries.some((provider) => provider.status === 'checking')) return 'checking';
  if (entries.some((provider) => provider.status === 'invalid')) return 'invalid';
  return 'not_ready';
}
