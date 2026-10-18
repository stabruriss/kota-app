import { useCallback, useEffect, useRef, useState } from 'react';
import {
  controlLocalWorkbench, readLocalAuth, readLocalWorkbench, sameLocalScope,
  type LocalAuth, type LocalContext, type LocalModel, type LocalScope, type LocalViewBlock, type LocalWorkbenchView,
} from '../lib/local-rpc';

interface Info {
  models: LocalModel[]; levels: string[]; context: LocalContext | null;
  autoCompaction: boolean | null; auth: LocalAuth | null;
  contextFor: { scope: LocalScope; model: LocalModel | null; statsRevision: number } | null;
}
const emptyInfo: Info = { models: [], levels: [], context: null, autoCompaction: null, auth: null, contextFor: null };
const online = (v: LocalWorkbenchView | null) => v?.state === 'ready' || v?.state === 'busy';
const sameModel = (a: LocalModel | null | undefined, b: LocalModel | null | undefined) =>
  a?.provider === b?.provider && a?.id === b?.id;

function stableBlocks(previous: LocalViewBlock[] | undefined, incoming: LocalViewBlock[]) {
  if (!previous || previous === incoming) return incoming;
  const byId = new Map(previous.map((block) => [block.id, block]));
  const blocks = incoming.map((block) => {
    const old = byId.get(block.id);
    return old && old.kind === block.kind && old.label === block.label && old.text === block.text
      && old.active === block.active && old.error === block.error && old.truncated === block.truncated ? old : block;
  });
  return blocks.length === previous.length && blocks.every((block, i) => block === previous[i]) ? previous : blocks;
}

/** Mounted only for the visible card. No stream listener or hidden JS buffer. */
export function useLocalWorkbench(projectRoot: string, agentId: string) {
  const [view, setView] = useState<LocalWorkbenchView | null>(null);
  const [info, setInfo] = useState<Info>(emptyInfo);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const latest = useRef<LocalWorkbenchView | null>(null);
  const generation = useRef(0);
  const actions = useRef<{
    control: <T>(request: Record<string, unknown>) => Promise<T | undefined>;
    refresh: () => void;
  } | null>(null);

  useEffect(() => {
    const gen = ++generation.current;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let reading = false;
    let operating = false;
    let metadataScope: LocalScope | null = null;
    let metadataModel: LocalModel | null = null;
    let statsRevision = -1;
    let authProvider: string | null = null;
    let authSerial = 0;
    const current = () => generation.current === gen;
    const visible = () => current() && !document.hidden;
    const valid = (scope: LocalScope) => visible() && !!latest.current && sameLocalScope(scope, latest.current.scope);
    latest.current = null;
    setView(null); setInfo(emptyInfo); setError(null); setBusy(false); setLoaded(false);

    async function credential(provider: string) {
      if (authProvider === provider) return;
      authProvider = provider;
      const serial = ++authSerial;
      setInfo((prev) => ({ ...prev, auth: null }));
      try {
        const auth = await readLocalAuth(provider);
        if (visible() && serial === authSerial && latest.current?.model?.provider === provider) {
          setInfo((prev) => ({ ...prev, auth }));
        }
      } catch { /* Unknown is deliberately neutral; no credential guesses. */ }
    }

    async function metadata(v: LocalWorkbenchView, all: boolean) {
      const scope = v.scope;
      const request = <T,>(type: string) => controlLocalWorkbench<T>(scope, { type });
      const state = await request<{ model?: LocalModel; thinkingLevel?: string; autoCompactionEnabled?: boolean }>('get_state');
      if (!valid(scope) || !sameModel(v.model, latest.current?.model)) return;
      if (state.model) {
        const next = { ...latest.current!, model: state.model, thinkingLevel: state.thinkingLevel ?? null };
        if (state.model.provider !== 'kota' || state.model.id !== 'auto') next.autoRoute = null;
        latest.current = next; setView(next);
        void credential(state.model.provider);
      }
      let models: LocalModel[] | undefined;
      if (all) {
        models = (await request<{ models: LocalModel[] }>('get_available_models')).models;
        if (!valid(scope)) return;
      }
      const { levels } = await request<{ levels: string[] }>('get_available_thinking_levels');
      if (!valid(scope)) return;
      const stats = await request<{ contextUsage?: LocalContext | null }>('get_session_stats');
      if (!valid(scope)) return;
      setInfo((prev) => ({ ...prev, ...(models ? { models } : {}), levels,
        context: stats.contextUsage ?? null, autoCompaction: state.autoCompactionEnabled ?? null,
        contextFor: { scope, model: state.model ?? null, statsRevision: v.statsRevision } }));
      metadataScope = scope; metadataModel = state.model ?? null; statsRevision = v.statsRevision;
    }

    async function refreshMetadata(v: LocalWorkbenchView) {
      if (operating || !online(v)) return;
      if (metadataScope && sameLocalScope(metadataScope, v.scope) && sameModel(metadataModel, v.model)
        && statsRevision === v.statsRevision) return;
      operating = true; setBusy(true);
      try { await metadata(v, !metadataScope || !sameLocalScope(metadataScope, v.scope)); }
      catch (e) {
        if (valid(v.scope)) { setError(String(e)); metadataScope = v.scope; metadataModel = v.model; statsRevision = v.statsRevision; }
      } finally { operating = false; if (current()) setBusy(false); }
    }

    async function read() {
      if (reading || !visible()) return;
      reading = true;
      try {
        const before = latest.current;
        let next = await readLocalWorkbench(projectRoot, agentId, before ? { scope: before.scope, revision: before.revision, queueRevision: before.queueRevision } : undefined);
        // A model change need not advance the block revision. On reentering Auto String,
        // fetch its retained log once instead of waiting for a new routing event.
        if (visible() && next?.model?.provider === 'kota' && next.model.id === 'auto' && next.autoRoute === undefined
          && !(before?.autoRoute && sameLocalScope(before.scope, next.scope))) {
          next = await readLocalWorkbench(projectRoot, agentId);
        }
        if (!visible()) return;
        if (next && (next.scope.projectRoot !== projectRoot || next.scope.agentId !== agentId)) return;
        if (next && before && next.scope.epoch < before.scope.epoch) return;
        if (next && before && sameLocalScope(next.scope, before.scope)) {
          if (next.revision < before.revision) return;
          next.blocks ??= before.blocks; next.queue ??= before.queue;
          // Omitted means unchanged, but a concrete model must never inherit Auto String state.
          if (next.model?.provider === 'kota' && next.model.id === 'auto') next.autoRoute ??= before.autoRoute;
          else next.autoRoute = null;
          if (next.autoRoute && before.autoRoute && next.autoRoute.events.length === before.autoRoute.events.length
            && next.autoRoute.events.every((event, i) => event.id === before.autoRoute!.events[i].id)) {
            next.autoRoute.events = before.autoRoute.events;
          }
          // Keep memoized completed blocks (and the array for queue-only
          // updates) stable across fresh RPC objects, only within this run.
          if (next.blocks && next.runId === before.runId) next.blocks = stableBlocks(before.blocks, next.blocks);
        }
        if (before && (!next || !sameLocalScope(before.scope, next.scope))) {
          setInfo(emptyInfo); metadataScope = null; metadataModel = null; statsRevision = -1;
          ++authSerial; authProvider = null;
        }
        latest.current = next; setLoaded(true);
        if (!before || !next || !sameLocalScope(before.scope, next.scope)
          || next.revision !== before.revision || next.state !== before.state
          || next.compacting !== before.compacting || next.queued !== before.queued || next.error !== before.error
          || next.model?.id !== before.model?.id || next.model?.provider !== before.model?.provider
          || next.thinkingLevel !== before.thinkingLevel || next.autoRoute !== before.autoRoute) setView(next);
        if (next) void refreshMetadata(next);
      } catch (e) { if (visible()) { setError(String(e)); setLoaded(true); } }
      finally {
        reading = false;
        if (visible()) timer = setTimeout(() => void read(), latest.current?.state === 'busy' || latest.current?.compacting ? 100 : 1000);
      }
    }

    actions.current = {
      refresh() { clearTimeout(timer); setError(null); void read(); },
      async control<T>(request: Record<string, unknown>): Promise<T | undefined> {
        const v = latest.current;
        if (!visible() || operating || !v || !online(v)) return;
        operating = true; setBusy(true); setError(null);
        try {
          const result = await controlLocalWorkbench<T>(v.scope, request);
          if (!valid(v.scope)) return;
          if (request.type === 'set_model' || request.type === 'set_thinking_level') {
            // Read the clamped actual value and context, never infer success.
            await metadata(v, false);
          }
          return valid(v.scope) ? result : undefined;
        } catch (e) { if (valid(v.scope)) setError(String(e)); }
        finally { operating = false; if (current()) setBusy(false); }
      },
    };
    const visibility = () => {
      clearTimeout(timer);
      if (visible()) { metadataScope = null; void read(); }
      else { ++authSerial; authProvider = null; }
    };
    document.addEventListener('visibilitychange', visibility);
    void read();
    return () => {
      ++generation.current; clearTimeout(timer); actions.current = null;
      document.removeEventListener('visibilitychange', visibility);
    };
  }, [projectRoot, agentId]);

  const control = useCallback(<T,>(request: Record<string, unknown>) =>
    actions.current?.control<T>(request) ?? Promise.resolve(undefined), []);
  const refresh = useCallback(() => actions.current?.refresh(), []);
  // A late stats reply must not resurrect a pre-compaction count or an old
  // model's window. Keep unknown neutral until this scope/model/revision is read.
  const contextFor = info.contextFor;
  const context = view && online(view) && contextFor && sameLocalScope(contextFor.scope, view.scope)
    && sameModel(contextFor.model, view.model) && contextFor.statsRevision === view.statsRevision ? info.context : null;
  return { view, info: { ...info, context }, busy, loaded, error, control, refresh };
}
