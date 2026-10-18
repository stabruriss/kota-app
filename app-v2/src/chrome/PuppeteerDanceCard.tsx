import { useEffect, useId, useRef, useState, type CSSProperties } from 'react';
import { flushSync } from 'react-dom';
import { HeroAvatarArt } from './HeroAvatarPicker';
import { LocalModelDialog } from './LocalModelDialog';
import { requestKotaLocalLink } from '../lib/local-auth';
import { useJevSettings } from '../lib/jev';
import { danceSettingsIssue, danceErrorText, type DanceAgent, type DanceController } from '../lib/dance-settings';
import { CopyRuleFilePath } from './CopyRuleFilePath';
import { DanceRouteLog } from './DanceRouteLog';
import type { DanceSettingsSnapshot, DanceSettings } from '../lib/puppeteer-dance';
import '../styles/puppeteer-dance.css';

const SHOW_LOG_PROJECT = true;

/** Read-only: the runtime sentinel changes every round, so it is shown, never chosen. */
export function SentinelReadout({ agent, compact = false, auto = false }: { agent: DanceAgent | null; compact?: boolean; auto?: boolean }) {
  return <div className="dance-sentinel-readout" role="status" aria-label="Current sentinel">
    {compact && <span className="dance-sentinel-label" title="The sentinel follows whoever received your last message. If the recipient can't be decided, the sentinel receives the message.">Sentinel</span>}
    {agent ? <><HeroAvatarArt avatarId={agent.avatarId} className="dance-agent-avatar" /><span className="dance-sentinel-name">{agent.name}</span></>
      : <span className="dance-sentinel-idle">Follows the last recipient</span>}
    {auto && <i className="dance-sentinel-auto">Auto</i>}
  </div>;
}

function RoutingCondition({ value, name, onChange }: { value: string; name: string; onChange: (value: string) => void }) {
  const input = useRef<HTMLTextAreaElement>(null);
  useEffect(() => { const el = input.current; if (el) { el.style.height = 'auto'; el.style.height = `${el.scrollHeight + 2}px`; } }, [value]);
  return <textarea ref={input} rows={2} aria-label={`Routing condition for ${name}`} placeholder="Which tasks should this agent receive?" value={value}
    onChange={event => onChange(Array.from(event.target.value).slice(0, 200).join(''))} />;
}

export function DanceSettingsEditor({ settings, agents, owner, busy, error, onSave, onClose, projectRoot, onPrepared, onReload, sentinel = null }: {
  settings: DanceSettings; agents: readonly DanceAgent[]; owner: string; busy: boolean; error: string | null; sentinel?: DanceAgent | null;
  onSave: (settings: DanceSettings) => Promise<DanceSettings | null>; onClose: () => void;
  projectRoot?: string | null; onPrepared?: () => void | Promise<unknown>; onReload?: () => void;
}) {
  const [draft, setDraft] = useState<DanceSettings>(() => structuredClone(settings));
  const dirty = JSON.stringify(draft) !== JSON.stringify(settings);
  const issue = danceSettingsIssue(draft, agents); const thresholdId = useId();
  return <LocalModelDialog title="Puppeteer Setting" subtitle="Dancing: decide which Agent in this project receive your message" onClose={onClose} closeLabel="Close Dance settings"
    titleAction={draft.strategy === 'rule' && projectRoot && <CopyRuleFilePath target={{ projectRoot, kind: 'dance' }} dirty={dirty} disabled={busy} onPrepared={onPrepared} />}
    footer={<><p role="status" className={error || issue ? 'local-model-warning' : ''}>{error || issue || (dirty ? 'Unsaved changes' : 'No changes.')}{error && onReload && <button type="button" className="local-model-text" onClick={onReload}>Reload</button>}</p>
      <button type="button" className="string-ok" disabled={busy || (dirty && !!issue)} onClick={dirty ? async () => { if (await onSave(draft)) onClose(); } : onClose}>{busy ? 'Saving…' : dirty ? 'Save' : 'OK'}</button></>}>
    <div className="dance-settings-body">{SHOW_LOG_PROJECT && <div className="dance-settings-owner">{owner}</div>}
      <DanceRouteLog projectRoot={projectRoot} footer={<DanceJevStatus onClose={onClose} />} />
      <fieldset disabled={busy}>
      <div className="string-threshold"><div><label htmlFor={thresholdId}>The certainty must be higher than this to switch agent.</label><output>{draft.threshold.toFixed(2)}</output></div>
        <input id={thresholdId} type="range" aria-label="Dance probability threshold" min="0.05" max="0.95" step="0.05" value={draft.threshold}
          style={{ '--string-threshold-fill': `${(draft.threshold - .05) / .9 * 100}%` } as CSSProperties} onChange={event => setDraft({ ...draft, threshold: Number(event.target.value) })} /></div>
      <div className="dance-strategy" role="group" aria-label="Dance mode">{(['context', 'rule'] as const).map(strategy => <button key={strategy} type="button" aria-pressed={draft.strategy === strategy}
        onClick={() => setDraft({ ...draft, strategy })}>{strategy === 'context' ? 'Context based' : 'Rule based'}</button>)}</div>
      <p>{draft.strategy === 'context' ? 'The recipient is decided from recent conversation.' : 'Matches each agent’s routing condition. Empty conditions are not matched.'}</p>
      <div className="dance-sentinel-row"><span>Sentinel</span><SentinelReadout agent={sentinel} auto /></div>
      <p>If the recipient cannot be decided, the sentinel receives the message.</p>
      {draft.strategy === 'rule' && <div className="dance-conditions">{agents.map(agent => { const value = draft.conditions[agent.id] ?? ''; return <section key={agent.id}>
        <header><HeroAvatarArt avatarId={agent.avatarId} className="dance-agent-avatar" /><b>{agent.name}</b><small>{Array.from(value).length} / 200</small></header>
        <RoutingCondition value={value} name={agent.name} onChange={condition => setDraft({ ...draft, conditions: { ...draft.conditions, [agent.id]: condition } })} />
        {!value.trim() && <small>Not matched.</small>}
      </section>; })}</div>}
    </fieldset></div>
  </LocalModelDialog>;
}

function DanceJevStatus({ onClose }: { onClose: () => void }) {
  const jev = useJevSettings(true);
  const rejected = jev.settings?.keys.find(key => key.providerId === jev.settings?.provider)?.status === 'rejected';
  return <div className="dance-settings-jev"><div><b>Jev</b><span>{jev.failed ? 'Status unavailable' : jev.loading ? 'Checking…' : rejected ? 'Rejected' : jev.settings?.ready ? 'Verified' : 'Not set'}</span></div>
    {jev.failed ? <button type="button" className="local-model-text" onClick={() => void jev.refresh()}>Retry</button>
      : <button type="button" className="local-model-text" onClick={() => { flushSync(onClose); requestKotaLocalLink('puppeteer-jev'); }}>{jev.settings?.ready ? 'Jev settings' : 'Set up Jev'}</button>}
    {!jev.loading && !jev.failed && !jev.settings?.ready && <p>{danceErrorText(rejected ? 'jev_key_rejected' : 'jev_key_required')}</p>}
  </div>;
}

const errorCode = (error: unknown) => typeof error === 'string' ? error : (error as { code?: string } | null)?.code ?? 'dance_settings_read_failed';

/** `seedTarget`: the composer's selected agent (null for a group send). Dance starts with it as sentinel, else the lowest seat. */
export function PuppeteerDanceCard({ controller, agents, owner, seedTarget = null }: { controller: DanceController; agents: readonly DanceAgent[]; owner: string; seedTarget?: string | null }) {
  const [editing, setEditing] = useState<DanceSettingsSnapshot | null>(null);
  const [blocked, setBlocked] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const inFlight = useRef(false);
  const generation = useRef(0);
  const jev = useJevSettings(true);
  const { settings, busy, error } = controller;
  const sentinel = settings?.enabled ? agents.find(agent => agent.id === controller.currentSentinel) ?? null : null;
  useEffect(() => { ++generation.current; setEditing(null); setBlocked(null); }, [controller.projectRoot]);
  async function openSettings() {
    if (inFlight.current) return;
    inFlight.current = true; setChecking(true);
    const gen = generation.current;
    try {
      const [next] = await Promise.all([controller.refresh(), jev.refresh()]);
      if (gen !== generation.current) return;
      if (next) setEditing(next); else setBlocked(errorCode(controller.currentError()));
    } finally { inFlight.current = false; setChecking(false); }
  }
  async function start() {
    if (inFlight.current || busy) return;
    if (settings?.enabled) {
      if (!await controller.save(current => ({ ...current, enabled: false }))) setBlocked(errorCode(controller.currentError()));
      return;
    }
    inFlight.current = true; setChecking(true);
    const gen = generation.current;
    try {
      const [next, key] = await Promise.all([controller.refresh(), jev.refresh()]);
      if (gen !== generation.current) return;
      if (!next) { setBlocked(errorCode(controller.currentError())); return; }
      const seed = agents.find(agent => agent.id === seedTarget)?.id ?? agents[0]?.id;
      if (!seed) { setBlocked('dance_seat_required'); return; }
      if (!key) { setBlocked('jev_settings_read_failed'); return; }
      const rejected = key.keys.find(item => item.providerId === key.provider)?.status === 'rejected';
      if (rejected || !key.ready) { setBlocked(rejected ? 'jev_key_rejected' : 'jev_key_required'); return; }
      const issue = danceSettingsIssue(next.settings, agents);
      if (issue) { setBlocked(issue === 'Add routing conditions for at least 2 agents.' ? 'dance_rules_required' : 'dance_settings_invalid'); return; }
      if (!await controller.save({ ...next.settings, enabled: true, sentinel: seed }, next.sourceRevision)) {
        const code = errorCode(controller.currentError());
        setBlocked(code === 'dance_sentinel_required' ? 'dance_seat_required' : code);
      }
    } finally { inFlight.current = false; setChecking(false); }
  }
  const jevProblem = blocked?.startsWith('jev_') && blocked !== 'jev_settings_read_failed';
  const seatProblem = blocked === 'dance_seat_required';
  const settingsProblem = ['dance_rules_required', 'dance_settings_invalid', 'dance_settings_conflict'].includes(blocked ?? '');
  const label = jevProblem ? 'Set up Jev' : seatProblem ? 'OK' : settingsProblem ? 'Edit rules' : 'Retry';
  return <section className="lm-card puppeteer-dance-card" aria-label={`Puppeteer Dance · ${owner}`} data-dance-enabled={!!settings?.enabled}>
    <header className="lm-card-head"><HeroAvatarArt avatarId="puppeteer" className="lm-avatar" /><span className="lm-copy"><b>Puppeteer</b><small>{settings?.strategy === 'rule' ? 'Rule based' : 'Context based'}</small></span>
      <span className="lm-actions"><button type="button" className={`dance-play${settings?.enabled ? ' is-on' : ''}`} aria-label={settings?.enabled ? 'Turn Dance off' : 'Turn Dance on'} aria-pressed={!!settings?.enabled}
        disabled={busy || checking} aria-busy={checking} onClick={() => void start()}><i aria-hidden /></button>
        <button type="button" className="lm-gear" aria-label={`Dance settings · ${owner}`} disabled={busy || checking} onClick={() => void openSettings()}>⚙</button></span>
    </header>
    <div className="dance-sentinel-row dance-sentinel-compact"><SentinelReadout compact agent={sentinel} /></div>
    {editing && <DanceSettingsEditor key={editing.sourceRevision} settings={editing.settings} agents={agents} owner={owner} busy={busy} error={error} sentinel={sentinel}
      onSave={draft => controller.save(draft, editing.sourceRevision)} onClose={() => setEditing(null)} projectRoot={controller.projectRoot}
      onPrepared={controller.refresh} onReload={() => void openSettings()} />}
    {blocked && <LocalModelDialog title="Before you start Dance" subtitle={owner} art={<HeroAvatarArt avatarId="puppeteer" className="dance-agent-avatar" />} closeLabel="Close start check" onClose={() => setBlocked(null)}
      footer={<>{!seatProblem && <button type="button" className="local-model-text" onClick={() => setBlocked(null)}>Cancel</button>}<button type="button" className="string-ok" onClick={() => {
        flushSync(() => setBlocked(null));
        if (seatProblem) return;
        if (jevProblem) requestKotaLocalLink('puppeteer-jev');
        else if (settingsProblem) void openSettings();
        else void start();
      }}>{label}</button></>}>
      <div className="dance-start-validation" role="status">{danceErrorText(blocked)}</div>
    </LocalModelDialog>}
  </section>;
}
