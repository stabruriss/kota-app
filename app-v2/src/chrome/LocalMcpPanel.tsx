import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { localMcpLabels, localMcpReasons, readLocalMcp, removeLocalMcp, type LocalMcpView } from '../lib/local-mcp';
import type { LocalScope } from '../lib/local-rpc';

/** A human-only drawer. Reuses the workbench clock; no hidden polling or reload. */
export function LocalMcpPanel({ projectRoot, agentId, scope, now }: {
  projectRoot: string; agentId: string; scope?: LocalScope; now: number;
}) {
  const identity = JSON.stringify([projectRoot,agentId,scope?.sessionId,scope?.epoch]);
  const [expanded,setExpanded] = useState(false);
  const [view,setView] = useState<LocalMcpView | null>(null);
  const [unavailable,setUnavailable] = useState(false);
  const [pending,setPending] = useState<string | null>(null);
  const [removeError,setRemoveError] = useState<string | null>(null);
  const section = useRef<HTMLElement>(null), trigger = useRef<HTMLButtonElement>(null);
  const drawer = useRef<HTMLDivElement>(null);
  const generation = useRef(0), active = useRef(false), revision = useRef(0);
  const flight = useRef<object | null>(null), removing = useRef(false);
  const panelId = useId();

  useEffect(() => {
    generation.current += 1; flight.current = null; removing.current = false;
    setView(null); setUnavailable(false); setPending(null); setRemoveError(null);
    return () => { generation.current += 1; };
  }, [identity]);
  useEffect(() => {
    active.current = expanded;
    drawer.current?.toggleAttribute('inert',!expanded);
    if (!expanded) return;
    const outside = (event: PointerEvent) => {
      if (!section.current?.contains(event.target as Node)) setExpanded(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault(); event.stopPropagation(); setExpanded(false); trigger.current?.focus();
    };
    document.addEventListener('pointerdown',outside);
    document.addEventListener('keydown',escape,true);
    return () => {
      active.current = false;
      document.removeEventListener('pointerdown',outside); document.removeEventListener('keydown',escape,true);
    };
  }, [expanded]);

  const refresh = useCallback(async () => {
    if (!active.current || document.hidden || flight.current || removing.current) return;
    const token = {}, epoch = generation.current, readRevision = revision.current;
    flight.current = token;
    try {
      const result = await readLocalMcp(projectRoot,agentId,scope);
      if (epoch !== generation.current || !active.current || readRevision !== revision.current) return;
      if (result.schema !== 1 || !result.available) { setUnavailable(true); return; }
      setView(result); setUnavailable(false);
    } catch {
      if (epoch === generation.current && active.current && readRevision === revision.current) setUnavailable(true);
    } finally { if (flight.current === token) flight.current = null; }
    // identity contains the complete scope; a new view object isn't a new query.
  }, [identity]);
  const pollTick = Math.floor(now / 2000);
  useEffect(() => { if (expanded && !unavailable) void refresh(); }, [expanded,refresh,pollTick]);

  async function remove(name: string) {
    if (removing.current) return;
    const epoch = generation.current;
    removing.current = true; setPending(name); setRemoveError(null);
    try {
      await removeLocalMcp(projectRoot,agentId,name);
      if (epoch !== generation.current) return;
      revision.current += 1; // Ignore a read that began before the saved removal.
      setView(current => current && ({ ...current, servers:current.servers.flatMap(row => row.name !== name ? [row]
        : row.status === 'connected' || row.status === 'connecting' ? [{ ...row,configured:false }] : []) }));
    } catch { if (epoch === generation.current) setRemoveError(name); }
    finally {
      if (epoch === generation.current) { removing.current = false; setPending(null); void refresh(); }
    }
  }

  const rows = view?.servers ?? [];
  const allConnected = rows.length > 0 && rows.every(row => row.status === 'connected');
  const attention = unavailable || rows.some(row => row.status === 'needs_auth' || row.status === 'failed');
  const close = () => { setExpanded(false); trigger.current?.focus(); };
  return <section ref={section} className={`mcp-section${expanded ? ' is-open' : ''}`} aria-label="MCP servers">
    <button ref={trigger} className="mcp-tab" aria-label="MCP servers" aria-expanded={expanded} aria-controls={panelId}
      title={unavailable ? 'Status unavailable' : !view ? 'MCP servers' : attention ? 'Needs attention' : allConnected ? 'Connected' : 'Not connected'} onClick={() => setExpanded(value => !value)}>
      <svg className="mcp-tab-arrow" viewBox="0 0 16 16" aria-hidden="true"><path d="m10 4-4 4 4 4" /></svg>
      <i className={attention ? 'attention' : allConnected ? 'connected' : ''} aria-hidden="true" />
      <span>MCP</span>{view && !unavailable && <span className="mcp-count">{rows.length}</span>}
    </button>
    <div ref={drawer} id={panelId} className="mcp-drawer" role={expanded ? 'dialog' : undefined} aria-label="MCP servers" aria-hidden={!expanded}>
      <div className="mcp-drawer-top">
        <p className="mcp-add-instruction">tell your agent to connect mcp for you</p>
        <button className="mcp-close" aria-label="Close MCP" title="Close MCP" tabIndex={expanded ? 0 : -1} onClick={close}><svg viewBox="0 0 16 16" aria-hidden="true"><path d="m6 4 4 4-4 4" /></svg></button>
      </div>
      {unavailable ? <div className="mcp-read-error" role="status"><span>Status unavailable</span><button className="mcp-retry" tabIndex={expanded ? 0 : -1} onClick={() => void refresh()}>Retry</button></div>
        : !view ? <p className="mcp-empty" role="status">Checking…</p>
        : rows.length === 0 ? <p className="mcp-empty">No MCP servers.</p>
        : <ul className="mcp-server-list" aria-label="Configured and active MCP servers">
          {rows.map(row => <li key={row.name} className="mcp-server-row" aria-busy={pending === row.name}>
            <div className="mcp-row-main">
              <strong className="mcp-name">{row.name}</strong>
              <span className={`mcp-health ${row.status}`}><i aria-hidden="true" />{localMcpLabels[row.status]}</span>
              {row.configured ? <button className="mcp-remove" tabIndex={expanded ? 0 : -1} disabled={!!pending} aria-label={`Remove ${row.name}`} onClick={() => void remove(row.name)}>{pending === row.name ? 'Removing…' : 'Remove'}</button>
                : <span className="mcp-removed" role="status">Removed</span>}
            </div>
            {row.reason && <p className="mcp-reason">{localMcpReasons[row.reason]}</p>}
            {removeError === row.name && <p className="mcp-remove-error" role="alert">Couldn't remove this server. Try again.</p>}
          </li>)}
        </ul>}
    </div>
  </section>;
}
