import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { sameLocalScope, type LocalScope } from '../lib/local-rpc';
import { cloneIdentity, controlLocalShadowClone, readLocalShadowClones, type LocalShadowClone, type LocalShadowClones } from '../lib/local-shadow-clone';
import { LocalStreamBlock, localElapsedLabel } from './LocalStreamBlock';
import kage from '../assets/shadow-clones/kage-bunshin.webp';
import kambei from '../assets/shadow-clones/kambei.webp';
import gorobei from '../assets/shadow-clones/gorobei.webp';
import shichiroji from '../assets/shadow-clones/shichiroji.webp';
import heihachi from '../assets/shadow-clones/heihachi.webp';
import kyuzo from '../assets/shadow-clones/kyuzo.webp';
import katsushiro from '../assets/shadow-clones/katsushiro.webp';
import kikuchiyo from '../assets/shadow-clones/kikuchiyo.webp';
import '../styles/local-shadow-clone.css';

const portraits = [kambei, gorobei, shichiroji, heihachi, kyuzo, katsushiro, kikuchiyo];
const labels = { running: 'Running', finished: 'Idle', stopped: 'Stopped' };

function Monitor({ clone, now, id }: { clone: LocalShadowClone; now: number; id: string }) {
  const scroll = useRef<HTMLDivElement>(null), follow = useRef(true), frame = useRef<number>();
  const [following, setFollowing] = useState(true);
  const blocks = clone.view?.blocks ?? [];
  useLayoutEffect(() => {
    if (follow.current && !document.hidden) frame.current = requestAnimationFrame(() => {
      if (scroll.current && follow.current) scroll.current.scrollTop = scroll.current.scrollHeight;
    });
    return () => { if (frame.current !== undefined) cancelAnimationFrame(frame.current); };
  }, [clone.view?.revision]);
  return <div id={id} className="clone-monitor" role="region" aria-label={`Clone #${clone.id} monitor`}><div className="activity-frame"><div ref={scroll} className="activity-scroll" tabIndex={0} aria-label={`Clone #${clone.id} output`}
    onScroll={event => { const element = event.currentTarget; follow.current = element.scrollHeight - element.scrollTop - element.clientHeight < 24; setFollowing(follow.current); }}>
    {!!clone.view?.dropped && <p className="stream-limit">Showing the latest display blocks. Room history is unchanged.</p>}
    {blocks.map(block => <LocalStreamBlock key={`${clone.view?.runId}:${block.id}`} block={block} now={block.kind === 'thinking' && block.active ? now : undefined} />)}
    {!blocks.length && <p className="stream-limit">Waiting for output…</p>}
  </div>{!following && <button className="latest-button" onClick={() => { follow.current = true; setFollowing(true); if (scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight; }}>Latest ↓</button>}</div></div>;
}

/** Reads in-memory copies only. Shares the workbench clock, never starts a helper. */
export function LocalShadowClonePanel({ projectRoot, agentId, scope, revision, now, onActivityChange }: {
  projectRoot: string; agentId: string; scope?: LocalScope; revision?: number; now: number; onActivityChange: (active: boolean) => void;
}) {
  const [view, setView] = useState<LocalShadowClones | null>(null);
  const [expanded, setExpanded] = useState(false), [selected, setSelected] = useState<number>();
  const [error, setError] = useState(false), [pending, setPending] = useState<number>();
  const [actionError, setActionError] = useState<number>();
  const section = useRef<HTMLElement>(null), trigger = useRef<HTMLButtonElement>(null), drawer = useRef<HTMLDivElement>(null);
  const dismissed = useRef<{ id: number; next?: number; workbench?: HTMLElement | null }>();
  const generation = useRef(0), flight = useRef<object | null>(null), current = useRef({ scope, selected, expanded });
  current.current = { scope, selected, expanded };
  const identity = JSON.stringify([projectRoot, agentId, scope?.sessionId, scope?.epoch]);
  const panelId = useId();
  const fitDrawer = useCallback(() => {
    const element = drawer.current;
    if (element) {
      const panel = section.current?.closest('.local-workbench')?.getBoundingClientRect();
      const bottom = Math.min(window.innerHeight, panel?.bottom ?? window.innerHeight);
      element.style.maxHeight = `${Math.max(0, bottom - element.getBoundingClientRect().top - 12)}px`;
    }
  }, []);
  useLayoutEffect(() => { if (expanded) fitDrawer(); }, [expanded, now, fitDrawer]);
  useEffect(() => {
    if (!expanded) return;
    window.addEventListener('resize', fitDrawer);
    window.addEventListener('scroll', fitDrawer, true);
    return () => { window.removeEventListener('resize', fitDrawer); window.removeEventListener('scroll', fitDrawer, true); };
  }, [expanded, fitDrawer]);

  useEffect(() => {
    generation.current++; flight.current = null; dismissed.current = undefined; setView(null); setSelected(undefined); setError(false); setPending(undefined); setActionError(undefined); setExpanded(false);
    return () => { generation.current++; };
  }, [identity]);
  useEffect(() => { onActivityChange(expanded || !!view?.clones.some(clone => clone.state === 'running')); }, [expanded, view, onActivityChange]);
  useEffect(() => { if (view && !view.clones.length) { setExpanded(false); setSelected(undefined); } }, [view]);
  useLayoutEffect(() => {
    if (!view) return;
    if (selected !== undefined && !view.clones.some(clone => clone.id === selected)) setSelected(undefined);
    const removed = dismissed.current;
    if (!removed || view.clones.some(clone => clone.id === removed.id)) return;
    dismissed.current = undefined;
    // Restore keyboard focus after the confirmed removal, unless the user
    // has already moved to another control or closed the drawer.
    if (!expanded || document.activeElement !== document.body) return;
    const next = section.current?.querySelector<HTMLButtonElement>(`[data-clone-monitor="${removed.next}"]`);
    const target = next ?? (view.clones.length ? trigger.current : removed.workbench);
    target?.focus();
  }, [view, selected, expanded]);
  useEffect(() => () => onActivityChange(false), [onActivityChange]);
  useEffect(() => {
    drawer.current?.toggleAttribute('inert', !expanded);
    if (!expanded) return;
    const outside = (event: PointerEvent) => { if (!section.current?.contains(event.target as Node)) setExpanded(false); };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault(); event.stopPropagation(); setExpanded(false); trigger.current?.focus();
    };
    document.addEventListener('pointerdown', outside); document.addEventListener('keydown', escape, true);
    return () => { document.removeEventListener('pointerdown', outside); document.removeEventListener('keydown', escape, true); };
  }, [expanded]);
  const refresh = useCallback(async () => {
    if (document.hidden || flight.current) return;
    const token = {}, epoch = generation.current, requested = current.current;
    flight.current = token;
    try {
      const result = await readLocalShadowClones(projectRoot, agentId, requested.expanded ? requested.selected : undefined);
      if (epoch !== generation.current) return;
      if (result && requested.scope && !sameLocalScope(result.scope, requested.scope)) { setError(true); return; }
      setView(result); setError(false);
    } catch { if (epoch === generation.current) setError(true); }
    finally {
      if (flight.current === token) {
        flight.current = null;
        // A click during a read must still fetch its chosen monitor. Merge
        // those changes into one follow-up, without another timer or helper.
        if (epoch === generation.current && (current.current.selected !== requested.selected || current.current.expanded !== requested.expanded)) void refresh();
      }
    }
  }, [identity]);
  useEffect(() => { void refresh(); }, [refresh, revision, now, expanded, selected]);
  useEffect(() => {
    const visible = () => { if (!document.hidden) void refresh(); };
    document.addEventListener('visibilitychange', visible);
    return () => document.removeEventListener('visibilitychange', visible);
  }, [refresh]);

  async function act(clone: LocalShadowClone, button: HTMLButtonElement) {
    if (!view || pending !== undefined) return;
    const epoch = generation.current;
    setPending(clone.id); setActionError(undefined);
    try {
      await controlLocalShadowClone(view.scope, clone.id, clone.state === 'stopped' ? 'destroy' : 'stop');
      if (epoch === generation.current) {
        if (clone.state === 'stopped' && document.activeElement === button) {
          const index = view.clones.findIndex(row => row.id === clone.id);
          dismissed.current = { id: clone.id, next: (view.clones[index + 1] ?? view.clones[index - 1])?.id,
            workbench: section.current?.closest<HTMLElement>('.local-workbench') };
        }
        await refresh();
      }
    } catch { if (epoch === generation.current) setActionError(clone.id); }
    finally { if (epoch === generation.current) setPending(undefined); }
  }
  const rows = view?.clones ?? [];
  if (!rows.length) return null;
  const running = rows.filter(clone => clone.state === 'running').length;
  return <section ref={section} className={`clone-section${expanded ? ' is-open' : ''}`} aria-label="Kage Bunshin">
    <button ref={trigger} className="mcp-tab clone-tab" aria-label={`Kage Bunshin, ${rows.length} ${rows.length === 1 ? 'clone' : 'clones'}`} aria-expanded={expanded} aria-controls={panelId} onClick={() => setExpanded(value => !value)}>
      <svg className="mcp-tab-arrow" viewBox="0 0 16 16" aria-hidden="true"><path d="m10 4-4 4 4 4" /></svg>
      <img className="kage-icon generated-icon" src={kage} width={21} height={21} alt="" /><span>Kage Bunshin</span><span className="mcp-count">{rows.length}</span>
    </button>
    <div ref={drawer} id={panelId} className="clone-drawer" role={expanded ? 'dialog' : undefined} aria-label="Kage Bunshin" aria-hidden={!expanded}>
      <div className="clone-drawer-top"><div className="clone-drawer-identity"><img className="kage-icon generated-icon" src={kage} width={32} height={32} alt="" /><div><h2>Kage Bunshin</h2><p>{rows.length} {rows.length === 1 ? 'clone' : 'clones'} · {running} running</p></div></div>
        <button className="mcp-close" aria-label="Close Kage Bunshin" tabIndex={expanded ? 0 : -1} onClick={() => { setExpanded(false); trigger.current?.focus(); }}><svg viewBox="0 0 16 16" aria-hidden="true"><path d="m6 4 4 4-4 4" /></svg></button></div>
      {error && <div className="mcp-read-error" role="status"><span>Status unavailable</span><button className="mcp-retry" onClick={() => void refresh()} tabIndex={expanded ? 0 : -1}>Retry</button></div>}
      <div className="clone-list">{rows.map(clone => <article key={clone.id} className={`clone-item${selected === clone.id ? ' selected' : ''}`}>
        <div className="clone-row">
          <button className="clone-open" data-clone-monitor={clone.id} tabIndex={expanded ? 0 : -1} aria-expanded={selected === clone.id} aria-controls={`${panelId}-clone-${clone.id}`} aria-label={`Monitor ${cloneIdentity(clone.id)}, Clone #${clone.id}`} onClick={() => setSelected(value => value === clone.id ? undefined : clone.id)}>
            <svg className="clone-chevron" viewBox="0 0 10 10" aria-hidden="true"><path d="m3 2 3 3-3 3" /></svg><img className="samurai-avatar" src={portraits[(clone.id - 1) % 7]} width={36} height={36} alt="" loading="lazy" />
            <span className="clone-identity"><strong>{cloneIdentity(clone.id)}</strong><small>Clone #{clone.id}</small></span></button>
          <span className={`clone-state ${clone.state}`}><i aria-hidden="true" />{labels[clone.state]}</span>
          <time className="clone-elapsed">{clone.state === 'running' ? localElapsedLabel(clone.startedAt, now) : localElapsedLabel('1970-01-01T00:00:00Z', clone.elapsedMs)}</time>
          <button className={`clone-stop${clone.state === 'stopped' ? ' clone-dismiss' : ''}`} tabIndex={expanded ? 0 : -1} disabled={pending !== undefined}
            aria-label={`${clone.state === 'stopped' ? 'Dismiss' : 'Stop'} Clone #${clone.id}`} title={clone.state === 'stopped' ? 'Delete this clone and its records' : undefined} onClick={event => void act(clone, event.currentTarget)}>
            {pending === clone.id ? '…' : clone.state === 'stopped' ? 'Dismiss' : 'Stop'}</button>
        </div>
        {actionError === clone.id && <p className="mcp-remove-error" role="alert">Couldn't update this clone. Try again.</p>}
        {selected === clone.id && expanded && <Monitor id={`${panelId}-clone-${clone.id}`} clone={clone} now={now} />}
      </article>)}</div>
    </div>
  </section>;
}
