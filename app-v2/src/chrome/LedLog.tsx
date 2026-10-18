import { memo, useCallback, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import '../styles/string-route-log.css';

export interface LedLogEvent { id: number; occurredAt?: string }
export interface LedLogRow { tone: string; type: string; text: string }
const ROW_HEIGHT = 36;
const VISIBLE_ROWS = 5;
function EventTime({ value }: { value?: string }) {
  const date = value ? new Date(value) : null;
  if (!date || !Number.isFinite(date.getTime())) return <span className="string-log-time">—</span>;
  const seconds = date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
  const hundredths = /\.\d{2,}(?:Z|[+-]\d{2}:?\d{2})$/.test(value!) ? `.${String(Math.floor(date.getMilliseconds() / 10)).padStart(2, '0')}` : '';
  return <time className="string-log-time" dateTime={value} title={`${date.toLocaleString(undefined, { timeZoneName: 'short' })} · ${value}`}>{seconds}{hundredths}</time>;
}
const EventRow = memo(function EventRow({ id, occurredAt, tone, type, text }: LedLogEvent & LedLogRow) {
  return <li className={`string-log-row type-${tone}`} data-event-id={id}>
    <EventTime value={occurredAt} /><span className="string-log-type">{type}</span>
    <span className="string-log-message" title={text}>{text.replace(/\r\n?|\n/g, ' ↵ ')}</span>
  </li>;
});

/** A bounded, newest-first Rust log. Reading/painting never creates an event or timestamp. */
export function LedLog<T extends LedLogEvent>({ events, format, label, emptyText, footer }: {
  events: T[]; format: (event: T) => LedLogRow; label: string; emptyText: string; footer: ReactNode;
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const lines = useRef<HTMLUListElement>(null);
  const previous = useRef<number[]>([]);
  const count = useRef(0);
  const [unseen, setUnseen] = useState(0);
  const [arrows, setArrows] = useState([false, false, false, false]);
  const syncScroll = useCallback(() => {
    const el = viewport.current;
    if (!el) return;
    const next = [el.scrollTop > 1, el.scrollTop + el.clientHeight < el.scrollHeight - 1, el.scrollLeft > 1, el.scrollLeft + el.clientWidth < el.scrollWidth - 1];
    setArrows(before => before.every((value, i) => value === next[i]) ? before : next);
    if (el.scrollTop <= 1) { count.current = 0; setUnseen(0); }
  }, []);
  const fit = useCallback(() => {
    const el = viewport.current, list = lines.current;
    if (!el || !list) return;
    const top = el.scrollTop, left = el.scrollLeft;
    const height = ROW_HEIGHT * (events.length ? Math.min(VISIBLE_ROWS, events.length) : 3);
    el.style.overflowY = events.length > VISIBLE_ROWS ? 'auto' : 'hidden';
    el.style.height = `${height}px`;
    let width = el.clientWidth;
    const first = Math.max(0, Math.floor(top / ROW_HEIGHT));
    const last = Math.min(events.length, Math.ceil((top + height) / ROW_HEIGHT));
    for (let i = first; i < last; i++) {
      const row = list.children[i] as HTMLElement;
      const style = getComputedStyle(row);
      const cells = Array.from(row.children);
      const content = cells.reduce((sum, cell) => sum + cell.getBoundingClientRect().width, 0);
      width = Math.max(width, content + (parseFloat(style.columnGap) || 0) * (cells.length - 1)
        + (parseFloat(style.paddingLeft) || 0) + (parseFloat(style.paddingRight) || 0));
    }
    // Clip offscreen rows so an older long error doesn't force an unnecessary horizontal bar.
    list.style.width = `${Math.ceil(width)}px`;
    const bar = el.offsetHeight - el.clientHeight;
    el.style.height = `${height + bar}px`;
    el.scrollTop = top; el.scrollLeft = left;
    syncScroll();
  }, [events.length, syncScroll]);

  useLayoutEffect(() => {
    const el = viewport.current;
    if (!el) return;
    const before = previous.current, top = el.scrollTop, left = el.scrollLeft;
    const anchor = before[Math.floor(top / ROW_HEIGHT)];
    const added = before.length ? events.filter(event => event.id > before[0]).length : 0;
    if (top > 1 && added) {
      const index = events.findIndex(event => event.id === anchor);
      // Grow scroll extent before setting a new anchor. fit() then adjusts visible-row width.
      el.scrollTop = index >= 0 ? index * ROW_HEIGHT + top % ROW_HEIGHT : el.scrollHeight;
      count.current += added; setUnseen(count.current);
    } else if (top <= 1) el.scrollTop = 0;
    el.scrollLeft = left;
    previous.current = events.map(event => event.id);
    fit();
  }, [events, fit]);
  useLayoutEffect(() => {
    const el = viewport.current;
    if (!el) return;
    const observer = new ResizeObserver(fit);
    observer.observe(el);
    let active = true;
    void document.fonts?.ready.then(() => { if (active) fit(); });
    return () => { active = false; observer.disconnect(); };
  }, [fit]);

  return <section className="string-log" aria-label={`${label} log`}>
    <div ref={viewport} className="string-log-viewport" role="region" tabIndex={0} aria-label={`${label} events`} onScroll={fit}>
      <ul ref={lines} className="string-log-lines">{events.map(event => <EventRow key={event.id} id={event.id} occurredAt={event.occurredAt} {...format(event)} />)}</ul>
      {!events.length && <div className="string-log-empty">{emptyText}</div>}
    </div>
    <div className="string-log-footer">
      {footer}
      {arrows[0] && <button className="string-log-latest" onClick={() => { viewport.current!.scrollTop = 0; fit(); }}>{unseen ? `${unseen} new ↑` : 'Latest ↑'}</button>}
      <span className="string-log-arrows" aria-hidden="true">{['↑', '↓', '←', '→'].map((symbol, i) => <span key={symbol} className={arrows[i] ? 'available' : ''}>{symbol}</span>)}</span>
    </div>
  </section>;
}
