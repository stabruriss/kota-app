import { useEffect, useId, useState, type CSSProperties, type ReactNode } from 'react';
import type { SoulPushPatch, SoulPushSettings } from '../lib/soul-push';
import '../styles/puppeteer-mark-dream.css';

export function PuppeteerMarkAndDreamControls({
  value, jevReady, disabled = false, onChange, onEditStart, onEditCancel, onJevSettings, status,
}: {
  value: SoulPushSettings;
  /** null means readiness has not been established; it must not look like a missing key. */
  jevReady: boolean | null;
  disabled?: boolean;
  onChange: (patch: SoulPushPatch) => void;
  onEditStart: () => void;
  onEditCancel: () => void;
  onJevSettings: () => void;
  status?: ReactNode;
}) {
  const id = useId();
  const locked = disabled || jevReady !== true;
  const editing = { onEditStart, onEditCancel };
  return <div className="puppeteer-mark-dream">
    <div className="puppeteer-mark-heading">
      <div><h3 id={`${id}-label`}>Use marks and dreams</h3>
        <p id={`${id}-help`}>Kota uses Jev to select old memory notes that may be related and attaches them to a new message.</p>
      </div>
      <button type="button" className="puppeteer-mark-switch" role="switch" aria-labelledby={`${id}-label`} aria-describedby={`${id}-help`}
        aria-checked={value.enabled && jevReady === true} disabled={locked} onClick={() => onChange({ enabled: !value.enabled })} />
    </div>
    {jevReady === false && <p className="puppeteer-mark-jev-hint"><button type="button" onClick={onJevSettings}>Set up Jev under String to enable.</button></p>}
    <fieldset className="puppeteer-mark-controls" disabled={locked || !value.enabled} aria-label="Mark and Dream settings">
      <div className="puppeteer-mark-thresholds">
        <Threshold label="Mark threshold" hint="Must be more certain than this to attach a working note."
          {...editing} value={value.markThreshold} onCommit={markThreshold => onChange({ markThreshold })} />
        <Threshold label="Dream threshold" hint="Must be more certain than this to attach a user note."
          {...editing} value={value.dreamThreshold} onCommit={dreamThreshold => onChange({ dreamThreshold })} />
      </div>
      <div className="puppeteer-mark-limits">
        <NumberSetting {...editing} label="Max marks per message" value={value.maxMarks} min={1} max={5} step={1} onCommit={maxMarks => onChange({ maxMarks })} />
        <NumberSetting {...editing} label="Max dreams per message" value={value.maxDreams} min={0} max={5} step={1} onCommit={maxDreams => onChange({ maxDreams })} />
      </div>
      <NumberSetting {...editing} label="Cooldown" unit="replies" value={value.cooldownReplies} min={0} max={50} step={1}
        hint="A pushed note stays quiet for this many of the agent's replies. Resets on a new session or a new day."
        onCommit={cooldownReplies => onChange({ cooldownReplies })} />
      <NumberSetting {...editing} label="Minimum message length" unit="characters" value={value.minimumMessageLength} min={0} max={500} step={10}
        hint="Shorter messages are sent without marks." onCommit={minimumMessageLength => onChange({ minimumMessageLength })} />
    </fieldset>
    {status && <div className="puppeteer-mark-status" role="status">{status}</div>}
  </div>;
}

interface EditingProps { onEditStart: () => void; onEditCancel: () => void }
function Threshold({ label, hint, value, onCommit, onEditStart, onEditCancel }: EditingProps & { label: string; hint: string; value: number; onCommit: (value: number) => void }) {
  const id = useId();
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = (raw: string) => {
    const next = Number(raw);
    if (Number.isFinite(next) && next >= .05 && next <= .95) onCommit(next);
  };
  return <div className="puppeteer-mark-threshold">
    <label htmlFor={id}>{label}<output htmlFor={id}>{draft.toFixed(2)}</output></label>
    <input id={id} type="range" min={.05} max={.95} step={.05} value={draft}
      aria-describedby={`${id}-hint`} aria-valuetext={draft.toFixed(2)}
      style={{ '--mark-threshold-fill': `${(draft - .05) / .9 * 100}%` } as CSSProperties}
      onFocus={onEditStart}
      onChange={event => { onEditStart(); setDraft(Number(event.currentTarget.value)); }}
      // Leave pointer capture to the native thumb so WebKit can drag it.
      onPointerDown={onEditStart}
      onPointerUp={event => commit(event.currentTarget.value)}
      onPointerCancel={() => { setDraft(value); onEditCancel(); }}
      onKeyDown={event => {
        if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); setDraft(value); onEditCancel(); }
        else onEditStart();
      }}
      onKeyUp={event => {
        if (['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End', 'PageUp', 'PageDown'].includes(event.key)) commit(event.currentTarget.value);
      }}
      onBlur={event => commit(event.currentTarget.value)} />
    <p id={`${id}-hint`}>{hint}</p>
  </div>;
}

function NumberSetting({ label, hint, unit, value, min, max, step, onCommit, onEditStart, onEditCancel }: EditingProps & {
  label: string; hint?: string; unit?: string; value: number; min: number; max: number; step: number; onCommit: (value: number) => void;
}) {
  const id = useId();
  const [draft, setDraft] = useState(String(value));
  const [invalid, setInvalid] = useState(false);
  useEffect(() => { setDraft(String(value)); setInvalid(false); }, [value]);
  const parse = () => {
    const parsed = draft.trim() ? Number(draft) : NaN;
    return Number.isInteger(parsed) && parsed >= min && parsed <= max && (parsed - min) % step === 0 ? parsed : null;
  };
  const commit = () => {
    const next = parse();
    setInvalid(next === null);
    if (next !== null) onCommit(next);
  };
  const adjust = (delta: number) => {
    const next = Math.max(min, Math.min(max, (parse() ?? value) + delta * step));
    onEditStart();
    setDraft(String(next)); setInvalid(false);
    onCommit(next);
  };
  const current = parse() ?? value;
  return <div className={`puppeteer-mark-number${hint ? ' has-hint' : ''}`}>
    <label htmlFor={id}>{label}</label>
    {hint && <p id={`${id}-hint`}>{hint}</p>}
    <div className="puppeteer-mark-stepper">
      <button type="button" aria-label={`Decrease ${label.toLowerCase()}`} disabled={current <= min}
        onPointerDown={event => event.preventDefault()} onClick={() => adjust(-1)}>−</button>
      <input id={id} type="number" inputMode="numeric" min={min} max={max} step={step} value={draft}
        aria-invalid={invalid || undefined} aria-describedby={invalid ? `${id}-error` : hint ? `${id}-hint` : undefined}
        onFocus={onEditStart} onChange={event => { onEditStart(); setDraft(event.currentTarget.value); setInvalid(false); }} onBlur={commit}
        onKeyDown={event => {
          if (event.key === 'Enter') { event.preventDefault(); commit(); }
          if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); setDraft(String(value)); setInvalid(false); onEditCancel(); }
        }} />
      {unit && <span>{unit}</span>}
      <button type="button" aria-label={`Increase ${label.toLowerCase()}`} disabled={current >= max}
        onPointerDown={event => event.preventDefault()} onClick={() => adjust(1)}>+</button>
    </div>
    {invalid && <p id={`${id}-error`} className="puppeteer-mark-invalid" role="status">Enter a value from {min} to {max} in steps of {step}.</p>}
  </div>;
}
