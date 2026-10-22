import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { useLocalModelConfiguration } from '../lib/local-model-configuration';
import { controlLocalWorkbench, localSnapshot, setOfflineLocalModel } from '../lib/local-rpc';
import { useTavernLocal } from '../lib/tavern-local';
import { useJevSettings } from '../lib/jev';
import { catalogModel, requestKotaLocalLink } from '../lib/local-auth';
import { AUTO_ROUTE_MODEL, stringErrorText, type AutoRouteGroup, type AutoRouteModel } from '../lib/string-routing';
import { StringLocalModelPicker } from './StringLocalModelPicker';
import { localModelName, LocalProviderIcon } from './LocalModelPicker';
import { LocalModelCredit } from './LocalModelCredit';
import '../styles/composer-model.css';

/** Memory belongs to this App instance, not to a Pi session or saved SHELL. */
export interface ComposerModelMemory {
  choices: Map<string, AutoRouteModel>;
  current: { projectRoot: string; agentId: string; routeModel?: string } | null;
}
const compact = (id: string) => [...id].length > 48 ? `…${[...id].slice(-48).join('')}` : id;
const caption = (model: AutoRouteModel) => `${model.modelId.split('/').at(-1)} • ${model.thinkingLevel}`;

export function ComposerModelControl({ projectRoot, agentId, name, memory }: {
  projectRoot: string; agentId: string; name: string; memory: ComposerModelMemory;
}) {
  const key = JSON.stringify([projectRoot, agentId]);
  const configuration = useLocalModelConfiguration(projectRoot, agentId);
  const local = useTavernLocal(true), jev = useJevSettings(true);
  const [choice, setChoice] = useState(() => memory.choices.get(key));
  const [saving, setSaving] = useState(false), [error, setError] = useState<string | null>(null);
  const alive = useRef(true), operating = useRef(false);
  const current = useRef(configuration); current.current = configuration;
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const model = configuration.value ? `${configuration.value.provider}/${configuration.value.id}` : '';
  const group = configuration.value?.autoRoute;
  const automatic = model === AUTO_ROUTE_MODEL;
  const configured = configuration.value;
  const entry = configured && catalogModel(local.catalog.data, configured.provider, configured.id);
  const ordinaryLabel = !automatic && configured?.thinking
    ? `${entry ? localModelName(configured.provider, entry.name, entry.modelId) : model} • ${configured.thinking}`
    : undefined;
  // An edited/reordered group must not silently make a remembered id mean a different model.
  const selected = automatic ? group?.models.find(item => item.id === choice?.id && item.providerId === choice.providerId
    && item.modelId === choice.modelId && item.thinkingLevel === choice.thinkingLevel) : undefined;
  useLayoutEffect(() => {
    memory.current = { projectRoot, agentId, ...(selected ? { routeModel: selected.id } : {}) };
    if (configuration.loaded && !configuration.error && !selected) { memory.choices.delete(key); setChoice(undefined); }
    return () => { if (memory.current?.projectRoot === projectRoot && memory.current.agentId === agentId) memory.current = null; };
  }, [projectRoot, agentId, key, memory, selected, configuration.loaded, configuration.error]);

  const save = useCallback(async (provider: string, modelId: string, autoRoute?: AutoRouteGroup) => {
    if (operating.current) throw { code: 'local_busy' };
    operating.current = true; setSaving(true); setError(null);
    try {
      // Read only on a user action. Online writes carry the session fence;
      // offline writes use the existing saved-model command and never launch Pi.
      const live = await localSnapshot(agentId).catch(() => null);
      if (live && ['ready', 'busy'].includes(live.state)) {
        if (live.scope.projectRoot !== projectRoot) throw { code: 'local_session_changed' };
        await controlLocalWorkbench(live.scope, { type: 'set_model', provider, modelId, ...(autoRoute ? { autoRoute } : {}) });
      } else await setOfflineLocalModel(projectRoot, agentId, provider, modelId, autoRoute);
      const saved = await current.current.read(true);
      return saved?.autoRoute;
    } catch (failure) {
      if (alive.current) setError(stringErrorText(failure));
      throw failure;
    } finally { operating.current = false; if (alive.current) setSaving(false); }
  }, [projectRoot, agentId]);
  return <div className="composer-model-controls" aria-label="Composer model" data-testid="composer-model-controls">
    <div className="composer-model-big">
      <StringLocalModelPicker ownerKey={key} owner={name} surface="workbench" model={model} group={group}
        triggerLabel={selected ? 'Auto Off' : ordinaryLabel} catalog={local.catalog.data} jev={jev}
        disabled={!configuration.loaded || !!configuration.error || saving}
        applySource={{ projectRoot, agentId }} ruleFile={{ projectRoot, agentId, kind: 'string' }}
        onSelect={next => { void save(next.providerId, next.modelId).catch(() => {}); }}
        onAutoRouteChange={next => save('kota', 'auto', next)}
        onOpen={() => void local.openModels()} onJevSettings={() => requestKotaLocalLink('puppeteer-jev')} />
    </div>
    {automatic && group && <ComposerRouteChoice key={key} models={group.models} selected={selected?.id}
      disabled={saving || !!configuration.error || jev.loading || jev.failed || !jev.settings?.ready}
      onChoose={next => {
        if (next) memory.choices.set(key, next); else memory.choices.delete(key);
        memory.current = { projectRoot, agentId, ...(next ? { routeModel: next.id } : {}) }; setChoice(next);
      }} />}
    {(error || !!configuration.error) && <span className="composer-model-error" role="status">{error || stringErrorText(configuration.error)}
      {configuration.error ? <button type="button" className="local-model-text" onClick={() => void configuration.read(true).catch(() => {})}>Retry</button> : null}
    </span>}
  </div>;
}

function ComposerRouteChoice({ models, selected, disabled, onChoose }: {
  models: AutoRouteModel[]; selected?: string; disabled: boolean; onChoose: (model?: AutoRouteModel) => void;
}) {
  const trigger = useRef<HTMLButtonElement>(null), closeTimer = useRef<ReturnType<typeof setTimeout>>();
  const [anchor, setAnchor] = useState<DOMRect | null>(null), [keyboard, setKeyboard] = useState(false);
  const model = models.find(item => item.id === selected);
  const enter = () => clearTimeout(closeTimer.current);
  const close = useCallback((restore = false) => { clearTimeout(closeTimer.current); setAnchor(null); if (restore) trigger.current?.focus(); }, []);
  const leave = () => { enter(); closeTimer.current = setTimeout(() => close(), 240); };
  const open = (focus = false) => { if (disabled || !trigger.current) return; enter(); setAnchor(trigger.current.getBoundingClientRect()); setKeyboard(focus); };
  useEffect(() => () => clearTimeout(closeTimer.current), []);
  useEffect(() => { if (disabled) close(); }, [disabled, close]);
  useEffect(() => {
    if (!anchor) return;
    const dismiss = () => close();
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopImmediatePropagation(); close(true); }
    };
    window.addEventListener('keydown', escape, true);
    window.addEventListener('resize', dismiss);
    window.addEventListener('scroll', dismiss, true);
    return () => { window.removeEventListener('keydown', escape, true); window.removeEventListener('resize', dismiss); window.removeEventListener('scroll', dismiss, true); };
  }, [anchor, close]);
  return <>
    <button ref={trigger} type="button" className="composer-model-small" disabled={disabled}
      aria-haspopup="menu" aria-expanded={!!anchor} aria-label={model ? `Manual model: ${caption(model)}` : 'Auto String routing: Auto'} title={model ? caption(model) : 'Auto'}
      onPointerEnter={() => open()} onPointerLeave={leave} onClick={() => anchor && keyboard ? close() : open(true)}
      onKeyDown={event => { if (event.key === 'ArrowUp' || event.key === 'ArrowDown') { event.preventDefault(); open(true); } }}>
      {model ? <span>{compact(model.modelId.split('/').at(-1)!)} • {model.thinkingLevel}</span> : <span className="composer-auto-on">on</span>}
    </button>
    {anchor && <ComposerRouteFan anchor={anchor} models={models} selected={selected} focusFirst={keyboard} trigger={trigger}
      onEnter={enter} onLeave={leave} onChoose={next => { onChoose(next); close(true); }} onClose={() => close(true)} />}
  </>;
}

function ComposerRouteFan({ anchor, models, selected, focusFirst, trigger, onChoose, onEnter, onLeave, onClose }: {
  anchor: DOMRect; models: AutoRouteModel[]; selected?: string; focusFirst: boolean; trigger: RefObject<HTMLButtonElement>;
  onChoose: (model?: AutoRouteModel) => void; onEnter: () => void; onLeave: () => void; onClose: () => void;
}) {
  const menu = useRef<HTMLDivElement>(null), [hover, setHover] = useState<string | null>(null);
  const right = anchor.left < Math.min(360, innerWidth / 2);
  const left = right ? Math.max(12, anchor.left - 8) : Math.max(12, anchor.right - Math.min(440, anchor.right - 12) + 42);
  const width = Math.min(440, innerWidth - left - 12), bottom = Math.max(10, innerHeight - anchor.top - 4);
  const step = Math.min(56, Math.max(28, (anchor.top - 24) / (models.length + 1)));
  const items = [...models, undefined];
  useEffect(() => { if (focusFirst) menu.current?.querySelector<HTMLButtonElement>('button')?.focus(); }, [focusFirst]);
  useEffect(() => {
    const outside = (event: PointerEvent) => { if (!menu.current?.contains(event.target as Node) && !trigger.current?.contains(event.target as Node)) onClose(); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [onClose, trigger]);
  return createPortal(<div ref={menu} className={`composer-route-fan${right ? ' opens-right' : ''}`} role="menu" aria-label="Auto String models"
    style={{ left, bottom, width, height: items.length * step + 20, '--fan-text-width': `${width - 70}px` } as CSSProperties}
    onPointerEnter={onEnter} onPointerLeave={onLeave} onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) onLeave(); }}
    onKeyDown={event => {
      if (!['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault();
      const buttons = [...menu.current!.querySelectorAll<HTMLButtonElement>('button')], at = buttons.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : (at + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length;
      buttons[next]?.focus();
    }}>
    <div className="composer-fan-bridge" aria-hidden="true" />
    {items.map((model, index) => {
      const id = model?.id ?? 'auto', t = (items.length - 1 - index) / (items.length - 1);
      return <div key={id} className={`composer-fan-position${hover === id ? ' hovered' : ''}`}
        style={{ bottom: 12 + (items.length - 1 - index) * step, [right ? 'left' : 'right']: 8 + Math.round((1 - t) * 16), '--fan-angle': `${(right ? -1 : 1) * t * 8}deg` } as CSSProperties}>
        <button type="button" role="menuitemradio" aria-checked={selected === model?.id} className="composer-fan-item"
          aria-label={model ? `${model.providerId} / ${model.modelId} • ${model.thinkingLevel}` : 'Auto'}
          onPointerEnter={() => setHover(id)} onPointerLeave={() => setHover(null)} onFocus={() => { onEnter(); setHover(id); }} onBlur={() => setHover(null)} onClick={() => onChoose(model)}>
          <span className="composer-fan-short">{model ? `${compact(model.modelId)} • ${model.thinkingLevel}` : 'Auto'}</span>
          <span className="composer-fan-provider">{model ? <LocalProviderIcon providerId={model.providerId} /> : <span className="composer-auto-on">on</span>}</span>
        </button>
        {hover === id && model && <span className="composer-fan-full" aria-hidden="true" style={{ maxWidth: Math.max(120, width - 70) }}>{model.modelId} • {model.thinkingLevel} <LocalModelCredit providerId={model.providerId} /></span>}
      </div>;
    })}
  </div>, document.body);
}
