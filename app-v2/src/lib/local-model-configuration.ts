import { useMemo, useSyncExternalStore } from 'react';
import { onLocalConfiguration } from './local-configuration';
import { localLaunchAction } from './local-startup';
import type { AutoRouteGroup } from './string-routing';

export interface LocalModelConfiguration {
  provider: string; id: string; thinking: string; autoRoute?: AutoRouteGroup;
}
interface Snapshot { value: LocalModelConfiguration | null; loaded: boolean; error: unknown; change: number }
const stores = new Map<string, ReturnType<typeof createStore>>();

/** Saved SHELL state, shared by the composer and workbench. No Pi polling. */
function createStore(projectRoot: string, agentId: string, key: string) {
  let snapshot: Snapshot = { value: null, loaded: false, error: null, change: 0 };
  const listeners = new Set<() => void>();
  let generation = 0, pending: Promise<LocalModelConfiguration | null> | null = null;
  let watching: Promise<void> | null = null, stop: (() => void) | undefined;
  const update = (next: Partial<Snapshot>) => {
    snapshot = { ...snapshot, ...next }; listeners.forEach(listener => listener());
  };
  const read = (reload = false): Promise<LocalModelConfiguration | null> => {
    if (pending) return pending;
    if (!reload && snapshot.loaded && !snapshot.error) return Promise.resolve(snapshot.value);
    const serial = ++generation;
    const task = (async () => {
      // Subscribe before the first read, so a racing save invalidates it.
      await watching;
      const value = await localLaunchAction<{ model: string | null; thinkingLevel: string; autoRoute?: AutoRouteGroup }>(projectRoot, agentId, 'configuration');
      const slash = value.model?.indexOf('/') ?? -1;
      const selected = slash > 0 ? { provider: value.model!.slice(0, slash), id: value.model!.slice(slash + 1), thinking: value.thinkingLevel, autoRoute: value.autoRoute } : null;
      if (serial === generation) update({ value: selected, loaded: true, error: null });
      return serial === generation ? selected : snapshot.value;
    })().catch(error => {
      if (serial === generation) update({ loaded: true, error });
      throw error;
    }).finally(() => { if (pending === task) pending = null; });
    pending = task;
    return task;
  };
  const store = {
    getSnapshot: () => snapshot,
    read,
    accept(value: LocalModelConfiguration) {
      ++generation; pending = null;
      update({ value, loaded: true, error: null });
    },
    subscribe(listener: () => void) {
      listeners.add(listener); stores.set(key, store);
      if (!watching) {
        let disposed = false;
        const subscription = onLocalConfiguration(projectRoot, agentId, () => {
          if (disposed) return;
          ++generation; pending = null;
          // Clear an offline workbench draft as soon as another surface saves.
          update({ change: snapshot.change + 1 });
          void read(true).catch(() => {});
        });
        watching = subscription.then(unlisten => { if (disposed) unlisten(); else stop = unlisten; }).catch(() => {});
        const end = () => { disposed = true; stop?.(); stop = undefined; watching = null; };
        stopWatching = end;
        void read().catch(() => {});
      }
      return () => {
        listeners.delete(listener);
        if (!listeners.size) {
          stopWatching?.(); ++generation; pending = null;
          if (stores.get(key) === store) stores.delete(key);
        }
      };
    },
  };
  let stopWatching: (() => void) | undefined;
  return store;
}
function configurationStore(projectRoot: string, agentId: string) {
  const key = JSON.stringify([projectRoot, agentId]);
  let store = stores.get(key);
  if (!store) { store = createStore(projectRoot, agentId, key); stores.set(key, store); }
  return store;
}
export function useLocalModelConfiguration(projectRoot: string, agentId: string) {
  const store = useMemo(() => configurationStore(projectRoot, agentId), [projectRoot, agentId]);
  const snapshot = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  return { ...snapshot, read: store.read, accept: store.accept };
}
