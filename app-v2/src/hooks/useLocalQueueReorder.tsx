import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import type { LocalQueueItem } from '../lib/local-rpc';

const changed = 'Queue changed. Try again.';
const rowsOf = (element: HTMLElement) => Array.from(element.children).filter((row): row is HTMLElement => row instanceof HTMLElement && row.classList.contains('queue-row'));
interface Drag {
  id: string; items: LocalQueueItem[]; from: number; to: number; min: number; max: number;
  pointer?: number; x: number; y: number; width: number; grip: HTMLButtonElement; captureLost?: boolean;
}

/** Preview stays separate from the authoritative list. Only the backend ACK
 * changes the ledger; the next normal workbench read supplies that order. */
export function useLocalQueueReorder({ items, scopeKey, identity, disabled, submit, refresh, renderGhost }: {
  items: LocalQueueItem[]; scopeKey: string; identity: string; disabled: boolean;
  submit: (ids: string[]) => Promise<unknown>; refresh: () => void; renderGhost?: (item: LocalQueueItem) => ReactNode;
}) {
  const list = useRef<HTMLDivElement>(null);
  const active = useRef<Drag | null>(null);
  const frame = useRef<number>();
  const current = useRef({ items, disabled, submit, refresh });
  current.current = { items, disabled, submit, refresh };
  const generation = useRef(0);
  const inFlight = useRef(false);
  const [pending, setPending] = useState(false);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [error, setError] = useState<string | null>(null);

  function release() {
    const value = active.current;
    active.current = null;
    if (frame.current !== undefined) cancelAnimationFrame(frame.current);
    frame.current = undefined;
    if (value?.pointer !== undefined && value.grip.hasPointerCapture?.(value.pointer)) value.grip.releasePointerCapture(value.pointer);
    setDrag(null);
    return value;
  }
  function cancel(message?: string) {
    if (!active.current) return;
    release();
    if (message) setError(message);
  }
  async function finish() {
    const value = release();
    if (!value || value.from === value.to) return;
    const order = [...value.items];
    order.splice(value.to, 0, ...order.splice(value.from, 1));
    const ids = order.filter(item => !item.handed).map(item => item.kotaId);
    const gen = generation.current;
    inFlight.current = true; setPending(true); setError(null);
    try { await current.current.submit(ids); }
    catch (e) {
      if (gen === generation.current) setError(String(e) === 'queue_changed' ? changed : "Couldn't reorder messages. Try again.");
    } finally {
      if (gen === generation.current) {
        inFlight.current = false; setPending(false); current.current.refresh();
        value.grip.isConnected && value.grip.focus({ preventScroll: true });
      }
    }
  }
  function position(x: number, y: number) {
    const value = active.current;
    const element = list.current;
    if (!value || !element) return false;
    const bounds = element.getBoundingClientRect();
    const inside = x >= bounds.left && x <= bounds.right && y >= bounds.top && y <= bounds.bottom;
    const rows = rowsOf(element);
    const remaining = rows.filter(row => row.dataset.queueId !== value.id);
    let target = remaining.findIndex(row => { const r = row.getBoundingClientRect(); return y < r.top + r.height / 2; });
    if (target < 0) target = remaining.length;
    value.to = Math.min(value.max, Math.max(value.min, target));
    value.x = x; value.y = y;
    setDrag({ ...value });
    return inside;
  }
  const events = useRef({ position, cancel, finish });
  events.current = { position, cancel, finish };
  function followEdge() {
    const value = active.current;
    const element = list.current;
    if (!value || !element || value.pointer === undefined) return;
    // Chrome can dispatch lostpointercapture just before pointerup. Allow
    // that terminal event; abandon on the next frame if it never arrives.
    if (value.captureLost) { events.current.cancel(); return; }
    const r = element.getBoundingClientRect();
    if (value.x >= r.left && value.x <= r.right && value.y >= r.top && value.y <= r.bottom) {
      const delta = value.y < r.top + 28 ? -6 : value.y > r.bottom - 28 ? 6 : 0;
      if (delta) { element.scrollTop += delta; events.current.position(value.x, value.y); }
    }
    frame.current = requestAnimationFrame(followEdge);
  }
  function begin(item: LocalQueueItem, grip: HTMLButtonElement, pointer?: number, x = 0, y = 0) {
    const { items, disabled } = current.current;
    if (disabled || inFlight.current || item.handed || active.current) return;
    const from = items.findIndex(row => row.kotaId === item.kotaId);
    if (from < 0) return;
    let min = from, max = from;
    while (min > 0 && !items[min - 1].handed) min--;
    while (max < items.length - 1 && !items[max + 1].handed) max++;
    if (min === max) return;
    const value: Drag = { id:item.kotaId, items, from, to:from, min, max, pointer, x, y,
      width:list.current?.getBoundingClientRect().width ?? 300, grip };
    active.current = value; setDrag({ ...value }); setError(null);
    grip.focus({ preventScroll: true });
    if (pointer !== undefined) { grip.setPointerCapture?.(pointer); frame.current = requestAnimationFrame(followEdge); }
  }

  // Scope, membership, status and any pending edit/removal invalidate previews.
  useLayoutEffect(() => { cancel(changed); }, [identity, disabled]);
  useLayoutEffect(() => { ++generation.current; inFlight.current = false; setPending(false); setError(null); }, [scopeKey]);
  useEffect(() => {
    const move = (event: PointerEvent) => {
      if (active.current?.pointer !== event.pointerId) return;
      event.preventDefault(); events.current.position(event.clientX, event.clientY);
    };
    const up = (event: PointerEvent) => {
      if (active.current?.pointer !== event.pointerId) return;
      // Also compute from release coordinates: a fast native drag can skip
      // the final buttons=1 pointermove in Chrome. A native drag can
      // also deliver its last move with buttons=0; only terminal events end it.
      if (events.current.position(event.clientX, event.clientY)) void events.current.finish();
      else events.current.cancel();
    };
    const lost = (event: PointerEvent) => { if (active.current?.pointer === event.pointerId) active.current.captureLost = true; };
    const stop = () => events.current.cancel();
    const key = (event: KeyboardEvent) => { if (event.key === 'Escape' && active.current) { event.preventDefault(); event.stopPropagation(); stop(); } };
    document.addEventListener('pointermove', move, { passive:false });
    document.addEventListener('pointerup', up); document.addEventListener('pointercancel', stop);
    document.addEventListener('lostpointercapture', lost); document.addEventListener('keydown', key, true);
    window.addEventListener('blur', stop); document.addEventListener('visibilitychange', stop);
    return () => {
      ++generation.current; release();
      document.removeEventListener('pointermove', move); document.removeEventListener('pointerup', up);
      document.removeEventListener('pointercancel', stop); document.removeEventListener('lostpointercapture', lost);
      document.removeEventListener('keydown', key, true); window.removeEventListener('blur', stop);
      document.removeEventListener('visibilitychange', stop);
    };
  }, []);

  const remainder = drag?.items.filter(item => item.kotaId !== drag.id);
  const before = drag && drag.to !== drag.from ? remainder?.[drag.to]?.kotaId : undefined;
  const after = drag && drag.to !== drag.from && !before ? remainder?.at(-1)?.kotaId : undefined;
  const moving = drag?.items[drag.from];
  return {
    list, pending, dragging:!!drag, error, cancel,
    rowProps: (item: LocalQueueItem) => ({ 'data-queue-id':item.kotaId,
      className:`queue-row${drag?.id === item.kotaId ? ' drag-source' : ''}${before === item.kotaId ? ' drop-before' : ''}${after === item.kotaId ? ' drop-after' : ''}` }),
    grip: (item: LocalQueueItem) => <span className="queue-grip-slot">{!item.handed && <button type="button" className="queue-grip"
      aria-label="Drag to reorder" aria-pressed={drag?.id === item.kotaId} title="Drag to reorder" disabled={disabled || pending}
      onBlur={() => { if (active.current?.id === item.kotaId) cancel(); }}
      onPointerDown={event => { if (event.button === 0) { event.preventDefault(); begin(item, event.currentTarget, event.pointerId, event.clientX, event.clientY); } }}
      onKeyDown={event => {
        if (!active.current && event.key === ' ') { event.preventDefault(); begin(item, event.currentTarget); }
        else if (active.current?.id === item.kotaId && active.current.pointer === undefined) {
          const value = active.current;
          if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
            event.preventDefault(); value.to = Math.min(value.max, Math.max(value.min, value.to + (event.key === 'ArrowUp' ? -1 : 1)));
            setDrag({ ...value });
            const row = list.current ? rowsOf(list.current)[value.to] : undefined;
            row?.scrollIntoView?.({ block:'nearest' });
          } else if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); void finish(); }
        }
      }}>{Array.from({ length:6 }, (_, i) => <i key={i} aria-hidden="true" />)}</button>}</span>,
    ghost: drag?.pointer !== undefined && moving ? createPortal(<div className="local-queue-drag-ghost" aria-hidden="true"
      style={{ left:drag.x + 10, top:drag.y + 10, width:drag.width }}>{renderGhost ? renderGhost(moving) : <span>{moving.summary || 'No text content'}</span>}</div>, document.body) : null,
  };
}
