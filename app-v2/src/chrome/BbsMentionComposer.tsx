import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react';
import { bbsCurrentMentionNames, bbsMentionCountLabel as count, bbsMentionCounts, bbsMentionDeviceKey,
  bbsMentionGroups, bbsMentionKey, bbsMentionSelection } from '../bbs-mentions';
import type { BbsMentionSelection, BbsRosterAgent, BbsRosterDevice, BbsRosterProject } from '../types/bbs-roster';
import '../styles/bbs-mentions.css';

const EMPTY_DEVICES: readonly BbsRosterDevice[] = [];
const PROJECT_BATCH = 8;
const AGENT_BATCH = 64;
function DeviceIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="4" y="4" width="16" height="12" rx="1.5" /><path d="M2 20h20l-2-4H4l-2 4Z" /></svg>;
}
function FolderIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 6V4h6l2 3h10v13H3V6Z" /></svg>;
}
function Cross() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M6 18 18 6" /></svg>;
}

export interface BbsMentionComposerProps {
  children: ReactNode;
  submit: ReactNode;
  devices?: readonly BbsRosterDevice[];
  currentProjectId: string;
  selected: readonly BbsMentionSelection[];
  onChange: (targets: BbsMentionSelection[]) => void;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  remoteAllowed: boolean;
  disabled?: boolean;
  loading?: boolean;
  error?: string | null;
  onRetry?: () => void;
  renderAvatar?: (agent: BbsRosterAgent, device: BbsRosterDevice) => ReactNode;
}

/** Layout B. Props-only: no IPC, storage, bus or default recipients. The editor
 * stays at one React position while roster/device/picker state changes. */
export function BbsMentionComposer({ children, submit, devices = EMPTY_DEVICES, currentProjectId,
  selected, onChange, open, onOpenChange, remoteAllowed, disabled = false, loading = false,
  error, onRetry, renderAvatar }: BbsMentionComposerProps) {
  const id = useId();
  const picker = useRef<HTMLElement>(null);
  const summary = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const [activeKey, setActiveKey] = useState('local');
  const [projectLimit, setProjectLimit] = useState(PROJECT_BATCH);
  const chosen = useMemo(() => bbsCurrentMentionNames(selected, devices), [selected, devices]);
  const keys = useMemo(() => new Set(chosen.map(bbsMentionKey)), [chosen]);
  const groups = useMemo(() => bbsMentionGroups(chosen), [chosen]);
  const counts = bbsMentionCounts(chosen);
  const active = devices.find(device => bbsMentionDeviceKey(device) === activeKey && device.rosterStatus === 'synced')
    ?? devices.find(device => device.local && device.rosterStatus === 'synced')
    ?? devices.find(device => device.rosterStatus === 'synced');
  const activeId = active && bbsMentionDeviceKey(active);
  const projects = useMemo(() => {
    const source = active?.projects ?? [];
    if (!active?.local) return source;
    const current = source.find(project => project.projectId === currentProjectId);
    return current ? [current, ...source.filter(project => project !== current)] : source;
  }, [active, currentProjectId]);

  useEffect(() => { setProjectLimit(PROJECT_BATCH); }, [activeId]);
  useEffect(() => {
    if (open) picker.current?.querySelector<HTMLButtonElement>('nav button[aria-pressed="true"]')?.focus({ preventScroll: true });
  }, [open]);
  useEffect(() => {
    if (!open) return;
    function keydown(event: KeyboardEvent) {
      if (event.key !== 'Escape' || event.defaultPrevented || event.isComposing) return;
      event.preventDefault(); event.stopPropagation(); onOpenChange(false); trigger.current?.focus();
    }
    function outside(event: PointerEvent) {
      const target = event.target as Node;
      if (!picker.current?.contains(target) && !trigger.current?.contains(target) && !summary.current?.contains(target)) onOpenChange(false);
    }
    document.addEventListener('keydown', keydown, true);
    document.addEventListener('pointerdown', outside);
    return () => { document.removeEventListener('keydown', keydown, true); document.removeEventListener('pointerdown', outside); };
  }, [open, onOpenChange]);

  const close = () => { onOpenChange(false); trigger.current?.focus(); };
  function toggle(device: BbsRosterDevice, project: BbsRosterProject, agent: BbsRosterAgent) {
    if (disabled || (!device.local && !remoteAllowed)) return;
    const target = bbsMentionSelection(device, project, agent);
    const key = bbsMentionKey(target);
    onChange(keys.has(key) ? chosen.filter(value => bbsMentionKey(value) !== key) : [...chosen, target]);
  }
  return <>
    {open && <section ref={picker} id={id} className="bbs-mentions-panel" aria-label="Mention agents">
      <header className="bbs-mentions-head"><strong>Mention agents</strong><button type="button" onClick={close} aria-label="Close agent picker"><Cross /></button></header>
      <nav className="bbs-mentions-devices" aria-label="Devices">
        {devices.map(device => {
          const key = bbsMentionDeviceKey(device);
          const n = chosen.filter(value => value.deviceId === key).length;
          return <button type="button" key={key} aria-pressed={key === activeId} disabled={disabled || device.rosterStatus !== 'synced'}
            onClick={() => setActiveKey(key)}><DeviceIcon /><span>{device.name}</span>
            {device.local ? <small>This device</small> : !device.online && device.rosterStatus === 'synced' ? <span className="bbs-mentions-offline" aria-label="Offline" /> : null}
            {n > 0 && <span className="bbs-mentions-count">{n}</span>}
          </button>;
        })}
      </nav>
      {loading && <p className="bbs-mentions-message" role="status">Loading agents…</p>}
      {error && <div className="bbs-mentions-message" role="status">{error}{onRetry && <button type="button" disabled={disabled || loading} onClick={onRetry}>Retry</button>}</div>}
      {!loading && !error && active && projects.length === 0 && <p className="bbs-mentions-message">No active agents.</p>}
      <div className="bbs-mentions-projects" key={activeId}>
        {active && projects.slice(0, projectLimit).map(project => <ProjectSection key={project.projectId}
          device={active} project={project} current={active.local && project.projectId === currentProjectId}
          disabled={disabled || (!active.local && !remoteAllowed)} selectedKeys={keys}
          onToggle={agent => toggle(active, project, agent)} renderAvatar={renderAvatar} />)}
        {projects.length > projectLimit && <button type="button" className="bbs-mentions-more" disabled={disabled}
          onClick={() => setProjectLimit(limit => limit + PROJECT_BATCH)}>Show more projects ({projects.length - projectLimit})</button>}
      </div>
      {active && !active.local && !remoteAllowed && <p className="bbs-mentions-warning">This thread is not shared. Start a new thread to @ this agent.</p>}
      <footer className="bbs-mentions-footer"><span aria-live="polite">{count(counts.agents, 'agent')} · {count(counts.projects, 'project')} · {count(counts.devices, 'device')}</span>
        <button type="button" className="bbs-mentions-clear" disabled={disabled || chosen.length === 0} onClick={() => onChange([])}>Clear</button>
        <button type="button" className="bbs-mentions-done" onClick={close}>Done</button></footer>
    </section>}
    {children}
    {!open && chosen.length > 0 && <div ref={summary} className="bbs-mentions-summary" aria-label="Selected recipients">
      {groups.map(group => <div key={group.key} className="bbs-mentions-group"><div className="bbs-mentions-route"><DeviceIcon />
        <span>{group.deviceName}</span><span className="bbs-mentions-slash">/</span><span>{group.projectName}</span></div>
        <div className="bbs-mentions-names">{group.targets.map(target => <button type="button" key={bbsMentionKey(target)} disabled={disabled}
          aria-label={`Remove ${target.agentName}, ${target.deviceName}, ${target.projectName}`}
          onClick={() => { onChange(chosen.filter(value => bbsMentionKey(value) !== bbsMentionKey(target))); trigger.current?.focus(); }}>
          {target.agentName}<Cross /></button>)}</div></div>)}
    </div>}
    <div className="bbs-reply-toolbar">
      <button ref={trigger} type="button" className={`bbs-agent-bar-toggle ${open || chosen.length ? 'on' : ''}`} aria-controls={id} aria-expanded={open}
        disabled={disabled} onClick={() => onOpenChange(!open)}>@ Agent{chosen.length > 0 ? ` · ${chosen.length}` : ''}</button>
      {chosen.length > 0 && <span className="bbs-mentions-scope">{count(counts.projects, 'project')} · {count(counts.devices, 'device')}</span>}
      {submit}
    </div>
  </>;
}

function ProjectSection({ device, project, current, disabled, selectedKeys, onToggle, renderAvatar }: {
  device: BbsRosterDevice; project: BbsRosterProject; current: boolean; disabled: boolean;
  selectedKeys: ReadonlySet<string>; onToggle: (agent: BbsRosterAgent) => void;
  renderAvatar?: BbsMentionComposerProps['renderAvatar'];
}) {
  const [limit, setLimit] = useState(AGENT_BATCH);
  return <section className="bbs-mentions-project" aria-label={`${device.name} / ${project.name}`}>
    <div className="bbs-mentions-project-head"><FolderIcon /><div><strong>{project.name}</strong><small>{current ? 'Current project' : count(project.agents.length, 'agent')}</small></div></div>
    <div className="bbs-mentions-agents">{project.agents.slice(0, limit).map(agent => {
      const value = bbsMentionSelection(device, project, agent);
      const checked = selectedKeys.has(bbsMentionKey(value));
      return <button type="button" className="bbs-mentions-agent" role="checkbox" aria-checked={checked} key={agent.agentId}
        aria-label={`${agent.name}, ${device.name}, ${project.name}`} disabled={disabled} onClick={() => onToggle(agent)}>
        {renderAvatar ? renderAvatar(agent, device) : <span className="bbs-mentions-avatar bbs-mentions-letter" aria-hidden="true">{Array.from(agent.name)[0] || '?'}</span>}
        <span className="bbs-mentions-agent-name">{agent.name}</span><svg className="bbs-mentions-check" viewBox="0 0 24 24" aria-hidden="true"><path d="m5 12 4 4L19 6" /></svg>
      </button>;
    })}
      {project.agents.length > limit && <button type="button" className="bbs-mentions-more" disabled={disabled}
        onClick={() => setLimit(value => value + AGENT_BATCH)}>Show more agents ({project.agents.length - limit})</button>}
    </div>
  </section>;
}
