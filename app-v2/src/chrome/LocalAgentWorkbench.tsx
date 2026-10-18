import type { LocalStartupOptions } from '../lib/local-startup';
import { localStartStage, localStartErrorText, type LocalStartStep } from '../lib/local-startup';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import type { Agent, AgentId } from '../types/scene';
import type { ProjectAgentRecord } from '../pty-client';
import type { LocalContext, LocalQueueItem, LocalViewBlock } from '../lib/local-rpc';
import { useLocalWorkbench } from '../hooks/useLocalWorkbench';
import { useLocalWorkbenchLaunch } from '../hooks/useLocalWorkbenchLaunch';
import { catalogModel, linkedCatalogProviders, LOCAL_AUTH_PROVIDERS, requestKotaLocalLink } from '../lib/local-auth';
import { LocalStreamBlock as StreamBlock, localElapsedLabel as elapsedLabel } from './LocalStreamBlock';
import { LocalShadowClonePanel } from './LocalShadowClonePanel';
import { onMagpieChanged, type MagpieState } from '../lib/local-magpie';
import { useJevSettings } from '../lib/jev';
import { AUTO_ROUTE_MODEL, AUTO_ROUTE_RESPONSE_COPY, type AutoRouteGroup } from '../lib/string-routing';
import { StringLocalModelPicker } from './StringLocalModelPicker';
import { StringRouteLog } from './StringRouteLog';
import { LocalQueueEditor } from './LocalQueueEditor';
import { LocalMcpPanel } from './LocalMcpPanel';
import { localQueueCanEdit } from '../lib/local-queue';
import { useLocalQueueReorder } from '../hooks/useLocalQueueReorder';
import { controlLocalWorkbench, sameLocalScope } from '../lib/local-rpc';
import type { LocalQueueController } from '../hooks/useLocalQueue';
import { HeroAvatarArt } from './HeroAvatarPicker';
import flame from '../assets/tavern/icons/tavern-entry.svg';
import turnsIcon from '../assets/tavern/icons/turns.svg';
import likesIcon from '../assets/tavern/icons/commends.svg';
import '../styles/local-workbench.css';

interface Props {
  projectRoot: string; agentId: AgentId; agent: Agent; record?: ProjectAgentRecord;
  agents: Readonly<Record<AgentId, Agent>>; section?: 'queue'; focusRequest: number;
  resumeOnOpen?: boolean; linkReturnRequest?: number;
  targets?: readonly AgentId[]; queueController?: LocalQueueController;
  privateMessage?: boolean;
  onClose: () => void; onDetails: (id: AgentId) => void;
  onFresh: (id: AgentId) => void; onResume: (id: AgentId, startup?: LocalStartupOptions) => Promise<unknown>;
}
const sources: Record<string, string> = {
  'kota.shadow_clone': 'Kage Bunshin', 'kota.user': 'Composer', 'kota.lm': 'Laughing Man', 'kota.bus': 'Agent Bus',
  'kota.bbs': 'BBS', 'kota.ember': 'Ember', 'kota.dream': 'Dream', 'kota.bartender': 'Bartender',
};
const clock = new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' });
const timeLabel = (value: string) => Number.isFinite(Date.parse(value)) ? clock.format(new Date(value)) : value;
export function contextRemaining(context: LocalContext | null): number | null {
  if (context?.percent == null || context.tokens == null || !Number.isFinite(context.percent)
    || !Number.isFinite(context.tokens) || context.tokens < 0
    || !Number.isFinite(context.contextWindow) || context.contextWindow <= 0) return null;
  return Math.max(0, Math.min(100, 100 - context.percent));
}

export function contextReading(context: LocalContext | null, estimate?: number | null) {
  const remaining = contextRemaining(context);
  if (remaining !== null) return { remaining, estimated: false };
  // Pi deliberately withholds post-compaction usage. Only its successful-end
  // estimate, paired with a current Pi window, can fill that gap.
  if (context?.tokens === null && context.percent === null && Number.isFinite(context.contextWindow)
    && context.contextWindow > 0 && estimate != null && Number.isFinite(estimate) && estimate >= 0) {
    return { remaining: Math.max(0, Math.min(100, 100 - estimate / context.contextWindow * 100)), estimated: true };
  }
  return { remaining: null, estimated: false };
}

function StartupSteps({ rows, now, attempt = false }: { rows: LocalStartStep[]; now: number; attempt?: boolean }) {
  return <ol className="startup-steps" aria-label="Startup steps">{rows.map((row, index) => <li key={`${row.startedAt}:${index}`} className={row.error ? 'failed' : row.finishedAt ? 'done' : 'active'}>
    <span className="startup-mark" aria-hidden="true">{row.error ? '!' : row.finishedAt ? '✓' : '·'}</span>
    <div><span>{attempt && row.stage === 'configuration_read' ? 'Check launch configuration' : attempt && row.stage === 'credential_status' ? 'Check launch credentials' : localStartStage(row.stage)}</span>{row.error && <p role="status">{localStartErrorText(row.error)}</p>}</div>
    <time>{row.finishedAt ? `${(row.elapsedMs / 1000).toFixed(1)}s` : elapsedLabel(row.startedAt, now)}</time>
  </li>)}</ol>;
}

function QueueRow({ item, sender, disabled, editDisabled, expand, operation, onRemove, onEdit, rowProps, grip }: { rowProps: { className: string; 'data-queue-id': string }; grip: ReactNode; item: LocalQueueItem; sender?: string; disabled: boolean; editDisabled?: boolean;
  operation?: 'edit' | 'remove'; onRemove?: () => void; onEdit?: () => void;
  expand: (id: string) => Promise<{ text: string; truncated: boolean } | undefined> }) {
  const [open, setOpen] = useState(false);
  const [full, setFull] = useState<{ text: string; truncated: boolean }>();
  const [loading, setLoading] = useState(false);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  async function toggle() {
    if (open) { setOpen(false); return; }
    if (!item.truncated || full) { setOpen(true); return; }
    setLoading(true);
    try {
      const result = await expand(item.kotaId);
      if (alive.current && result) { setFull(result); setOpen(true); }
    } finally { if (alive.current) setLoading(false); }
  }
  return <article {...rowProps}>{grip}
    <span className="source-label">{sources[item.customType] ?? 'Message'}</span>
    <div className="queue-body"><div className="queue-meta">{sender && <strong>{sender}</strong>}<time dateTime={item.createdAt}>{timeLabel(item.createdAt)}</time>
      <span className="sending-label">{item.handed ? 'Already sent' : item.state === 'clear_requested' ? 'Clearing…' : item.state === 'accepted' ? 'Queued' : item.state === 'sent' ? 'Sending…' : 'Held'}</span></div>
      {(onRemove || onEdit) && <div className="queue-row-actions">
        {onEdit && <button className="queue-edit" disabled={disabled || editDisabled || !!operation || item.handed} onClick={onEdit}>{operation === 'edit' ? 'Taking out…' : 'Edit'}</button>}
        {onRemove && <button className="queue-remove" disabled={disabled || !!operation || item.handed} onClick={onRemove}>{operation === 'remove' ? 'Removing…' : 'Remove'}</button>}
      </div>}
      <p className={`queue-text${open ? '' : ' collapsed'}`}>{open && full ? full.text : item.summary || 'No text content'}</p>
      {open && full?.truncated && <small className="stream-limit">Display limit reached. Original message is retained.</small>}
      {(item.truncated || item.summary.length > 100 || item.summary.includes('\n')) && <button className="queue-more" disabled={disabled || loading} onClick={() => void toggle()}>{loading ? 'Loading…' : open ? 'Less' : 'More'}</button>}
    </div>
  </article>;
}

export function LocalAgentWorkbench({ projectRoot, agentId, agent, record, agents, section, focusRequest, resumeOnOpen, linkReturnRequest,
  onClose, onDetails, onFresh, onResume, targets = [], privateMessage = false, queueController }: Props) {
  const { view, info, busy, loaded, error, control, refresh } = useLocalWorkbench(projectRoot, agentId);
  const panel = useRef<HTMLElement>(null);
  const [magpie, setMagpie] = useState<MagpieState | null>(null);
  useEffect(() => onMagpieChanged(setMagpie), []);
  const queue = useRef<HTMLElement>(null);
  const scroll = useRef<HTMLDivElement>(null);
  const scrollFrame = useRef<number | null>(null);
  const follow = useRef(true);
  const [following, setFollowing] = useState(true);
  const [now, setNow] = useState(Date.now);
  const [cloneActivity, setCloneActivity] = useState(false);
  const connected = view?.state === 'ready' || view?.state === 'busy';
  const launch = useLocalWorkbenchLaunch({ projectRoot, agentId, connected, loaded, resumeOnOpen, focusRequest, linkReturnRequest, onResume, onStarted: refresh });
  const jev = useJevSettings(loaded);
  const [modelSaving, setModelSaving] = useState(false);
  const launchBusy = launch.phase !== 'idle';
  const compacting = connected && !!view?.compacting;
  const working = connected && (view?.state === 'busy' || compacting);
  const scopeKey = view ? JSON.stringify([view.scope.projectRoot, view.scope.agentId, view.scope.sessionId, view.scope.epoch]) : 'offline';
  const liveOwner = useRef({ projectRoot, agentId, scope: view?.scope });
  liveOwner.current = { projectRoot, agentId, scope: view?.scope };
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const rejectedRun = useRef<string | null>(null);
  useEffect(() => {
    if (view?.autoRoute?.error?.code !== 'jev_auth') return;
    const run = `${scopeKey}:${view.runId}`;
    if (rejectedRun.current !== run) { rejectedRun.current = run; void jev.refresh(); }
  }, [view?.autoRoute?.error?.code, scopeKey, view?.runId, jev.refresh]);
  const saveAutoRoute = async (autoRoute: AutoRouteGroup) => {
    if (!connected || !view) return launch.saveAutoRoute(autoRoute);
    const scope = view.scope;
    const result = await controlLocalWorkbench<{ autoRoute?: AutoRouteGroup }>(scope, { type: 'set_model', provider: 'kota', modelId: 'auto', autoRoute });
    const owner = liveOwner.current;
    if (!alive.current || owner.projectRoot !== projectRoot || owner.agentId !== agentId || !owner.scope || !sameLocalScope(scope, owner.scope)) throw { code: 'stale_scope' };
    if (!result.autoRoute) throw { code: 'local_save_failed' };
    launch.acceptAutoRoute(result.autoRoute); refresh();
    return result.autoRoute;
  };
  const observedRun = useRef<{ scopeKey: string; runId: number } | null>(null);
  const blocks = view?.blocks ?? [];
  const outputShown = connected && ((working && !!view?.startedAt)
    || (observedRun.current?.scopeKey === scopeKey && observedRun.current.runId === view?.runId && blocks.length > 0));
  const motionQuery = useMemo(() => window.matchMedia('(prefers-reduced-motion: reduce)'), []);
  const [reducedMotion, setReducedMotion] = useState(() => motionQuery.matches);
  const observedStartup = useRef<string | null>(null);
  const [finishedStartup, setFinishedStartup] = useState<string | null>(null);
  const startupReady = launch.startup?.state === 'ready';
  const completingStartup = startupReady && observedStartup.current === launch.startup?.attemptId
    && finishedStartup !== launch.startup?.attemptId && !reducedMotion && !outputShown;
  const startupFailed = !!launch.requestError || launch.startup?.state === 'failed' || launch.startup?.state === 'cancelled';
  const startupLogShown = !!(launch.startup || launch.requestError || launch.metadata.length)
    && (!connected || launchBusy || completingStartup || startupFailed) && (!startupReady || completingStartup);
  const startupPinned = !outputShown && startupLogShown && (launchBusy || completingStartup);
  useEffect(() => {
    const change = () => setReducedMotion(motionQuery.matches);
    motionQuery.addEventListener('change', change);
    return () => motionQuery.removeEventListener('change', change);
  }, [motionQuery]);
  useLayoutEffect(() => {
    if (launch.startup?.state === 'running') observedStartup.current = launch.startup.attemptId;
    // Do not replay a success animation after output takes over, a motion
    // preference changes, or an already-idle panel is reopened.
    if (startupReady && (outputShown || reducedMotion)) setFinishedStartup(launch.startup!.attemptId);
  }, [launch.startup?.attemptId, launch.startup?.state, startupReady, outputShown, reducedMotion]);
  // Only retain a run this mounted panel actually observed working. An idle
  // reopen must not display the completed snapshot still held by Rust.
  useLayoutEffect(() => {
    if (working && view) observedRun.current = { scopeKey, runId: view.runId };
    else if (observedRun.current?.scopeKey !== scopeKey) observedRun.current = null;
  }, [scopeKey, working, view?.runId]);
  const queueItems = view?.queue ?? [];
  const draft = queueController?.drafts.get(queueController.ownerKey(projectRoot, agentId));
  const addDraft = queueController?.addDrafts.get(queueController.ownerKey(projectRoot, agentId));
  const editPending = !!view && queueItems.some(item => queueController?.pending.get(queueController.itemKey(view.scope, item)) === 'edit');
  const queueError = queueController?.errors.get(queueController.ownerKey(projectRoot, agentId));
  const reorder = useLocalQueueReorder({ items:queueItems, scopeKey, identity:`${scopeKey}:${view?.queueRevision}`,
    disabled:busy || !connected || queueItems.some(item => !!view && queueController?.pending.has(queueController.itemKey(view.scope, item))),
    submit:ids => controlLocalWorkbench(view!.scope, { type:'queue_reorder', ids }), refresh,
    renderGhost:item => <><span className="ghost-grip" aria-hidden="true">{Array.from({ length:6 }, (_, i) => <i key={i} />)}</span><span className="ghost-source">{sources[item.customType] ?? 'Message'}</span>
      <div className="ghost-body"><div className="ghost-meta"><time>{timeLabel(item.createdAt)}</time><span>Queued</span></div><p>{item.summary || 'No text content'}</p></div></> });
  useEffect(() => {
    if (draft?.valid && view && (!sameLocalScope(draft.scope, view.scope) || view.state === 'offline' || view.state === 'error')) queueController?.invalidate(draft.scope);
  }, [draft, view?.scope, view?.state, queueController?.invalidate]);
  const cancelScroll = useCallback(() => {
    if (scrollFrame.current !== null) cancelAnimationFrame(scrollFrame.current);
    scrollFrame.current = null;
  }, []);
  const followLatest = useCallback(() => {
    if (document.hidden || !follow.current || !scroll.current || scrollFrame.current !== null) return;
    // Read layout only once per frame, even if multiple snapshots commit.
    scrollFrame.current = requestAnimationFrame(() => {
      scrollFrame.current = null;
      if (!document.hidden && follow.current && scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight;
    });
  }, []);

  useEffect(() => {
    if (section === 'queue') { queue.current?.scrollIntoView?.({ block: 'nearest' }); queue.current?.focus(); }
    else panel.current?.focus();
  }, [focusRequest, section]);
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing || e.key !== 'Escape') return;
      if (document.querySelector('[role="dialog"], [role="menu"]')) return;
      e.preventDefault(); e.stopPropagation(); onClose();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [onClose]);
  useEffect(() => {
    let timer: ReturnType<typeof setInterval> | undefined;
    const visibility = () => {
      clearInterval(timer);
      if (!document.hidden && (working || launchBusy || launch.metadataLoading || cloneActivity)) { setNow(Date.now()); timer = setInterval(() => setNow(Date.now()), 1000); }
    };
    visibility(); document.addEventListener('visibilitychange', visibility);
    return () => { clearInterval(timer); document.removeEventListener('visibilitychange', visibility); };
  }, [working, launchBusy, launch.metadataLoading, cloneActivity]);
  useLayoutEffect(() => { cancelScroll(); follow.current = true; setFollowing(true); }, [scopeKey, view?.runId, cancelScroll]);
  useLayoutEffect(() => { if (startupPinned) { follow.current = true; setFollowing(true); } }, [startupPinned, launch.startup?.attemptId]);
  useLayoutEffect(() => { if (outputShown || startupLogShown) followLatest(); else cancelScroll(); },
    [blocks, outputShown, startupLogShown, launch.metadata, launch.startup, followLatest, cancelScroll]);
  useEffect(() => {
    const element = scroll.current;
    if (!startupPinned || !element) return;
    const preventWheel = (event: WheelEvent) => event.preventDefault();
    element.addEventListener('wheel', preventWheel, { passive: false });
    return () => element.removeEventListener('wheel', preventWheel);
  }, [startupPinned]);
  useEffect(() => {
    const visibility = () => { if (document.hidden) cancelScroll(); else followLatest(); };
    document.addEventListener('visibilitychange', visibility);
    return () => { document.removeEventListener('visibilitychange', visibility); cancelScroll(); };
  }, [cancelScroll, followLatest]);

  const models = useMemo(() => {
    const grouped = new Map<string, typeof info.models>();
    const catalog = linkedCatalogProviders(launch.catalog).flatMap(p =>
      p.models.map(m => ({ provider: p.providerId, id: m.modelId, name: m.name })));
    const choices = connected ? launch.catalog
      ? [...info.models.filter(m => m.provider !== 'magpie'), ...catalog.filter(m => m.provider === 'magpie')]
      : info.models : catalog;
    for (const model of choices) grouped.set(model.provider, [...(grouped.get(model.provider) ?? []), model]);
    return [...grouped.entries()];
  }, [connected, info.models, launch.catalog]);
  const selectedModel = connected ? view?.model : launch.saved;
  const selected = selectedModel ? `${selectedModel.provider}/${selectedModel.id}` : '';
  const automatic = selected === AUTO_ROUTE_MODEL || (connected && !!view?.autoRoute);
  const selectedMagpie = selectedModel?.provider === 'magpie';
  const selectedName = selectedModel ? info.models.find(m => m.provider === selectedModel.provider && m.id === selectedModel.id)?.name
    || catalogModel(launch.catalog, selectedModel.provider, selectedModel.id)?.name || selectedModel.id : 'Unavailable';
  const providerName = LOCAL_AUTH_PROVIDERS.find(p => p.id === selectedModel?.provider)?.name ?? selectedModel?.provider ?? 'Provider';
  const coldAuth = launch.availability?.providers.find(p => p.providerId === selectedModel?.provider);
  const checkCode = launch.check?.status === 'blocked' ? launch.check.code : null;
  const checkReason = !automatic && launch.check?.status === 'blocked' && launch.check.providerId === selectedModel?.provider ? launch.check.code : null;
  const autoBlocked = automatic && (['auto_route_models_required', 'auto_route_invalid', 'jev_key_required', 'jev_key_rejected'].includes(checkCode ?? '')
    || (!jev.loading && !jev.failed && !jev.settings?.ready));
  const modelRequired = !connected && launch.configurationLoaded && (!selectedModel || autoBlocked);
  const modelUnavailable = !connected && !!selectedModel && checkCode === 'model_not_found';
  const signedOut = checkReason === 'provider_not_signed_in';
  const invalid = checkReason === 'credential_invalid' || checkReason === 'azure_resource_missing';
  const unknown = !automatic && !connected && !modelRequired && !launch.cancelled && !signedOut && !invalid && (launch.metadataError || coldAuth?.status === 'error'
    || (launch.check?.status === 'blocked' && !modelRequired && !modelUnavailable));
  const noneSignedIn = launch.availability?.summary === 'not_ready' && launch.availability.providers.every(p => p.status === 'not_signed_in' || p.status === 'invalid');
  const needsSignIn = !automatic && !connected && !launch.cancelled && !unknown && (signedOut || invalid || coldAuth?.status === 'not_signed_in' || coldAuth?.status === 'invalid'
    || (modelRequired && noneSignedIn));
  const needsAgain = invalid || coldAuth?.status === 'invalid';
  const loginReason = needsAgain ? `${providerName} needs to sign in again.` : noneSignedIn ? 'No provider is signed in.' : `${providerName} isn’t signed in.`;
  const auth = connected ? info.auth : coldAuth ? { status: coldAuth.status === 'signed_in' ? 'ready' : 'not_ready', authType: coldAuth.authType } : null;
  const thinking = connected ? view?.thinkingLevel : launch.saved?.thinking;
  const thinkingLevels = connected && (!selectedMagpie || !launch.catalog) ? info.levels : selectedModel ? catalogModel(launch.catalog, selectedModel.provider, selectedModel.id)?.thinkingLevels ?? [] : [];
  const routeErrorCode = view?.autoRoute?.error?.code;
  const routeEvent = view?.autoRoute?.events.find(event => event.id === view.autoRoute?.error?.eventId);
  const routeOriginalError = routeErrorCode === 'route_exhausted' && routeEvent && routeEvent.kind !== 'selection' ? routeEvent.message : null;
  const routeError = routeErrorCode ? routeOriginalError?.startsWith(AUTO_ROUTE_RESPONSE_COPY[routeErrorCode]) ? routeOriginalError : [AUTO_ROUTE_RESPONSE_COPY[routeErrorCode], routeOriginalError].filter(Boolean).join('\n') : null;
  const lastErrorBlock = routeError ? blocks.reduce<string | null>((id, block) => block.kind === 'end' && block.error ? block.id : id, null) : null;
  const routeErrorBlock: LocalViewBlock | null = routeError ? { id: 'string-error', kind: 'end', label: 'Response error', text: routeError, error: true, active: false, truncated: false } : null;
  const modelNote = launch.phase === 'saving' ? 'Saving…' : launch.changed
    ? launch.changed.clamped ? `Saved. Thinking set to ${launch.changed.thinking} for ${selectedName}.` : 'Saved.'
    : connected ? 'Changes apply to the next model request.' : modelUnavailable ? 'This model is unavailable. Choose another model.'
    : selectedModel ? 'Choose model and thinking, then start the session.' : null;
  const reading = contextReading(info.context, view?.contextEstimate);
  const remaining = compacting ? null : reading.remaining;
  const estimated = !compacting && reading.estimated;
  const contextStatus = compacting ? 'Compacting' : 'Usage unavailable';
  const phase = compacting ? 'Compacting' : view?.phase === 'Compacting' ? 'Working' : view?.phase || 'Working';
  const offline = loaded && (!view || view.state === 'offline' || view.state === 'error');
  const message = error || ((launch.saveFailed || (!needsSignIn && !modelRequired && !modelUnavailable)) && launch.error) || (!needsSignIn && !modelRequired && !modelUnavailable && view?.error);
  const emptyContent = <><img src={flame} alt="" /><strong>{!loaded ? 'Opening workbench…' : launchBusy ? 'Starting…' : compacting ? 'Compacting context.' : working ? 'Working.' : offline ? 'Session is offline.' : 'Ready when you are.'}</strong>
    <p>{!loaded || launch.phase === 'starting' ? 'Reading the current session.' : launch.phase === 'saving' ? `${agent.name} starts once the model is saved.`
      : selectedMagpie && needsSignIn ? 'Open Magpie settings in Link.'
      : modelRequired && !unknown ? needsSignIn ? 'Sign in, then choose a model to continue.' : 'Choose a model to continue.'
      : needsSignIn ? needsAgain ? `Sign in again${models.length ? ', or choose another model' : ''} to continue.` : models.length ? 'Sign in, or choose another model to continue.' : 'Sign in to a provider to continue.'
      : unknown ? 'Retry to reload credentials and models.' : working ? 'Pi is preparing the next model request.' : offline ? 'Open a session or send a message to continue.' : `Send ${agent.name} a message to begin.`}</p>
    {offline && !launchBusy && !needsSignIn && !modelRequired && <button className="soft-button" disabled={!selectedModel || modelSaving} onClick={() => void launch.start()}>Start session</button>}
  </>;
  return <section ref={panel} tabIndex={-1} className="local-workbench" aria-label={`${agent.name} workbench`} data-local-workbench={agentId}>
    <header className="bench-header">
      <HeroAvatarArt className="bench-avatar" avatarId={agent.avatarId} provider="kota-local" />
      <div className="identity"><div className="name-line"><h1>{agent.name}</h1><img className="flame" src={flame} alt="Kota Local" /></div>
        <div className="status-line"><i className={connected ? 'online' : ''} /><span>{connected ? 'Online' : view?.state === 'starting' || launch.phase === 'starting' ? 'Starting' : offline ? 'Offline' : 'Connecting'}</span>
          {working && <><span className="status-separator">·</span><span>{phase}</span><time>{elapsedLabel(view?.startedAt, now)}</time></>}
        </div></div>
      <div className="header-stats"><span title="Completed turns"><img src={turnsIcon} alt="" /><b>{record?.turns ?? '—'}</b><small>turns</small></span>
        <span title="Likes"><img src={likesIcon} alt="" /><b>{record?.commends ?? '—'}</b></span></div>
      <button className="icon-button" title="View incarnation details" aria-label="View incarnation details" onClick={() => onDetails(agentId)}><svg viewBox="0 0 24 24"><circle cx="12" cy="7" r="3" /><path d="M5 21v-3a7 7 0 0 1 14 0v3" /></svg></button>
      <button className="icon-button" title="Hide workbench · Esc" aria-label="Hide workbench" onClick={onClose}>−</button>
    </header>
    <div className="model-strip">
      <div className="bench-model-field"><span>Model</span><StringLocalModelPicker key={`${projectRoot}:${agentId}`} ownerKey={`${projectRoot}:${agentId}`}
        owner={agent.name} surface="workbench" applySource={{ projectRoot, agentId }} ruleFile={{ projectRoot, agentId, kind: 'string' }} model={automatic ? AUTO_ROUTE_MODEL : selected} group={launch.saved?.autoRoute}
        catalog={launch.catalog} jev={jev} disabled={!loaded || busy || launchBusy} onOpen={() => void launch.local.openModels()}
        onSelect={choice => { if (connected) void control({ type: 'set_model', provider: choice.providerId, modelId: choice.modelId }); else launch.selectModel(choice.providerId, choice.modelId); }}
        onAutoRouteChange={saveAutoRoute} onSavingChange={setModelSaving} onJevSettings={() => requestKotaLocalLink('puppeteer-jev')} /></div>
      <label><span>Thinking</span><select aria-label="Thinking" disabled={automatic || launchBusy || busy || !thinkingLevels.length} value={automatic ? 'off' : thinking ?? ''}
        onChange={(e) => { if (connected) void control({ type: 'set_thinking_level', level: e.target.value }); else launch.selectThinking(e.target.value); }}>
        {automatic ? <option value="off">By Auto String</option> : <>
          {!thinkingLevels.includes(thinking ?? '') && <option value={thinking ?? ''}>{thinking ?? '—'}</option>}
          {thinkingLevels.map((level) => <option key={level} value={level}>{level}</option>)}
        </>}
      </select></label>
      {!automatic && (selectedMagpie ? <div className="login-status">Magpie · {magpie?.linked ? 'Linked' : 'Not connected'}</div>
      : modelRequired && !needsSignIn && !unknown ? <div className="login-status signin-hint warn" role="status"><i /><span className="hint-text">Choose a model to start this agent.</span></div>
      : needsSignIn || unknown ? <div className={`login-status signin-hint${needsSignIn ? ' warn' : ' neutral'}`} role="status">
        <i /><span className="hint-text">{needsSignIn ? loginReason : 'Status unavailable'}</span>
        <button className="hint-action" disabled={launchBusy} onClick={() => needsSignIn ? requestKotaLocalLink() : void launch.retry()}>{needsSignIn ? 'Sign in' : 'Retry'}</button>
      </div> : <div className="login-status" title="Read-only credential status. This does not confirm quota or service availability.">
        <i className={auth?.status === 'ready' ? 'linked' : ''} /><span>{selectedModel?.provider ?? 'Credentials'}</span><span className="login-dot">·</span>
        <span>{auth?.authType === 'oauth' ? 'OAuth' : auth?.authType === 'api_key' ? 'API key' : '—'}</span>
        <span className="linked-label">{auth?.status === 'ready' ? 'Linked' : auth ? 'Not linked' : 'Status unavailable'}</span>
      </div>)}
      {automatic && <StringRouteLog key={scopeKey} events={view?.autoRoute?.events ?? []}
        jevStatus={jev.loading ? 'Checking…' : jev.failed ? 'Status unavailable' : jev.settings?.ready ? 'Configured' : jev.settings?.keys.find(key => key.providerId === jev.settings!.provider)?.status === 'rejected' ? 'Rejected' : 'Not set'} />}
      <div className="mcp-model-footer">
        {modelNote && <p className={`model-note${launch.changed ? ' changed' : ''}`}>{modelNote}</p>}
        <div className="bench-tabs"><LocalMcpPanel projectRoot={projectRoot} agentId={agentId} scope={connected ? view?.scope : undefined} now={now} />
          <LocalShadowClonePanel projectRoot={projectRoot} agentId={agentId} scope={view?.scope} revision={view?.revision} now={now} onActivityChange={setCloneActivity} />
        </div>
      </div>
    </div>
    <div className="bench-content">
      <div className="activity-and-context">
        <section className="activity" aria-label="Current turn">
          <div className="section-heading"><h2>Current turn {(working || launchBusy) && <span className="live-dot" />}</h2>{launchBusy ? <button className="stop-button" disabled={!launch.startup} onClick={() => void launch.cancel()}>Cancel startup</button> : working && <button className="stop-button" disabled={busy} onClick={() => { reorder.cancel(); void control({ type: 'abort' }); }}><i />Stop</button>}</div>
          {message && <p className="workbench-error" role="status">{message}</p>}
          {routeErrorBlock && (!outputShown || !lastErrorBlock) && <StreamBlock block={routeErrorBlock} />}
          {outputShown ? <div className="activity-frame"><div ref={scroll} className="activity-scroll" tabIndex={0} aria-label="Current turn output"
            onScroll={(e) => { const el = e.currentTarget; follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24; if (!follow.current) cancelScroll(); setFollowing(follow.current); }}>
            {(view?.dropped || blocks.length > 60) ? <p className="stream-limit">Showing the latest display blocks. Room history is unchanged.</p> : null}
            {blocks.slice(-60).map((block) => <StreamBlock key={`${scopeKey}:${view?.runId}:${block.id}`} block={routeError && block.id === lastErrorBlock ? { ...block, text: routeError } : block}
              now={block.kind === 'thinking' && block.active ? now : undefined} />)}
            {!blocks.length && <p className="stream-limit">Waiting for output…</p>}
          </div>{!following && <button className="latest-button" onClick={() => { cancelScroll(); follow.current = true; setFollowing(true); if (scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight; }}>Latest ↓</button>}</div>
          : <div className={`activity-frame startup-frame${completingStartup ? ' completing' : ''}`}>
            <div ref={scroll} className={`activity-scroll startup-scroll${startupPinned ? ' pinned' : ''}`} tabIndex={startupLogShown && !startupPinned ? 0 : undefined}
              onScroll={(e) => {
                if (startupPinned) { followLatest(); return; }
                // The terminal result changes the layout. Let the scheduled
                // follow reach it before treating later scrolling as manual.
                if (scrollFrame.current !== null) return;
                const el = e.currentTarget; follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
              }}
              onAnimationEnd={(e) => { if (e.target === e.currentTarget && e.animationName === 'local-startup-log-out' && completingStartup) setFinishedStartup(launch.startup!.attemptId); }}>
              {startupLogShown && <>
                {!startupPinned && <div className="empty-state startup-guide">{emptyContent}</div>}
                <div className="startup-log" aria-label="Session startup log">
                  <StartupSteps rows={launch.metadata} now={now} />
                  {launch.requestError && <p className="workbench-error" role="status">{localStartErrorText(launch.requestError)}</p>}
                  {!startupPinned && launch.startup?.selection && <p className="startup-selection">{launch.startup.selection.providerId} / {launch.startup.selection.modelId} · {launch.startup.selection.thinkingLevel}</p>}
                  {launch.startup && <><StartupSteps rows={launch.startup.steps} now={now} attempt />
                    {launch.startup.state !== 'running' && <p className={`startup-result${startupReady ? ' ready' : ''}`}><span aria-hidden="true">{startupReady ? '✓' : '!'}</span><span>{startupReady ? 'Session ready.' : launch.startup.state === 'cancelled' ? 'Startup cancelled.' : 'Session could not start.'}{launch.startup.saved && ' Configuration saved.'}</span></p>}</>}
                </div>
              </>}
            </div>
            {(!startupLogShown || completingStartup) && <div className="empty-state startup-idle" aria-hidden={completingStartup || undefined}>
              {completingStartup ? <><img src={flame} alt="" /><strong>Ready when you are.</strong><p>Send {agent.name} a message to begin.</p></> : emptyContent}
            </div>}
          </div>}
        </section>
        <aside className={`context-section${remaining != null && remaining < 15 ? ' low' : ''}`} aria-label="Context remaining"
          title={estimated ? "Estimated from Pi's completed compaction. Updated when context usage is available." : undefined}>
          <h2>Context</h2><div className={`beer-mug${compacting ? ' compacting' : remaining === null ? ' unknown' : remaining === 0 ? ' empty' : ''}`} role="meter" aria-label="Context remaining" aria-valuemin={0} aria-valuemax={100} aria-valuenow={remaining ?? undefined} aria-valuetext={remaining === null ? compacting ? 'Compacting' : 'Unavailable' : `${estimated ? 'Approximately ' : ''}${Math.round(remaining)}% remaining${estimated ? ' (estimated after compaction)' : ''}`}>
            <div className="mug-handle" /><div className="glass"><div className="liquid" style={{ height: `${(remaining ?? 0) * .82}%` } as CSSProperties}><div className="beer-foam" /></div><div className="glass-rim" /><div className="glass-shine" /><div className="glass-foot" /></div><div className="glass-shadow" />
          </div>{compacting ? <span className="compacting-caption">Compacting</span> : <>
            <strong className="context-number">{remaining === null ? '—' : <>{estimated && '~'}{Math.round(remaining)}<span>%</span></>}</strong><span className="remaining">remaining</span>
            <span className="context-tokens">{remaining != null && info.context ? `${estimated ? '~' : ''}${Math.round(info.context.contextWindow * remaining / 100).toLocaleString()} / ${info.context.contextWindow.toLocaleString()}` : contextStatus}</span>
          </>}
          {info.autoCompaction === false && <small className="auto-compaction-off">Auto compaction off</small>}
          <div className="session-actions"><button className="session-button" disabled={needsSignIn || modelRequired || launchBusy || modelSaving} onClick={() => onFresh(agentId)}>New session</button></div>
        </aside>
      </div>
      <section ref={queue} className="queue-section" tabIndex={-1} aria-label="Queued messages">
        <div className="section-heading"><h2>Queue <span>{view?.queueTotal ?? 0}</span></h2>{queueController &&
          <button className="queue-add" disabled={needsSignIn || modelRequired || launchBusy || busy || !!draft || !!addDraft || editPending || reorder.pending || reorder.dragging} aria-expanded={!!addDraft}
            onClick={() => queueController.add(projectRoot, agentId)}><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 4v12M4 10h12" /></svg>Add</button>}
        </div>
        {(queueError || reorder.error) && <p className="workbench-error" role="status">{reorder.error || queueError}</p>}
        <div ref={reorder.list} className="queue-list" aria-busy={reorder.pending}>{queueItems.map((item) => <QueueRow key={`${scopeKey}:${item.kotaId}`} item={item} rowProps={reorder.rowProps(item)} grip={reorder.grip(item)} sender={item.senderId ? agents[item.senderId]?.name ?? item.senderId : undefined} disabled={busy || !connected || reorder.pending || reorder.dragging} editDisabled={!!addDraft}
          operation={view ? queueController?.pending.get(queueController.itemKey(view.scope, item)) : undefined}
          onRemove={view && queueController ? () => void queueController.cancel(view.scope, item, false) : undefined}
          onEdit={view && queueController && localQueueCanEdit(item) ? () => void queueController.cancel(view.scope, item, true) : undefined}
          expand={(kotaId) => control({ type: 'queue_text', kotaId })} />)}
          {!queueItems.length && <p className="queue-empty">No queued messages.</p>}
          {(view?.queueTotal ?? 0) > queueItems.length && <p className="stream-limit">Showing the first {queueItems.length} queued messages.</p>}
        </div>
        {draft && queueController && <LocalQueueEditor key={draft.instance} draft={draft} queue={queueController} agents={agents} targets={targets} />}
        {addDraft && queueController && <LocalQueueEditor key={addDraft.instance} draft={addDraft} queue={queueController} agents={agents} targets={targets}
          direct={!working && (view?.queueTotal ?? 0) === 0} privateMessage={privateMessage} />}
      </section>
    </div>
    {reorder.ghost}
    <footer className="bench-footer"><span><kbd>Esc</kbd> Hide</span></footer>
  </section>;
}
