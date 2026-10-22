import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { setOfflineLocalModel, type LocalLaunchCheck } from '../lib/local-rpc';
import type { AutoRouteGroup } from '../lib/string-routing';
import { useLocalModelConfiguration, type LocalModelConfiguration } from '../lib/local-model-configuration';
import { catalogModel, localThinkingForModel } from '../lib/local-auth';
import { useTavernLocal } from '../lib/tavern-local';
import { localLaunchAction, onLocalStartup, localStartErrorText,
  type LocalStartError, type LocalStartStep, type LocalStartView, type LocalStartupOptions } from '../lib/local-startup';

type Saved = LocalModelConfiguration;
interface Options {
  projectRoot: string; agentId: string; connected: boolean; loaded: boolean;
  resumeOnOpen?: boolean; focusRequest: number; linkReturnRequest?: number;
  onResume: (id: string, startup?: LocalStartupOptions) => Promise<unknown>;
  onStarted: () => void;
}
const controlled = (error: unknown, stage: string): LocalStartError => error && typeof error === 'object' && 'code' in error
  ? { ...(error as LocalStartError), stage: (error as LocalStartError).stage ?? stage }
  : { code: 'local_start_failed', stage };

/** Independent reads and a single explicit start. The backend owns cancellation
 * and the final outcome; slow metadata never locks the whole panel. */
export function useLocalWorkbenchLaunch(options: Options) {
  const { projectRoot, agentId, connected, loaded, resumeOnOpen, focusRequest, linkReturnRequest = 0 } = options;
  const shared = useLocalModelConfiguration(projectRoot, agentId);
  const configuration = shared.value, configurationLoaded = shared.loaded;
  const setConfiguration = shared.accept;
  const [draft, setDraft] = useState<Saved | null>(null);
  const local = useTavernLocal(loaded);
  const catalog = local.catalog.data, availability = local.auth.data;
  const [startup, setStartup] = useState<LocalStartView | null>(null);
  const [metadata, setMetadata] = useState<LocalStartStep[]>([]);
  const [beginning, setBeginning] = useState(false);
  const [startError, setStartError] = useState<LocalStartError | null>(null);
  const [dismissedAttempt, setDismissedAttempt] = useState<string | null>(null);
  const current = useRef(options); current.current = options;
  const state = useRef({ configuration, draft, catalog, startup }); state.current = { configuration, draft, catalog, startup };
  const alive = useRef(true), operating = useRef(false), opened = useRef<number | null>(null), returned = useRef(0), cold = useRef(false);
  const readSerial = useRef(0), eventSerial = useRef(0), operation = useRef(0);
  const configurationSerial = useRef(0);
  const pendingAttempt = useRef<string | null>(null);
  const accept = useCallback((view: LocalStartView | null) => {
    if (!alive.current || !view || view.projectRoot !== projectRoot || view.agentId !== agentId) return;
    const previous = state.current.startup;
    if (previous?.attemptId === view.attemptId && previous.revision > view.revision) return;
    if (previous && previous.attemptId !== view.attemptId && previous.steps[0]?.startedAt > view.steps[0]?.startedAt) return;
    state.current.startup = view;
    setStartup(view);
    // A historical startup snapshot is not the current saved configuration.
    // Publish our own save once; later phase events must not undo a composer edit.
    if (view.saved && view.selection && pendingAttempt.current === view.attemptId
      && (previous?.attemptId !== view.attemptId || !previous.saved)) {
      ++configurationSerial.current;
      const selection = { provider: view.selection.providerId, id: view.selection.modelId, thinking: view.selection.thinkingLevel,
        autoRoute: view.selection.autoRoute ?? state.current.configuration?.autoRoute };
      state.current.configuration = selection; setConfiguration(selection);
      if (pendingAttempt.current === view.attemptId) { state.current.draft = selection; setDraft(selection); }
    }
    if (view.state !== 'running' && pendingAttempt.current === view.attemptId) {
      operating.current = false; setBeginning(false);
    }
  }, [projectRoot, agentId, setConfiguration]);
  useEffect(() => {
    alive.current = true;
    setDraft(null); state.current.draft = null;
    let disposed = false;
    let off: (() => void) | undefined;
    void onLocalStartup(agentId, view => { ++eventSerial.current; accept(view); }).then(async unsubscribe => {
      if (disposed) { unsubscribe(); return; }
      off = unsubscribe;
      const serial = eventSerial.current;
      const view = await localLaunchAction<LocalStartView | null>(projectRoot, agentId, 'read');
      if (!disposed && serial === eventSerial.current) accept(view);
    }).catch(error => { if (!disposed) setStartError(controlled(error, 'configuration_read')); });
    return () => { disposed = true; alive.current = false; cold.current = false; ++readSerial.current; ++configurationSerial.current; off?.(); };
  }, [projectRoot, agentId, accept]);

  // A completed external save wins over the previous offline dropdown draft.
  useLayoutEffect(() => {
    ++configurationSerial.current;
    state.current.draft = null; setDraft(null);
    setStartError(error => error?.stage === 'configuration_read' ? null : error);
  }, [shared.change]);
  useEffect(() => {
    if (shared.error) setStartError(controlled(shared.error, 'configuration_read'));
  }, [shared.error]);
  const readConfiguration = useCallback(async (reload = false) => {
    const serial = ++configurationSerial.current;
    const selected = await shared.read(reload);
    if (alive.current && serial === configurationSerial.current) state.current.configuration = selected;
    return selected;
  }, [shared.read]);

  const refresh = useCallback(async (reloadLocal = true) => {
    const serial = ++readSerial.current;
    const valid = () => alive.current && serial === readSerial.current;
    const at = new Date().toISOString();
    setMetadata(['configuration_read', 'credential_status', 'model_catalog'].map(stage => ({ stage, startedAt: at, finishedAt: null, elapsedMs: 0, error: null })));
    const read = async <T,>(stage: string, run: () => Promise<T>, apply: (value: T) => void) => {
      let failure: LocalStartError | null = null;
      try { const value = await run(); if (valid()) apply(value); }
      catch (error) { failure = controlled(error, stage); }
      if (valid()) setMetadata(rows => rows.map(row => row.stage === stage ? { ...row, finishedAt: new Date().toISOString(),
        elapsedMs: Date.now() - Date.parse(at), error: failure } : row));
    };
    // Each result updates immediately. Neither metadata query can hold up SHELL.
    const configurationRead = read('configuration_read', () => readConfiguration(reloadLocal), () => {});
    if (reloadLocal) { void local.auth.refresh(); void local.catalog.refresh(); }
    await configurationRead;
  }, [readConfiguration, local.auth.refresh, local.catalog.refresh]);
  useEffect(() => {
    // Use the same catalog/status reads as the visible model picker. Initial
    // startup does not run a second native catalog query.
    const authFailure = local.auth.failed || (availability?.summary === 'error' && !availability.providers.length);
    setMetadata(rows => {
      let changed = false;
      const next = rows.map(row => {
        const source = row.stage === 'credential_status' ? { loading: local.auth.loading, failed: authFailure }
          : row.stage === 'model_catalog' ? local.catalog : null;
        if (!source || source.loading || row.finishedAt) return row;
        changed = true;
        const failure: LocalStartError | null = !source.failed ? null : row.stage === 'credential_status'
          ? availability?.error ?? { code: 'auth_status_unavailable', stage: row.stage }
          : { code: 'model_catalog_unavailable', stage: row.stage };
        return { ...row, finishedAt: new Date().toISOString(), elapsedMs: Date.now() - Date.parse(row.startedAt), error: failure };
      });
      return changed ? next : rows;
    });
  }, [metadata, local.auth.loading, local.auth.failed, availability, local.catalog.loading, local.catalog.failed]);

  const start = useCallback(async (useSaved = false) => {
    if (operating.current || current.current.connected || state.current.startup?.state === 'running') return;
    const selected = useSaved ? state.current.configuration : state.current.draft ?? state.current.configuration;
    if (!selected) { setStartError({ code: 'model_required', stage: 'configuration_read' }); return; }
    const serial = ++operation.current;
    operating.current = true; setBeginning(true); setStartError(null);
    let attemptId: string | undefined;
    try {
      const selection = !useSaved && state.current.draft ? { providerId: selected.provider, modelId: selected.id, thinkingLevel: selected.thinking,
        ...(selected.provider === 'kota' && selected.id === 'auto' && selected.autoRoute ? { autoRoute: selected.autoRoute } : {}) } : undefined;
      const begun = await localLaunchAction<{ view: LocalStartView; reused: boolean }>(projectRoot, agentId, 'begin', { selection });
      attemptId = begun.view.attemptId; pendingAttempt.current = attemptId; accept(begun.view);
      if (begun.reused) return;
      const result = await current.current.onResume(agentId, { attemptId }) as { status?: string } | boolean | undefined;
      const status = result === false ? 'cancelled' : typeof result === 'object' ? result?.status : undefined;
      if (status === 'cancelled' || status === 'failed') {
        accept(await localLaunchAction<LocalStartView>(projectRoot, agentId, status === 'cancelled' ? 'cancel' : 'failed', { attemptId }));
      } else {
        accept(await localLaunchAction<LocalStartView | null>(projectRoot, agentId, 'read'));
        if (alive.current) current.current.onStarted();
      }
    } catch (error) {
      if (attemptId) {
        try { accept(await localLaunchAction<LocalStartView>(projectRoot, agentId, 'failed', { attemptId })); }
        catch { if (alive.current && serial === operation.current) setStartError(controlled(error, 'launch_configuration')); }
      } else if (alive.current) setStartError(controlled(error, 'configuration_read'));
    } finally { if (serial === operation.current) { operating.current = false; if (alive.current) setBeginning(false); } }
  }, [projectRoot, agentId, accept]);
  const cancel = useCallback(async () => {
    const view = state.current.startup;
    if (!view || view.state !== 'running') return;
    try {
      accept(await localLaunchAction<LocalStartView>(projectRoot, agentId, 'cancel', { attemptId: view.attemptId }));
      ++operation.current; operating.current = false; setBeginning(false);
    } catch (error) { if (alive.current) setStartError(controlled(error, 'session_ready')); }
  }, [projectRoot, agentId, accept]);
  const selectModel = useCallback((provider: string, id: string) => {
    if (operating.current || state.current.startup?.state === 'running') return;
    const levels = catalogModel(state.current.catalog, provider, id)?.thinkingLevels ?? [];
    const previous = state.current.draft ?? state.current.configuration;
    const selected = { provider, id, thinking: localThinkingForModel(levels, previous?.thinking), autoRoute: previous?.autoRoute };
    state.current.draft = selected; setDraft(selected);
    setStartError(null); setDismissedAttempt(state.current.startup?.attemptId ?? null);
  }, []);
  const acceptAutoRoute = useCallback((autoRoute: AutoRouteGroup) => {
    ++configurationSerial.current;
    const selected: Saved = { provider: 'kota', id: 'auto', thinking: 'off', autoRoute };
    state.current.configuration = selected; state.current.draft = selected;
    setConfiguration(selected); setDraft(selected); setStartError(null);
    setDismissedAttempt(state.current.startup?.attemptId ?? null);
  }, [setConfiguration]);
  const saveAutoRoute = useCallback(async (autoRoute: AutoRouteGroup) => {
    if (operating.current || state.current.startup?.state === 'running') throw { code: 'local_busy' };
    const serial = configurationSerial.current;
    const saved = await setOfflineLocalModel(projectRoot, agentId, 'kota', 'auto', autoRoute);
    if (!saved.autoRoute) throw { code: 'local_save_failed' };
    if (alive.current && current.current.projectRoot === projectRoot && current.current.agentId === agentId
      && serial === configurationSerial.current) acceptAutoRoute(saved.autoRoute);
    return saved.autoRoute;
  }, [projectRoot, agentId, acceptAutoRoute]);
  const selectThinking = useCallback((thinking: string) => {
    if (operating.current || state.current.startup?.state === 'running') return;
    const selected = state.current.draft ?? state.current.configuration;
    if (selected) { const next = { ...selected, thinking }; state.current.draft = next; setDraft(next); }
    setStartError(null); setDismissedAttempt(state.current.startup?.attemptId ?? null);
  }, []);
  useEffect(() => {
    if (connected) { cold.current = false; if (loaded) void readConfiguration().catch(() => {}); return; }
    if (loaded && !cold.current) { cold.current = true; void refresh(false); }
  }, [connected, loaded, refresh, readConfiguration]);
  useEffect(() => {
    if (!loaded || !configurationLoaded || !resumeOnOpen || opened.current === focusRequest) return;
    opened.current = focusRequest;
    if (!connected && configuration) void start(true);
  }, [loaded, configurationLoaded, configuration, connected, resumeOnOpen, focusRequest, start]);
  useEffect(() => {
    if (!linkReturnRequest || returned.current === linkReturnRequest) return;
    returned.current = linkReturnRequest;
    if (!connected) void refresh().then(() => { if (alive.current && state.current.configuration) void start(true); });
  }, [linkReturnRequest, connected, refresh, start]);

  const saved = draft ?? configuration;
  const failure = startError ?? (startup?.state === 'failed' && dismissedAttempt !== startup.attemptId ? startup.error : null);
  const check: LocalLaunchCheck | null = failure ? { status: 'blocked', ...failure }
    : configurationLoaded && !saved ? { status: 'blocked', code: 'model_required' } : null;
  const running = startup?.state === 'running';
  const phase = running ? startup.steps.at(-1)?.stage === 'configuration_save' ? 'saving' : 'starting' : beginning ? 'starting' : 'idle';
  return { saved, configuration, configurationLoaded, local, catalog, availability, check, metadataError: metadata.some(row => !!row.error),
    requestError: startError,
    error: failure ? localStartErrorText(failure) : null, saveFailed: failure?.stage === 'configuration_save', phase,
    changed: startup?.saved && startup.selection ? { thinking: startup.selection.thinkingLevel, clamped: false } : null,
    cancelled: startup?.state === 'cancelled', startup, metadata, metadataLoading: metadata.some(row => !row.finishedAt),
    retry: refresh, readConfiguration, acceptAutoRoute, saveAutoRoute, start, cancel, selectModel, selectThinking };
}
