import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { loadAccountUserIdentity, readVioletRoomCache, type AccountUserIdentity, type VioletChatMessage } from '../pty-client';
import type { Agent, AgentId } from '../types/scene';
import type { RoomQuoteInsertResult, RoomQuoteReference } from '../lib/room-quote';
import { collapseNativeBroadcastUserMessages, VioletMessageBubble } from './VioletRoomPanel';
import { RoomSearchIcon } from './room-search-elements';
import { useRoomSearch } from './useRoomSearch';
import '../styles/room-search.css';

interface Props {
  projectRoot: string;
  projectName?: string | null;
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  onQuoteMessage?: (quote: RoomQuoteReference) => RoomQuoteInsertResult;
  onClose: () => void;
}

export function VioletRoomSearch({ projectRoot, projectName, agentMeta, onQuoteMessage, onClose }: Props) {
  const [query, setQuery] = useState('');
  const [humanOnly, setHumanOnly] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [contextId, setContextId] = useState<string | null>(null);
  const [human, setHuman] = useState<AccountUserIdentity>({ name: 'User' });
  const [notice, setNotice] = useState<string | null>(null);
  const modal = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const restoreList = useRef(0);
  const { result, loading, error, loadMore, retry } = useRoomSearch(projectRoot, query, humanOnly);
  const edges = useScrollEdges(list, result, !contextId);
  const grouped = useMemo(() => collapseSearchMessages(result?.hits ?? []), [result?.hits]);
  const hits = grouped.messages;
  const selected = grouped.resolveId(selectedId);
  // One local-date reference for every result in this render; no midnight timer.
  const resultNow = new Date();

  useEffect(() => {
    let alive = true;
    void loadAccountUserIdentity().then((identity) => { if (alive) setHuman(identity); }).catch(() => {});
    return () => { alive = false; };
  }, []);

  useLayoutEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    // Same shade boundary as BBS: keep the project bar and global shortcuts available.
    const workspace = document.querySelector<HTMLElement>('.workspace');
    const dialog = modal.current;
    const wasInert = workspace?.inert;
    if (workspace) workspace.inert = true;
    input.current?.focus();
    return () => {
      if (workspace) workspace.inert = wasInert ?? false;
      if (previous?.isConnected && dialog?.contains(document.activeElement)) previous.focus({ preventScroll: true });
    };
  }, []);

  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(null), 2200);
    return () => window.clearTimeout(timer);
  }, [notice]);

  const back = () => {
    setContextId(null);
    setNotice(null);
  };
  useEffect(() => {
    // The global project bar remains usable above the shade. Escape still belongs
    // to this modal if focus moved there without changing the current project.
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented || event.isComposing) return;
      if (event.target instanceof Node && modal.current?.contains(event.target)) return;
      if (event.target instanceof Element && event.target.closest('[role="dialog"], [role="menu"]')) return;
      event.preventDefault();
      event.stopPropagation();
      if (contextId) { setContextId(null); setNotice(null); } else onClose();
    };
    window.addEventListener('keydown', escape);
    return () => window.removeEventListener('keydown', escape);
  }, [contextId, onClose]);
  useLayoutEffect(() => {
    if (contextId || !list.current) return;
    list.current.scrollTop = restoreList.current;
    if (selected) {
      [...list.current.querySelectorAll<HTMLElement>('[data-search-message-id]')]
        .find((row) => row.dataset.searchMessageId === selected)?.focus({ preventScroll: true });
    }
    // Selection changes while browsing must not restore an older scroll position.
  }, [contextId]);

  const openContext = (id: string) => {
    restoreList.current = list.current?.scrollTop ?? 0;
    setSelectedId(id);
    setContextId(id);
  };
  const changeQuery = (value: string) => {
    setQuery(value);
    setSelectedId(null);
    restoreList.current = 0;
    if (list.current) list.current.scrollTop = 0;
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      if (contextId) back(); else onClose();
      return;
    }
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'f') {
      event.preventDefault();
      event.stopPropagation();
      if (contextId) back();
      window.requestAnimationFrame(() => input.current?.focus());
      return;
    }
    if (event.key === 'Tab') {
      const controls = [...(modal.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), a[href], [tabindex="0"]') ?? [])]
        .filter((element) => !element.closest('[hidden]'));
      const first = controls[0];
      const last = controls[controls.length - 1];
      if (event.shiftKey && (document.activeElement === first || document.activeElement === modal.current)) {
        event.preventDefault(); last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault(); first?.focus();
      }
    }
    // Cmd+9 deliberately continues to App's existing window handler.
    if (contextId || event.metaKey || event.ctrlKey || event.altKey || loading) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!hits.length) return;
      const current = hits.findIndex((hit) => hit.id === selected);
      if (current === -1 && event.key === 'ArrowUp') return;
      const next = current === -1 ? 0 : Math.max(0, Math.min(hits.length - 1, current + (event.key === 'ArrowDown' ? 1 : -1)));
      setSelectedId(hits[next].id);
      const row = list.current?.querySelectorAll<HTMLElement>('[data-search-message-id]')[next];
      row?.scrollIntoView({ block: 'nearest' });
      if (event.target !== input.current) row?.focus({ preventScroll: true });
    } else if (event.key === 'Enter' && (event.target === input.current || event.target === list.current)) {
      event.preventDefault();
      if (selected) openContext(selected);
    }
  };
  const quote = (reference: RoomQuoteReference) => {
    const outcome = onQuoteMessage?.(reference) ?? 'blocked';
    setNotice(outcome === 'inserted' ? null : outcome === 'limit' ? '4 quotes max' : outcome === 'duplicate' ? 'Already quoted' : 'This quote cannot be used in the current project or recipient scope');
  };

  return createPortal(
    <div className="room-search-shade">
      <div ref={modal} className="room-search-modal" role="dialog" aria-modal="true" aria-label="Search room history" onKeyDown={onKeyDown}>
        <header className="room-search-header">
          <span className="room-search-title-icon"><RoomSearchIcon /></span>
          <div><h2>Room search</h2><span>{projectName || 'Current project'} · Local history</span></div>
          <button type="button" className="room-search-close" aria-label="Close search" onClick={onClose}>×</button>
        </header>
        <section className="room-search-view" hidden={!!contextId} aria-label="Search results">
          <div className="room-search-controls">
            <div className="room-search-input-row">
              <RoomSearchIcon />
              <input ref={input} type="search" aria-label="Search messages" aria-describedby="room-search-phrase-hint" placeholder="Search messages…" value={query} onChange={(event) => changeQuery(event.target.value)} />
              {query && <button type="button" aria-label="Clear query" onClick={() => { changeQuery(''); input.current?.focus(); }}>×</button>}
            </div>
            <p className="room-search-hint" id="room-search-phrase-hint">Use straight double quotes for a phrase, e.g. <code>"red fox"</code></p>
            <div className="room-search-options">
              <label><input type="checkbox" checked={humanOnly} onChange={(event) => {
                setHumanOnly(event.target.checked); setSelectedId(null); restoreList.current = 0;
                if (list.current) list.current.scrollTop = 0;
              }} /> Human Msg Only</label>
              <span>↑ ↓ Select · Enter Context · Esc Close</span>
            </div>
          </div>
          <div className="room-search-count" role="status">
            <span>{loading ? 'Searching…' : result?.total !== undefined ? `${result.total.toLocaleString()} matching records` : result ? 'Matching messages · count unavailable' : 'Search this project’s history'}</span>
            <span>Newest first · 50 per page</span>
          </div>
          <div ref={list} className="room-search-results-scroll" onScroll={edges.measure} tabIndex={0} aria-label="Result list" aria-busy={loading}>
            <div>
              {error && <div className="room-search-error" role="alert">{error} <button type="button" onClick={retry} disabled={loading}>Try again</button></div>}
              {hits.length > 0 && <ol className="room-search-results">{hits.map((message) => <li key={message.id}>
                <VioletMessageBubble message={message} projectRoot={projectRoot} agent={agentMeta?.[message.agentId]} agentMeta={agentMeta} humanTargetName={human.name} searchResult={{ human, now: resultNow, terms: result?.matchTerms ?? [], selected: selected === message.id, onOpen: () => openContext(message.id) }} />
              </li>)}</ol>}
              {!hits.length && !error && <div className="room-search-empty">
                <RoomSearchIcon />
                <p>{loading ? 'Looking through local history…' : query.trim() ? 'No matching messages' : 'Find a message in this room'}</p>
                <span>{query.trim() ? 'Try another word or turn off Human Msg Only.' : 'Words match together. Use "double quotes" for a phrase.'}</span>
              </div>}
              {result?.nextCursor && <div className="room-search-boundary">{edges.bottom && <button type="button" disabled={loading} onClick={loadMore}>{loading ? 'Loading…' : 'Load more results'}</button>}</div>}
            </div>
          </div>
        </section>
        {contextId && <section className="room-search-view" aria-label="Message context">
          <nav className="room-search-context-nav"><button type="button" autoFocus onClick={back}>‹ Back to results</button><span>Message context</span><span>Esc Back to results</span></nav>
          <RoomSearchContext key={contextId} projectRoot={projectRoot} id={contextId} agentMeta={agentMeta} humanName={human.name} onQuote={quote} />
        </section>}
        {notice && <div className="violet-room-quote-toast" role="status">{notice}</div>}
      </div>
    </div>, document.body,
  );
}

function collapseSearchMessages(messages: readonly VioletChatMessage[]) {
  const collapsed = collapseNativeBroadcastUserMessages(messages);
  const byId = new Map(collapsed.map((message) => [message.id, message]));
  const originals = new Map(messages.map((message) => [message.id, message]));
  return {
    // Keep the backend's ordering, including same-timestamp ties. The room
    // helper chooses each group's representative in chronological order.
    messages: messages.flatMap((message) => {
      const representative = byId.get(message.id);
      return representative ? [representative] : [];
    }),
    resolveId(id: string | null): string | null {
      if (id === null || byId.has(id)) return id;
      const original = originals.get(id);
      if (!original) return id;
      // Reuse the exact room predicate/window, without a second grouping rule.
      // Chronological representatives ensure the first matching group wins.
      return collapsed.find((message) => collapseNativeBroadcastUserMessages([message, original]).length === 1)?.id ?? id;
    },
  };
}

function useScrollEdges(ref: RefObject<HTMLDivElement>, content: unknown, visible = true) {
  const [edges, setEdges] = useState({ top: false, bottom: false });
  const measure = useCallback(() => {
    const element = ref.current;
    if (!element || !visible) return;
    // The boundary occupies normal flow. Reveal its control only as it enters view.
    const top = element.scrollTop < 72;
    const bottom = element.scrollHeight - element.scrollTop - element.clientHeight < 72;
    setEdges((previous) => previous.top === top && previous.bottom === bottom ? previous : { top, bottom });
  }, [ref, visible]);
  useLayoutEffect(() => {
    measure();
    if (!ref.current || !visible || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(ref.current);
    if (ref.current.firstElementChild) observer.observe(ref.current.firstElementChild);
    return () => observer.disconnect();
  }, [ref, content, visible, measure]);
  return { ...edges, measure };
}

type Anchor = { id: string; top: number };

function RoomSearchContext({ projectRoot, id, agentMeta, humanName, onQuote }: {
  projectRoot: string;
  id: string;
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  humanName: string;
  onQuote: (quote: RoomQuoteReference) => void;
}) {
  const [messages, setMessages] = useState<VioletChatMessage[]>([]);
  const [target, setTarget] = useState(id);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [ends, setEnds] = useState({ before: false, after: false });
  const alive = useRef(true);
  const busy = useRef(false);
  const scroll = useRef<HTMLDivElement>(null);
  const anchor = useRef<Anchor | 'target' | null>('target');
  const centering = useRef(true);
  const edges = useScrollEdges(scroll, messages);
  const grouped = useMemo(() => collapseSearchMessages(messages), [messages]);
  const visibleMessages = grouped.messages;
  const visibleTarget = grouped.resolveId(target);

  useEffect(() => {
    let current = true;
    alive.current = true;
    void readVioletRoomCache({ projectRoot, around: { id, before: 15, after: 15 } }).then((state) => {
      if (!current) return;
      setTarget(state.resolvedTargetId ?? id);
      setMessages(state.messages);
    }).catch((reason: unknown) => { if (current) setError(String(reason)); })
      .finally(() => { if (current) setLoading(false); });
    return () => { current = false; alive.current = false; };
  }, [id, projectRoot]);

  useLayoutEffect(() => {
    const element = scroll.current;
    if (!element || !messages.length || !anchor.current) return;
    const nodes = [...element.querySelectorAll<HTMLElement>('[data-violet-message-id]')];
    const saved = anchor.current;
    const node = nodes.find((item) => item.dataset.violetMessageId === (saved === 'target' ? visibleTarget : grouped.resolveId(saved.id)));
    if (node) {
      const top = node.getBoundingClientRect().top - element.getBoundingClientRect().top;
      element.scrollTop += saved === 'target' ? top - (element.clientHeight - node.offsetHeight) / 2 : top - saved.top;
    }
    anchor.current = null;
    edges.measure();
  }, [messages, visibleTarget, grouped, edges.measure]);

  useLayoutEffect(() => {
    const element = scroll.current;
    if (!element?.firstElementChild || !messages.length || typeof ResizeObserver === 'undefined') return;
    // Images and quote/file previews settle asynchronously. Keep the initial target
    // centered until the user takes over; never pull a reader back after scrolling.
    const observer = new ResizeObserver(() => {
      if (!centering.current) return;
      const node = [...element.querySelectorAll<HTMLElement>('[data-violet-message-id]')]
        .find((item) => item.dataset.violetMessageId === visibleTarget);
      if (node) element.scrollTop += node.getBoundingClientRect().top - element.getBoundingClientRect().top - (element.clientHeight - node.offsetHeight) / 2;
      edges.measure();
    });
    observer.observe(element);
    observer.observe(element.firstElementChild);
    return () => observer.disconnect();
  }, [messages, visibleTarget, edges.measure]);

  const load = async (direction: 'before' | 'after') => {
    if (busy.current || loading || !messages.length) return;
    centering.current = false;
    busy.current = true;
    setLoading(true);
    setError(null);
    const edge = direction === 'before' ? visibleMessages[0] : visibleMessages[visibleMessages.length - 1];
    try {
      const state = await readVioletRoomCache({ projectRoot, around: { id: edge.id, before: direction === 'before' ? 15 : 0, after: direction === 'after' ? 15 : 0 } });
      if (!alive.current) return;
      const known = new Set(messages.map((message) => message.id));
      const added = state.messages.filter((message) => !known.has(message.id));
      if (!added.length) { setEnds((previous) => ({ ...previous, [direction]: true })); return; }
      const element = scroll.current;
      const top = element?.getBoundingClientRect().top ?? 0;
      const visible = [...(element?.querySelectorAll<HTMLElement>('[data-violet-message-id]') ?? [])]
        .find((node) => node.getBoundingClientRect().bottom > top);
      if (visible) anchor.current = { id: visible.dataset.violetMessageId!, top: visible.getBoundingClientRect().top - top };
      const merged = direction === 'before' ? [...added, ...messages] : [...messages, ...added];
      if (merged.length > 200) setEnds((previous) => ({ ...previous, [direction === 'before' ? 'after' : 'before']: false }));
      setMessages(direction === 'before' ? merged.slice(0, 200) : merged.slice(-200));
    } catch (reason) { if (alive.current) setError(String(reason)); }
    finally { busy.current = false; if (alive.current) setLoading(false); }
  };

  return <div ref={scroll} className="violet-room-scroll room-search-context-scroll" onScroll={edges.measure} aria-label="Context messages" aria-busy={loading} tabIndex={0}
    onWheel={() => { centering.current = false; }}
    onPointerDown={() => { centering.current = false; }}
    onTouchMove={() => { centering.current = false; }}
    onKeyDown={(event) => { if (['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '].includes(event.key)) centering.current = false; }}>
    <div>
      {error && <div className="room-search-error" role="alert">{error}</div>}
      {!messages.length && !error && <div className="room-search-empty"><p>Loading message context…</p></div>}
      {messages.length > 0 && <>
        <div className="room-search-boundary">{edges.top && (ends.before ? <span>Beginning of history</span> : <button type="button" disabled={loading} onClick={() => void load('before')}>Earlier 15 messages</button>)}</div>
        {visibleMessages.map((message) => <div key={message.id} className={message.id === visibleTarget ? 'room-search-context-target' : undefined}>
          <VioletMessageBubble message={message} projectRoot={projectRoot} agent={agentMeta?.[message.agentId]} agentMeta={agentMeta} humanTargetName={humanName} onQuoteMessage={onQuote} />
        </div>)}
        <div className="room-search-boundary">{edges.bottom && (ends.after ? <span>End of history</span> : <button type="button" disabled={loading} onClick={() => void load('after')}>Later 15 messages</button>)}</div>
      </>}
    </div>
  </div>;
}
