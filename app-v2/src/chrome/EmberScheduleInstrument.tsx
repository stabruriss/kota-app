/* ══════════════════════════════════════════════════════════════
   EmberScheduleInstrument — production TSX port of the confirmed Ember
   schedule mock (project-memory/scratch/ember-medal-mock).

   Embeddable block: owns its own vertical scroll, never goes full-screen.
   All styling lives in ../styles/ember-schedule.css (every class `esi-`).

   The footer action bar (Delete / Save / Schedule) is NOT part of this
   component — RightColumn renders it. This component only renders the
   schedule editor body + the send-out readout, and keeps that content
   scrollable so the host footer is never clipped.
   ══════════════════════════════════════════════════════════════ */
import { useEffect, useRef, useState } from 'react';
import type {
  ChangeEvent,
  CSSProperties,
  PointerEvent as ReactPointerEvent,
  ReactNode,
  WheelEvent as ReactWheelEvent,
} from 'react';
import '../styles/ember-schedule.css';

/* ────────────────────────────────────────────────────────────────
   Props — kept byte-for-byte in sync with the spec RightColumn passes.
   ──────────────────────────────────────────────────────────────── */
export interface EmberScheduleInstrumentProps {
  sendMode: 'idle' | 'delay' | 'at';
  setSendMode: (v: 'idle' | 'delay' | 'at') => void;
  delayHours: number;
  setDelayHours: (n: number) => void;
  delayMinutes: number;
  setDelayMinutes: (n: number) => void;
  atDate: string; // 'YYYY-MM-DD'
  setAtDate: (s: string) => void;
  atTime: string; // 'HH:MM' (24h)
  setAtTime: (s: string) => void;
  repeatEnabled: boolean;
  setRepeatEnabled: (b: boolean) => void;
  repeatKind: 'fixed' | 'weekly' | 'monthly';
  setRepeatKind: (k: 'fixed' | 'weekly' | 'monthly') => void;
  repDays: number;
  setRepDays: (n: number) => void;
  repHrs: number;
  setRepHrs: (n: number) => void;
  repMin: number;
  setRepMin: (n: number) => void;
  weekDays: number[]; // 0=Sun..6=Sat; UI starts on Monday
  setWeekDays: (f: (cur: number[]) => number[]) => void;
  everyNWeeks: number; // 1-50
  setEveryNWeeks: (n: number) => void;
  monthDays: string[]; // '1'..'31' | 'last'; multi-select
  setMonthDays: (f: (cur: string[]) => string[]) => void;
  everyNMonths: number; // 1-12, 100 = easter egg
  setEveryNMonths: (n: number) => void;
  endMode: 'never' | 'after' | 'at';
  setEndMode: (m: 'never' | 'after' | 'at') => void;
  endAfterCount: number;
  setEndAfterCount: (n: number) => void;
  endDate: string; // 'YYYY-MM-DD'
  setEndDate: (s: string) => void;
  endTime: string; // 'HH:MM' (24h)
  setEndTime: (s: string) => void;
}

/* ────────────────────────────────────────────────────────────────
   Helpers
   ──────────────────────────────────────────────────────────────── */
const pad = (n: number): string => String(n).padStart(2, '0');
const clamp = (v: number, lo: number, hi: number): number => Math.max(lo, Math.min(hi, v));
const wrap = (v: number, n: number): number => ((v % n) + n) % n;
const wrapRange = (v: number, lo: number, hi: number, dir: number): number =>
  lo + wrap(v - lo + dir, hi - lo + 1);
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const YEAR_NOW = new Date().getFullYear();

function daysInMonth(mo: number, y: number): number {
  return new Date(y, mo, 0).getDate(); // mo 1-12
}

function tzLabel(): string {
  const off = -new Date().getTimezoneOffset() / 60;
  const sign = off >= 0 ? '+' : '−';
  const whole = Math.trunc(Math.abs(off));
  const frac = Math.abs(off) % 1 ? ':30' : '';
  return `GMT${sign}${whole}${frac}`;
}

function fmtDur(totalMin: number): string {
  if (totalMin <= 0) return 'right away';
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  return [h ? `${h}h` : null, m ? `${m}m` : null].filter(Boolean).join(' ');
}

function fmtInterval(d: number, h: number, m: number): string {
  const parts = [
    d ? `${d} day${d > 1 ? 's' : ''}` : null,
    h ? `${h} hr${h > 1 ? 's' : ''}` : null,
    m ? `${m} min` : null,
  ].filter((x): x is string => Boolean(x));
  if (!parts.length) return 'day';
  if (parts.length === 1) return parts[0];
  return parts.slice(0, -1).join(', ') + ' and ' + parts.slice(-1);
}

/* Parse 'YYYY-MM-DD' → {y,mo,d}; fall back to today on any malformed input. */
function parseDateStr(s: string): { y: number; mo: number; d: number } {
  const m = /^(\d{4})-(\d{1,2})-(\d{1,2})$/.exec(s);
  if (m) return { y: +m[1], mo: +m[2], d: +m[3] };
  const now = new Date();
  return { y: now.getFullYear(), mo: now.getMonth() + 1, d: now.getDate() };
}
const fmtDateStr = (y: number, mo: number, d: number): string => `${y}-${pad(mo)}-${pad(d)}`;

/* Parse 'HH:MM' (24h) → {h,min}; fall back to current time. */
function parseTimeStr(s: string): { h: number; min: number } {
  const m = /^(\d{1,2}):(\d{1,2})$/.exec(s);
  if (m) return { h: clamp(+m[1], 0, 23), min: clamp(+m[2], 0, 59) };
  const now = new Date();
  return { h: now.getHours(), min: now.getMinutes() };
}
const fmtTimeStr = (h24: number, min: number): string => `${pad(h24)}:${pad(min)}`;

/* 24h → {h12, mer} and back. */
function to12(h24: number): { h12: number; mer: 'AM' | 'PM' } {
  const mer: 'AM' | 'PM' = h24 < 12 ? 'AM' : 'PM';
  let h12 = h24 % 12;
  if (!h12) h12 = 12;
  return { h12, mer };
}
const to24 = (h12: number, mer: 'AM' | 'PM'): number => (h12 % 12) + (mer === 'PM' ? 12 : 0);

/* ────────────────────────────────────────────────────────────────
   Inline icon set (Lucide-style, hand-rolled so React owns every node).
   ──────────────────────────────────────────────────────────────── */
type IconName =
  | 'moon' | 'timer' | 'clock' | 'chevronUp' | 'chevronDown' | 'chevronRight'
  | 'triRight' | 'arrowRight' | 'globe' | 'repeat' | 'infinity' | 'calendar';

interface IconProps {
  name: IconName;
  size?: number;
  className?: string;
  style?: CSSProperties;
}

function Icon({ name, size = 18, className = '', style }: IconProps) {
  const p = {
    fill: 'none',
    stroke: 'currentColor',
    strokeWidth: 1.75,
    strokeLinecap: 'round' as const,
    strokeLinejoin: 'round' as const,
  };
  const paths: Record<IconName, ReactNode> = {
    moon: <path {...p} d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9z" />,
    timer: (
      <g {...p}>
        <line x1="10" x2="14" y1="2" y2="2" />
        <line x1="12" x2="14.5" y1="14" y2="11.5" />
        <circle cx="12" cy="14" r="8" />
      </g>
    ),
    clock: (
      <g {...p}>
        <circle cx="12" cy="12" r="9" />
        <polyline points="12 7 12 12 15.5 14" />
      </g>
    ),
    chevronUp: <path {...p} d="m6 14 6-6 6 6" />,
    chevronDown: <path {...p} d="m6 10 6 6 6-6" />,
    chevronRight: <path {...p} d="m9 6 6 6-6 6" />,
    triRight: <polygon points="9 5 9 19 18 12" fill="currentColor" stroke="none" />,
    arrowRight: (
      <g {...p}>
        <line x1="4" y1="12" x2="20" y2="12" />
        <polyline points="14 6 20 12 14 18" />
      </g>
    ),
    globe: (
      <g {...p}>
        <circle cx="12" cy="12" r="9" />
        <path d="M3 12h18" />
        <path d="M12 3a14 14 0 0 1 0 18 14 14 0 0 1 0-18z" />
      </g>
    ),
    repeat: (
      <g {...p}>
        <path d="m17 2 4 4-4 4" />
        <path d="M3 11v-1a4 4 0 0 1 4-4h14" />
        <path d="m7 22-4-4 4-4" />
        <path d="M21 13v1a4 4 0 0 1-4 4H3" />
      </g>
    ),
    infinity: <path {...p} d="M6 16c3 0 4-8 7-8a4 4 0 0 1 0 8c-3 0-4-8-7-8a4 4 0 0 0 0 8z" />,
    calendar: (
      <g {...p}>
        <rect x="3" y="4.5" width="18" height="17" rx="2.5" />
        <line x1="16" x2="16" y1="2" y2="7" />
        <line x1="8" x2="8" y1="2" y2="7" />
        <line x1="3" x2="21" y1="10" y2="10" />
      </g>
    ),
  };
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      className={`esi-icon ${className}`.trim()}
      style={style}
      aria-hidden="true"
    >
      {paths[name]}
    </svg>
  );
}

/* ────────────────────────────────────────────────────────────────
   Toggle — physical sliding switch
   ──────────────────────────────────────────────────────────────── */
function Toggle({ on, onChange }: { on: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      type="button"
      className={'esi-toggle' + (on ? ' esi-on' : '')}
      role="switch"
      aria-checked={on}
      onClick={() => onChange(!on)}
    >
      <span className="esi-toggle-knob" />
    </button>
  );
}

/* ────────────────────────────────────────────────────────────────
   Stepper — LCD readout with press-and-hold studs + click-to-type
   ──────────────────────────────────────────────────────────────── */
interface StepperProps {
  value: number;
  unit?: string;
  onDelta: (dir: number) => void;
  dim?: boolean;
  pad?: boolean;
  min?: number;
  max?: number;
  setValue?: (n: number) => void;
  primary?: boolean;
  displayValue?: string;
}

function Stepper({
  value,
  unit,
  onDelta,
  dim = false,
  pad: doPad = true,
  min,
  max,
  setValue,
  primary = false,
  displayValue,
}: StepperProps) {
  const hold = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');

  function start(dir: number): void {
    onDelta(dir);
    let d = 330;
    const tick = (): void => {
      onDelta(dir);
      d = Math.max(45, d * 0.78);
      hold.current = setTimeout(tick, d);
    };
    hold.current = setTimeout(tick, 330);
  }
  function stop(): void {
    if (hold.current) clearTimeout(hold.current);
  }
  function beginEdit(): void {
    if (!setValue || dim) return;
    setDraft(String(value));
    setEditing(true);
  }
  function commit(): void {
    setEditing(false);
    let n = parseInt(draft, 10);
    if (Number.isNaN(n)) return;
    if (min != null) n = Math.max(min, n);
    if (max != null) n = Math.min(max, n);
    setValue?.(n);
  }

  const disp = displayValue != null ? displayValue : doPad ? pad(value) : String(value);

  return (
    <div className={'esi-stepper' + (dim ? ' esi-dim' : '') + (primary ? ' esi-primary' : '')}>
      <div
        className={'esi-lcd' + (setValue ? ' esi-editable' : '')}
        onClick={beginEdit}
        title={setValue ? 'Click to type' : undefined}
      >
        {editing ? (
          <input
            className="esi-lcd-input"
            type="text"
            inputMode="numeric"
            autoFocus
            value={draft}
            onChange={(e) => setDraft(e.target.value.replace(/[^0-9]/g, '').slice(0, 4))}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === 'Enter') commit();
              if (e.key === 'Escape') setEditing(false);
            }}
          />
        ) : (
          <span className={'esi-lcd-num' + (displayValue != null ? ' esi-egg' : '')}>{disp}</span>
        )}
        {unit && <span className="esi-lcd-unit">{unit}</span>}
      </div>
      <div className="esi-studs">
        <div
          className="esi-stud"
          onPointerDown={() => start(1)}
          onPointerUp={stop}
          onPointerLeave={stop}
          onPointerCancel={stop}
        >
          <Icon name="chevronUp" size={13} />
        </div>
        <div
          className="esi-stud"
          onPointerDown={() => start(-1)}
          onPointerUp={stop}
          onPointerLeave={stop}
          onPointerCancel={stop}
        >
          <Icon name="chevronDown" size={13} />
        </div>
      </div>
    </div>
  );
}

/* ────────────────────────────────────────────────────────────────
   SevenSeg — 7-segment LED display
   ──────────────────────────────────────────────────────────────── */
const SEG_MAP: Record<string, string> = {
  '0': 'abcdef', '1': 'bc', '2': 'abged', '3': 'abgcd', '4': 'fgbc',
  '5': 'afgcd', '6': 'afgedc', '7': 'abc', '8': 'abcdefg', '9': 'abcdfg',
};

function segPoly(name: string): string {
  const T = 5;
  const h = (x: number, y: number, L: number): string =>
    `${x},${y} ${x + T / 2},${y - T / 2} ${x + L - T / 2},${y - T / 2} ${x + L},${y} ${x + L - T / 2},${y + T / 2} ${x + T / 2},${y + T / 2}`;
  const v = (x: number, y: number, L: number): string =>
    `${x},${y} ${x - T / 2},${y + T / 2} ${x - T / 2},${y + L - T / 2} ${x},${y + L} ${x + T / 2},${y + L - T / 2} ${x + T / 2},${y + T / 2}`;
  const map: Record<string, string> = {
    a: h(3, 3, 20), g: h(3, 23, 20), d: h(3, 43, 20),
    f: v(3, 3, 20), b: v(23, 3, 20), e: v(3, 23, 20), c: v(23, 23, 20),
  };
  return map[name];
}

function SevenDigit({ ch }: { ch: string }) {
  const on = (SEG_MAP[ch] || '').split('');
  return (
    <svg className="esi-seg-digit" viewBox="0 0 26 46">
      {['a', 'b', 'c', 'd', 'e', 'f', 'g'].map((s) => (
        <polygon key={s} points={segPoly(s)} className={'esi-seg-poly ' + (on.includes(s) ? 'esi-on' : 'esi-off')} />
      ))}
    </svg>
  );
}

function SevenSeg({ value, color, digits = 2 }: { value: number; color: 'orange' | 'cyan'; digits?: number }) {
  const str = String(Math.max(0, value)).padStart(digits, '0');
  return (
    <div className={'esi-seven esi-' + color}>
      {str.split('').map((ch, i) => (
        <SevenDigit key={i} ch={ch} />
      ))}
    </div>
  );
}

/* ────────────────────────────────────────────────────────────────
   VSlider — vertical slider (drag / wheel)
   ──────────────────────────────────────────────────────────────── */
function VSlider({
  value,
  max,
  onChange,
  label,
  color,
}: {
  value: number;
  max: number;
  onChange: (n: number) => void;
  label: string;
  color: string;
}) {
  const trackRef = useRef<HTMLDivElement>(null);
  const frac = Math.max(0, Math.min(1, value / max));

  function setFromY(clientY: number): void {
    const el = trackRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const f = 1 - (clientY - r.top) / r.height;
    onChange(Math.round(Math.max(0, Math.min(1, f)) * max));
  }
  function onDown(e: ReactPointerEvent): void {
    e.preventDefault();
    setFromY(e.clientY);
    const move = (ev: PointerEvent): void => setFromY(ev.clientY);
    const up = (): void => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.style.cursor = '';
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    document.body.style.cursor = 'ns-resize';
  }
  function onWheel(e: ReactWheelEvent): void {
    onChange(Math.max(0, Math.min(max, value + (e.deltaY < 0 ? 1 : -1))));
  }

  return (
    <div className="esi-vsl" style={{ '--esi-vsl-color': color } as CSSProperties}>
      <div className="esi-vsl-label">{label}</div>
      <div className="esi-vsl-track" ref={trackRef} onPointerDown={onDown} onWheel={onWheel}>
        <div className="esi-vsl-tick" />
        <div className="esi-vsl-thumb" style={{ bottom: `${frac * 100}%` }} />
      </div>
    </div>
  );
}

/* ────────────────────────────────────────────────────────────────
   Wheel — spinner drum (drag w/ momentum, trackpad scroll, +/- hold).
   Generic over the value type so AM/PM (string) and numerics share one
   implementation, exactly like the mock.
   ──────────────────────────────────────────────────────────────── */
interface WheelProps<T> {
  label: string;
  value: T;
  onChange: (v: T) => void;
  format: (v: T) => string;
  stepFn: (v: T, dir: number) => T;
}

function Wheel<T>({ label, value, onChange, format, stepFn }: WheelProps<T>) {
  const rowH = 32;
  const stripRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const valRef = useRef<T>(value);
  valRef.current = value;
  const stepRef = useRef(stepFn);
  stepRef.current = stepFn;
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const holdRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const rafRef = useRef<number>(0);

  useEffect(() => () => cancelAnimationFrame(rafRef.current), []);

  function valAt(d: number): T {
    let v = value;
    if (d > 0) for (let i = 0; i < d; i++) v = stepFn(v, 1);
    else for (let i = 0; i < -d; i++) v = stepFn(v, -1);
    return v;
  }

  function rollAnim(dir: number): void {
    const strip = stripRef.current;
    if (!strip) return;
    cancelAnimationFrame(rafRef.current);
    strip.style.transition = 'none';
    strip.style.transform = `translateY(${dir > 0 ? rowH : -rowH}px)`;
    void strip.offsetHeight; // commit start frame
    strip.style.transition = 'transform 150ms cubic-bezier(0.22,0.61,0.36,1)';
    strip.style.transform = 'translateY(0px)';
  }
  function stepRoll(dir: number): void {
    const nv = stepRef.current(valRef.current, dir);
    valRef.current = nv; // optimistic so rapid presses compound before re-render
    onChangeRef.current(nv);
    rollAnim(dir);
  }

  function onDown(e: ReactPointerEvent): void {
    e.preventDefault();
    cancelAnimationFrame(rafRef.current);
    const strip = stripRef.current;
    if (!strip) return;
    strip.style.transition = 'none';
    let lastY = e.clientY;
    let dy = 0;
    let cur = valRef.current;
    let vel = 0;
    let lastT = performance.now();
    const move = (ev: PointerEvent): void => {
      const d = ev.clientY - lastY;
      lastY = ev.clientY;
      const now = performance.now();
      const dt = Math.max(1, now - lastT);
      lastT = now;
      vel = vel * 0.6 + ((d / dt) * 16) * 0.4;
      dy += d;
      let changed = false;
      while (dy >= rowH) { cur = stepRef.current(cur, -1); dy -= rowH; changed = true; }
      while (dy <= -rowH) { cur = stepRef.current(cur, 1); dy += rowH; changed = true; }
      if (changed) onChangeRef.current(cur);
      strip.style.transform = `translateY(${dy}px)`;
    };
    const settle = (): void => {
      strip.style.transition = 'transform 220ms cubic-bezier(0.22,0.61,0.36,1)';
      strip.style.transform = 'translateY(0px)';
    };
    const up = (): void => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.style.cursor = '';
      let v = vel;
      if (Math.abs(v) < 1.4) { settle(); return; }
      const spin = (): void => {
        v *= 0.94;
        dy += v;
        let changed = false;
        while (dy >= rowH) { cur = stepRef.current(cur, -1); dy -= rowH; changed = true; }
        while (dy <= -rowH) { cur = stepRef.current(cur, 1); dy += rowH; changed = true; }
        if (changed) onChangeRef.current(cur);
        strip.style.transform = `translateY(${dy}px)`;
        if (Math.abs(v) > 0.6) rafRef.current = requestAnimationFrame(spin);
        else settle();
      };
      rafRef.current = requestAnimationFrame(spin);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    document.body.style.cursor = 'grabbing';
  }

  /* trackpad / wheel — native non-passive listener so we can preventDefault */
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    let acc = 0;
    const th = 24;
    const onWheel = (e: WheelEvent): void => {
      e.preventDefault();
      cancelAnimationFrame(rafRef.current);
      acc += e.deltaY;
      let cur = valRef.current;
      let dir = 0;
      while (acc >= th) { cur = stepRef.current(cur, 1); acc -= th; dir = 1; }
      while (acc <= -th) { cur = stepRef.current(cur, -1); acc += th; dir = -1; }
      if (dir) { onChangeRef.current(cur); rollAnim(dir); }
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, []);

  function stopHold(): void {
    if (holdRef.current) clearTimeout(holdRef.current);
    window.removeEventListener('pointerup', stopHold);
    window.removeEventListener('pointercancel', stopHold);
  }
  function startHold(dir: number): void {
    stopHold();
    cancelAnimationFrame(rafRef.current);
    stepRoll(dir);
    let d = 300;
    const tick = (): void => {
      stepRoll(dir);
      d = Math.max(60, d * 0.82);
      holdRef.current = setTimeout(tick, d);
    };
    holdRef.current = setTimeout(tick, 300);
    window.addEventListener('pointerup', stopHold);
    window.addEventListener('pointercancel', stopHold);
  }

  const offs = [-2, -1, 0, 1, 2];
  return (
    <div className="esi-wheel-col">
      <div className="esi-wheel" ref={rootRef}>
        <div className="esi-wheel-view" onPointerDown={onDown}>
          <div className="esi-wheel-band" />
          <div className="esi-wheel-strip" ref={stripRef}>
            {offs.map((o) => (
              <div key={o} className={'esi-wheel-cell' + (o === 0 ? ' esi-cur' : '')}>
                {format(valAt(o))}
              </div>
            ))}
          </div>
        </div>
      </div>
      <div className="esi-wheel-step">
        <button
          type="button"
          className="esi-wstep"
          aria-label={`${label} forward`}
          onPointerDown={(e) => { e.preventDefault(); startHold(1); }}
          onPointerUp={stopHold}
          onPointerLeave={stopHold}
          onPointerCancel={stopHold}
        >
          <Icon name="chevronUp" size={13} />
        </button>
        <button
          type="button"
          className="esi-wstep"
          aria-label={`${label} back`}
          onPointerDown={(e) => { e.preventDefault(); startHold(-1); }}
          onPointerUp={stopHold}
          onPointerLeave={stopHold}
          onPointerCancel={stopHold}
        >
          <Icon name="chevronDown" size={13} />
        </button>
      </div>
    </div>
  );
}

/* ────────────────────────────────────────────────────────────────
   DateInput — typed MM/DD/YYYY, commits only valid future dates.
   Reads/writes a 'YYYY-MM-DD' string.
   ──────────────────────────────────────────────────────────────── */
function DateInput({
  value,
  active,
  onFocus,
  onCommit,
}: {
  value: string; // 'YYYY-MM-DD'
  active: boolean;
  onFocus: () => void;
  onCommit: (s: string) => void;
}) {
  const { y, mo, d } = parseDateStr(value);
  const fmt = (m: number, dd: number, yy: number): string => `${pad(m)}/${pad(dd)}/${yy}`;
  const [draft, setDraft] = useState(fmt(mo, d, y));
  const [bad, setBad] = useState(false);
  useEffect(() => {
    setDraft(fmt(mo, d, y));
  }, [mo, d, y]);

  function onType(e: ChangeEvent<HTMLInputElement>): void {
    const s = e.target.value.replace(/[^0-9]/g, '').slice(0, 8);
    let out = s;
    if (s.length > 4) out = `${s.slice(0, 2)}/${s.slice(2, 4)}/${s.slice(4)}`;
    else if (s.length > 2) out = `${s.slice(0, 2)}/${s.slice(2)}`;
    setDraft(out);
    setBad(false);
  }
  function commit(): void {
    const m = /^(\d{1,2})\/(\d{1,2})\/(\d{4})$/.exec(draft);
    if (m) {
      const MM = +m[1];
      const DD = +m[2];
      const YYYY = +m[3];
      const valid = MM >= 1 && MM <= 12 && DD >= 1 && DD <= daysInMonth(MM, YYYY);
      const future = new Date(YYYY, MM - 1, DD, 23, 59, 59).getTime() > Date.now();
      if (valid && future) {
        onCommit(fmtDateStr(YYYY, MM, DD));
        setBad(false);
        return;
      }
    }
    setDraft(fmt(mo, d, y)); // revert
    setBad(true);
    setTimeout(() => setBad(false), 900);
  }
  return (
    <span className={'esi-date-input' + (active ? ' esi-active' : '') + (bad ? ' esi-bad' : '')}>
      <Icon name="calendar" size={13} />
      <input
        type="text"
        inputMode="numeric"
        value={draft}
        placeholder="MM/DD/YYYY"
        onFocus={onFocus}
        onChange={onType}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
        }}
      />
    </span>
  );
}

/* ────────────────────────────────────────────────────────────────
   TimeInput — typed HH:MM (24h) for "Ends on". Plain native time-ish
   field; commits on blur, reverts on malformed input.
   ──────────────────────────────────────────────────────────────── */
function TimeInput({
  value,
  active,
  onFocus,
  onCommit,
}: {
  value: string; // 'HH:MM' 24h
  active: boolean;
  onFocus: () => void;
  onCommit: (s: string) => void;
}) {
  const { h, min } = parseTimeStr(value);
  const [draft, setDraft] = useState(fmtTimeStr(h, min));
  const [bad, setBad] = useState(false);
  useEffect(() => {
    setDraft(fmtTimeStr(h, min));
  }, [h, min]);

  function onType(e: ChangeEvent<HTMLInputElement>): void {
    const s = e.target.value.replace(/[^0-9]/g, '').slice(0, 4);
    const out = s.length > 2 ? `${s.slice(0, 2)}:${s.slice(2)}` : s;
    setDraft(out);
    setBad(false);
  }
  function commit(): void {
    const m = /^(\d{1,2}):(\d{1,2})$/.exec(draft);
    if (m) {
      const HH = +m[1];
      const MM = +m[2];
      if (HH >= 0 && HH <= 23 && MM >= 0 && MM <= 59) {
        onCommit(fmtTimeStr(HH, MM));
        setBad(false);
        return;
      }
    }
    setDraft(fmtTimeStr(h, min));
    setBad(true);
    setTimeout(() => setBad(false), 900);
  }
  return (
    <span className={'esi-date-input esi-time-input' + (active ? ' esi-active' : '') + (bad ? ' esi-bad' : '')}>
      <Icon name="clock" size={13} />
      <input
        type="text"
        inputMode="numeric"
        value={draft}
        placeholder="HH:MM"
        onFocus={onFocus}
        onChange={onType}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
        }}
      />
    </span>
  );
}

/* ────────────────────────────────────────────────────────────────
   Mode metadata
   ──────────────────────────────────────────────────────────────── */
const MODES: { id: 'idle' | 'delay' | 'at'; icon: IconName; name: string }[] = [
  { id: 'idle', icon: 'moon', name: 'When idle' },
  { id: 'delay', icon: 'timer', name: 'After delay' },
  { id: 'at', icon: 'clock', name: 'At a time' },
];

/* ════════════════════════════════════════════════════════════════
   EmberScheduleInstrument
   ════════════════════════════════════════════════════════════════ */
export function EmberScheduleInstrument(props: EmberScheduleInstrumentProps) {
  const {
    sendMode, setSendMode,
    delayHours, setDelayHours, delayMinutes, setDelayMinutes,
    atDate, setAtDate, atTime, setAtTime,
    repeatEnabled, setRepeatEnabled,
    repeatKind, setRepeatKind,
    repDays, setRepDays, repHrs, setRepHrs, repMin, setRepMin,
    weekDays, setWeekDays, everyNWeeks, setEveryNWeeks,
    monthDays, setMonthDays, everyNMonths, setEveryNMonths,
    endMode, setEndMode, endAfterCount, setEndAfterCount,
    endDate, setEndDate, endTime, setEndTime,
  } = props;

  const [repeatOpen, setRepeatOpen] = useState(false);
  const [runsExpanded, setRunsExpanded] = useState(false);
  const [monthExpanded, setMonthExpanded] = useState(false);
  const [endActive, setEndActive] = useState<'date' | 'time' | null>(null);

  // Derive wheel-facing values from the atDate/atTime strings.
  const { y: atY, mo: atMo, d: atD } = parseDateStr(atDate);
  const { h: atH24, min: atMin } = parseTimeStr(atTime);
  const { h12: atH12, mer: atMer } = to12(atH24);

  // The repeat drawer lives only under "At a time" — close it on mode leave.
  useEffect(() => {
    if (sendMode !== 'at') setRepeatOpen(false);
  }, [sendMode]);

  const repeating = repeatEnabled && sendMode === 'at';
  const stepMs = Math.max(60000, (repDays * 1440 + repHrs * 60 + repMin) * 60000);

  /* ── wheel write-backs (keep day/year/month coherent) ── */
  function commitDate(nextY: number, nextMo: number, nextD: number): void {
    const yy = clamp(nextY, YEAR_NOW, YEAR_NOW + 6);
    const mm = clamp(nextMo, 1, 12);
    const dd = clamp(nextD, 1, daysInMonth(mm, yy));
    setAtDate(fmtDateStr(yy, mm, dd));
  }
  const setWheelMonth = (mo: number): void => commitDate(atY, mo, atD);
  const setWheelDay = (d: number): void => commitDate(atY, atMo, d);
  const setWheelYear = (y: number): void => commitDate(y, atMo, atD);
  function commitTime(h12: number, min: number, mer: 'AM' | 'PM'): void {
    setAtTime(fmtTimeStr(to24(clamp(h12, 1, 12), mer), clamp(min, 0, 59)));
  }
  const setWheelHour = (h12: number): void => commitTime(h12, atMin, atMer);
  const setWheelMin = (min: number): void => commitTime(atH12, min, atMer);
  const setWheelMer = (mer: 'AM' | 'PM'): void => commitTime(atH12, atMin, mer);

  /* ── send-out helpers ── */
  function atDateObj(): Date {
    return new Date(atY, atMo - 1, atD, atH24, atMin);
  }
  function fmtRun(dt: Date): string {
    const { h12, mer } = to12(dt.getHours());
    return `${MONTHS[dt.getMonth()]} ${dt.getDate()}, ${dt.getFullYear()} · ${h12}:${pad(dt.getMinutes())} ${mer}`;
  }
  function runCount(limit: number): number {
    if (endMode === 'after') return Math.min(endAfterCount, limit);
    if (endMode === 'at') {
      const ed = parseDateStr(endDate);
      const et = parseTimeStr(endTime);
      const end = new Date(ed.y, ed.mo - 1, ed.d, et.h, et.min).getTime();
      let t = atDateObj().getTime();
      let n = 0;
      while (t <= end && n < limit) {
        n++;
        t += stepMs;
      }
      return Math.max(1, n);
    }
    return limit;
  }
  function buildRuns(n: number): Date[] {
    const base = atDateObj().getTime();
    const out: Date[] = [];
    for (let i = 0; i < n; i++) out.push(new Date(base + i * stepMs));
    return out;
  }
  function sendInText(): string {
    if (sendMode === 'idle') return 'Send when agents fall idle';
    if (sendMode === 'delay') {
      const totalMin = delayHours * 60 + delayMinutes;
      return totalMin <= 0 ? 'Send right away' : `Send in ${fmtDur(totalMin)}`;
    }
    const ms = atDateObj().getTime() - Date.now();
    if (ms <= 0) return 'Send now';
    const mins = Math.floor(ms / 60000);
    const d = Math.floor(mins / 1440);
    const h = Math.floor((mins % 1440) / 60);
    const m = mins % 60;
    const parts: string[] = [];
    if (d) parts.push(`${d} day${d > 1 ? 's' : ''}`);
    if (h) parts.push(`${h} hr${h > 1 ? 's' : ''}`);
    if (m || !parts.length) parts.push(`${m} min`);
    return `Send in ${parts.join(' ')}`;
  }

  /* ── small repeat tag (One-Time / Repeat) ── */
  const RepTag = () => (
    <button
      type="button"
      className={'esi-rep-tag' + (repeating ? ' esi-on' : '') + (repeatOpen ? ' esi-active' : '')}
      onClick={() => setRepeatOpen((o) => !o)}
      aria-expanded={repeatOpen}
    >
      <span className="esi-rep-tag-shell">
        <Icon name={repeating ? 'repeat' : 'arrowRight'} size={12} />
        <span>{repeating ? 'Repeat' : 'One-Time'}</span>
        <span className="esi-rep-tag-dot" />
      </span>
    </button>
  );
  const RunRow = ({ i, dt }: { i: number; dt: Date }) => (
    <div className="esi-run-row">
      <span className="esi-run-idx">{i}</span>
      <span className="esi-run-when">{fmtRun(dt)}</span>
    </div>
  );

  // First-3-runs preview: the linear stepMs cadence is exact for fixed
  // interval. For weekly/monthly it is only an estimate (cadence ≠ stepMs);
  // backend will provide precise counting — flagged inline as a note.
  // TODO(ember-backend): replace estimate rows for weekly/monthly with the
  // real occurrence list once server-side counting lands.
  const previewIsEstimate = repeating && repeatKind !== 'fixed';

  const dH = delayHours;
  const dM = delayMinutes;

  return (
    <div className="esi-root">
      <div className="esi-body">
        {/* ───── SEND WHEN ───── */}
        <div className="esi-section">
          <div className="esi-sec-head">
            <span className="esi-sec-label">
              <span className="esi-gly">◆</span> Send when
            </span>
          </div>
          <div className="esi-segmented">
            {MODES.map((m) => (
              <div
                key={m.id}
                className={'esi-seg' + (sendMode === m.id ? ' esi-active' : '')}
                onClick={() => setSendMode(m.id)}
              >
                <Icon name={m.icon} size={19} />
                <span className="esi-seg-name">{m.name}</span>
              </div>
            ))}
          </div>

          <div className="esi-mode-body" key={sendMode}>
            {sendMode === 'idle' && (
              <div className="esi-idle-card">
                <div className="esi-idle-gauge">
                  <div className="esi-idle-rings">
                    <span />
                    <span />
                  </div>
                  <div className="esi-idle-lamp" />
                </div>
                <div>
                  <div className="esi-idle-copy-t">Whenever the table goes quiet.</div>
                  <div className="esi-idle-copy-s">
                    Held until every target agent leaves its working state — then sent at once.
                  </div>
                </div>
              </div>
            )}

            {sendMode === 'delay' && (
              <div className="esi-delay2">
                <VSlider
                  value={dH}
                  max={24}
                  label="HR"
                  color="var(--brass-hi)"
                  onChange={(h) => {
                    setDelayHours(clamp(h, 0, 24));
                  }}
                />
                <div className="esi-seg-display">
                  <div className="esi-seg-col">
                    <SevenSeg value={dH} color="orange" />
                    <span className="esi-seg-unit">HRS</span>
                  </div>
                  <div className="esi-seg-colon">
                    <span />
                    <span />
                  </div>
                  <div className="esi-seg-col">
                    <SevenSeg value={dM} color="cyan" />
                    <span className="esi-seg-unit">MIN</span>
                  </div>
                </div>
                <VSlider
                  value={dM}
                  max={59}
                  label="MIN"
                  color="var(--sky-hi)"
                  onChange={(m) => {
                    setDelayMinutes(clamp(m, 0, 59));
                  }}
                />
              </div>
            )}

            {sendMode === 'at' && (
              <div className="esi-at-wrap">
                <div className="esi-tz-note">
                  <Icon name="globe" size={12} /> All times in{' '}
                  {Intl.DateTimeFormat().resolvedOptions().timeZone} · {tzLabel()}
                </div>
                <div className="esi-dtwheel">
                  <div className="esi-dt-group">
                    <div className="esi-dt-head">Date</div>
                    <div className="esi-wheel-grid">
                      <Wheel<number>
                        label="Month"
                        value={atMo}
                        onChange={setWheelMonth}
                        format={(v) => MONTHS[v - 1]}
                        stepFn={(v, d) => wrapRange(v, 1, 12, d)}
                      />
                      <span className="esi-wsep">–</span>
                      <Wheel<number>
                        label="Day"
                        value={atD}
                        onChange={setWheelDay}
                        format={(v) => pad(v)}
                        stepFn={(v, d) => wrapRange(v, 1, daysInMonth(atMo, atY), d)}
                      />
                      <span className="esi-wsep">–</span>
                      <Wheel<number>
                        label="Year"
                        value={atY}
                        onChange={setWheelYear}
                        format={(v) => String(v)}
                        stepFn={(v, d) => clamp(v + d, YEAR_NOW, YEAR_NOW + 6)}
                      />
                    </div>
                  </div>
                  <div className="esi-dt-group">
                    <div className="esi-dt-head">Time</div>
                    <div className="esi-wheel-grid">
                      <Wheel<number>
                        label="Hour"
                        value={atH12}
                        onChange={setWheelHour}
                        format={(v) => pad(v)}
                        stepFn={(v, d) => wrapRange(v, 1, 12, d)}
                      />
                      <span className="esi-wsep esi-colon">:</span>
                      <Wheel<number>
                        label="Minute"
                        value={atMin}
                        onChange={setWheelMin}
                        format={(v) => pad(v)}
                        stepFn={(v, d) => wrapRange(v, 0, 59, d)}
                      />
                      <span className="esi-wsep esi-dot">·</span>
                      <Wheel<'AM' | 'PM'>
                        label="AM / PM"
                        value={atMer}
                        onChange={setWheelMer}
                        format={(v) => v}
                        stepFn={(v) => (v === 'AM' ? 'PM' : 'AM')}
                      />
                    </div>
                  </div>
                </div>

                <div className={'esi-repeat-drawer' + (repeatOpen ? ' esi-open' : '')}>
                  <div className="esi-rd-head">
                    <Toggle on={repeatEnabled} onChange={setRepeatEnabled} />
                    <span className="esi-rd-title">Repeat</span>
                    <div className={'esi-rk-seg esi-head' + (repeatEnabled ? '' : ' esi-disabled')}>
                      {(
                        [
                          ['fixed', 'Fixed interval'],
                          ['weekly', 'Every week'],
                          ['monthly', 'Every month'],
                        ] as [EmberScheduleInstrumentProps['repeatKind'], string][]
                      ).map(([k, label]) => (
                        <button
                          key={k}
                          type="button"
                          className={'esi-rk-btn' + (repeatKind === k ? ' esi-on' : '')}
                          disabled={!repeatEnabled}
                          onClick={() => setRepeatKind(k)}
                        >
                          {label}
                        </button>
                      ))}
                    </div>
                    <button
                      type="button"
                      className="esi-rd-close"
                      onClick={() => setRepeatOpen(false)}
                      aria-label="collapse"
                    >
                      <Icon name="triRight" size={13} />
                    </button>
                  </div>

                  <div className={'esi-rd-body' + (repeatEnabled ? '' : ' esi-disabled')}>
                    <div className="esi-rd-block">
                      {repeatKind === 'fixed' && (
                        <>
                          <div className="esi-rd-label">Repeats every</div>
                          <div className="esi-interval-grid">
                            <div className="esi-unit-stack esi-primary">
                              <Stepper
                                value={repDays}
                                unit="d"
                                pad={false}
                                primary
                                min={0}
                                max={365}
                                setValue={(n) => setRepDays(clamp(n, 0, 365))}
                                onDelta={(d) => setRepDays(clamp(repDays + d, 0, 365))}
                              />
                              <span className="esi-stepper-label">Days</span>
                            </div>
                            <span className="esi-interval-plus">+</span>
                            <div className="esi-unit-stack esi-secondary">
                              <Stepper
                                value={repHrs}
                                unit="h"
                                min={0}
                                max={23}
                                setValue={(n) => setRepHrs(clamp(n, 0, 23))}
                                onDelta={(d) => setRepHrs(clamp(repHrs + d, 0, 23))}
                              />
                              <span className="esi-stepper-label">Hours</span>
                            </div>
                            <div className="esi-unit-stack esi-secondary">
                              <Stepper
                                value={repMin}
                                unit="m"
                                min={0}
                                max={59}
                                setValue={(n) => setRepMin(clamp(n, 0, 59))}
                                onDelta={(d) => setRepMin(clamp(repMin + d * 5, 0, 55))}
                              />
                              <span className="esi-stepper-label">Minutes</span>
                            </div>
                          </div>
                        </>
                      )}

                      {repeatKind === 'weekly' && (
                        <>
                          <div className="esi-rd-label">On these days</div>
                          <div className="esi-weekday-row">
                            {(
                              [
                                ['Mon', 1], ['Tue', 2], ['Wed', 3], ['Thu', 4],
                                ['Fri', 5], ['Sat', 6], ['Sun', 0],
                              ] as [string, number][]
                            ).map(([d, val]) => (
                              <button
                                key={val}
                                type="button"
                                className={'esi-weekday-chip' + (weekDays.includes(val) ? ' esi-on' : '')}
                                onClick={() =>
                                  setWeekDays((cur) =>
                                    cur.includes(val)
                                      ? cur.length > 1
                                        ? cur.filter((x) => x !== val)
                                        : cur
                                      : [...cur, val],
                                  )
                                }
                              >
                                {d}
                              </button>
                            ))}
                          </div>
                          <div className="esi-rd-label esi-gap">Repeats every</div>
                          <div className="esi-every-n">
                            <Stepper
                              value={everyNWeeks}
                              pad={false}
                              primary
                              min={1}
                              max={50}
                              setValue={(n) => setEveryNWeeks(clamp(n, 1, 50))}
                              onDelta={(d) => setEveryNWeeks(clamp(everyNWeeks + d, 1, 50))}
                            />
                            <span className="esi-every-n-unit">{everyNWeeks === 1 ? 'week' : 'weeks'}</span>
                          </div>
                        </>
                      )}

                      {repeatKind === 'monthly' && (
                        <>
                          <div className="esi-rd-label">On day</div>
                          <div className="esi-month-anchor-row">
                            {(
                              [
                                ['1', '1st'], ['15', '15th'], ['last', 'Last day'],
                              ] as [string, string][]
                            ).map(([v, label]) => (
                              <button
                                key={v}
                                type="button"
                                className={'esi-month-anchor' + (monthDays.includes(v) ? ' esi-on' : '')}
                                onClick={() =>
                                  setMonthDays((cur) =>
                                    cur.includes(v)
                                      ? cur.length > 1
                                        ? cur.filter((x) => x !== v)
                                        : cur
                                      : [...cur, v],
                                  )
                                }
                              >
                                {label}
                              </button>
                            ))}
                            <button
                              type="button"
                              className={'esi-month-more' + (monthExpanded ? ' esi-on' : '')}
                              onClick={() => setMonthExpanded((o) => !o)}
                            >
                              More <Icon name={monthExpanded ? 'chevronDown' : 'chevronRight'} size={12} />
                            </button>
                          </div>
                          {monthExpanded && (
                            <div className="esi-month-grid-wrap">
                              <div className="esi-month-grid">
                                {Array.from({ length: 31 }, (_, i) => String(i + 1)).map((d) => (
                                  <button
                                    key={d}
                                    type="button"
                                    className={
                                      'esi-month-day' +
                                      (monthDays.includes(d) ? ' esi-on' : '') +
                                      (Number(d) >= 29 ? ' esi-rare' : '')
                                    }
                                    onClick={() =>
                                      setMonthDays((cur) =>
                                        cur.includes(d)
                                          ? cur.length > 1
                                            ? cur.filter((x) => x !== d)
                                            : cur
                                          : [...cur, d],
                                      )
                                    }
                                  >
                                    {d}
                                  </button>
                                ))}
                              </div>
                              <div className="esi-month-note">
                                Days a month doesn&apos;t have fall back to its last day — sent once.
                              </div>
                            </div>
                          )}
                          <div className="esi-rd-label esi-gap">Repeats every</div>
                          <div className="esi-every-n">
                            <Stepper
                              value={everyNMonths}
                              pad={false}
                              primary
                              min={1}
                              max={100}
                              displayValue={everyNMonths >= 100 ? 'EE' : undefined}
                              setValue={(n) => setEveryNMonths(everyNMonths >= 100 ? 12 : clamp(n, 1, 12))}
                              onDelta={(d) => {
                                if (d > 0 && everyNMonths >= 12) {
                                  setEveryNMonths(100); // a year → easter egg
                                } else if (d < 0 && everyNMonths >= 100) {
                                  setEveryNMonths(12); // step back from the egg
                                } else {
                                  setEveryNMonths(clamp(everyNMonths + d, 1, 12));
                                }
                              }}
                            />
                            <span className="esi-every-n-unit">
                              {everyNMonths >= 100
                                ? ''
                                : everyNMonths === 12
                                  ? 'months · = Every year'
                                  : everyNMonths === 1
                                    ? 'month'
                                    : 'months'}
                            </span>
                          </div>
                        </>
                      )}
                    </div>

                    <div className="esi-rd-block">
                      <div className="esi-rd-label">Ends</div>
                      <div className="esi-radio-list">
                        <div
                          className={'esi-radio-row' + (endMode === 'never' ? ' esi-sel' : '')}
                          onClick={() => setEndMode('never')}
                        >
                          <span className="esi-stud-radio" />
                          <div className="esi-radio-main">
                            <Icon name="infinity" size={16} />
                            <span className="esi-radio-text">Never ends</span>
                            <span className="esi-radio-sub">Runs until you stop it</span>
                          </div>
                        </div>
                        <div
                          className={'esi-radio-row' + (endMode === 'after' ? ' esi-sel' : '')}
                          onClick={() => setEndMode('after')}
                        >
                          <span className="esi-stud-radio" />
                          <div className="esi-radio-main">
                            <span className="esi-radio-text">Ends after</span>
                            <Stepper
                              value={endAfterCount}
                              unit="×"
                              pad={false}
                              dim={endMode !== 'after'}
                              min={1}
                              max={100}
                              setValue={(n) => {
                                setEndMode('after');
                                setEndAfterCount(clamp(n, 1, 100));
                              }}
                              onDelta={(d) => setEndAfterCount(clamp(endAfterCount + d, 1, 100))}
                            />
                            <span className="esi-radio-sub">sends, then retires</span>
                          </div>
                        </div>
                        <div
                          className={'esi-radio-row' + (endMode === 'at' ? ' esi-sel' : '')}
                          onClick={() => setEndMode('at')}
                        >
                          <span className="esi-stud-radio" />
                          <div className="esi-radio-main">
                            <span className="esi-radio-text">Ends on</span>
                            <DateInput
                              value={endDate}
                              active={endMode === 'at' && endActive === 'date'}
                              onFocus={() => {
                                setEndMode('at');
                                setEndActive('date');
                              }}
                              onCommit={(s) => setEndDate(s)}
                            />
                            <TimeInput
                              value={endTime}
                              active={endMode === 'at' && endActive === 'time'}
                              onFocus={() => {
                                setEndMode('at');
                                setEndActive('time');
                              }}
                              onCommit={(s) => setEndTime(s)}
                            />
                          </div>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            )}
          </div>
        </div>

        {/* ───── Send-out readout ───── */}
        <div className="esi-section">
          <div className="esi-sec-head">
            <span className="esi-sec-label">
              <span className="esi-gly">◆</span> Summary
            </span>
          </div>
          <div className="esi-sendout">
            {!repeating ? (
              <div className="esi-sendline">
                <span className="esi-send-in">{sendInText()}</span>
                {sendMode === 'at' && <RepTag />}
              </div>
            ) : (
              <div className="esi-runs">
                <div className="esi-runs-head">
                  <button type="button" className="esi-runs-toggle" onClick={() => setRunsExpanded((e) => !e)}>
                    <Icon name={runsExpanded ? 'chevronDown' : 'chevronRight'} size={13} />
                    <span>{runsExpanded ? 'All runs' : 'First 3 runs'}</span>
                  </button>
                  <span className="esi-spacer" />
                  <RepTag />
                </div>
                <div className={'esi-runs-list' + (runsExpanded ? ' esi-expanded' : '')}>
                  {!runsExpanded
                    ? buildRuns(3).map((dt, i) => <RunRow key={i} i={i + 1} dt={dt} />)
                    : endMode === 'never'
                      ? [
                          ...buildRuns(3).map((dt, i) => <RunRow key={i} i={i + 1} dt={dt} />),
                          <div className="esi-run-row esi-forever" key="fv">
                            <span className="esi-run-idx">∞</span>
                            <span className="esi-run-forever">
                              repeats {repeatKind === 'fixed'
                                ? `every ${fmtInterval(repDays, repHrs, repMin)}`
                                : repeatKind === 'weekly'
                                  ? everyNWeeks === 1 ? 'weekly' : `every ${everyNWeeks} weeks`
                                  : everyNMonths === 12 ? 'yearly' : everyNMonths === 1 ? 'monthly' : `every ${everyNMonths} months`}
                              , forever
                            </span>
                          </div>,
                        ]
                      : buildRuns(runCount(100)).map((dt, i) => <RunRow key={i} i={i + 1} dt={dt} />)}
                  {previewIsEstimate && (
                    <div className="esi-run-row">
                      <span className="esi-run-idx">~</span>
                      <span className="esi-run-todo">
                        Estimated cadence — exact dates resolved when scheduled.
                      </span>
                    </div>
                  )}
                </div>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

export default EmberScheduleInstrument;
