import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { WorkingHero } from '../types/agentbar';
import type { AgentId } from '../types/scene';
import { listWorkspaceProjects, listProjectAgentIdentities, loadProjectAgentDetail, type ProjectAgentIdentity, type WorkspaceProject } from '../pty-client';
import { readRecruitPreview, type IncarnationRecruitSource, type RecruitPreview } from '../lib/reincarnation';
import { avatarClassForId, avatarImageStyleForId } from '../lib/hero-avatars';
import { splitProjectAgentName } from './ProjectAgentName';
import { WorkingHeroPicker } from './WorkingHeroPicker';
import '../styles/recruit-picker.css';

type Selection = { hero: WorkingHero; preview: RecruitPreview } | { source: IncarnationRecruitSource; preview: RecruitPreview };
type RecruitTab = 'tavern' | 'this' | 'other';
const TAVERN_PREVIEW_COUNT = 4;
const SAME_PROJECT_COPY = 'The new agent starts with the same marks and episodes, then goes its own way.';
const failureText = (error: unknown) => error instanceof Error ? error.message : String(error);
const tabLabel = (tab: RecruitTab) => tab === 'tavern' ? 'Tavern' : tab === 'this' ? 'This Project' : 'Other Project';

export function ShortcutRecruitModal({ seatNumber, heroes, unavailableHeroIds, projectRoot, onSelect, onSelectIncarnation, onDismiss }: {
  seatNumber: number;
  heroes: readonly WorkingHero[];
  unavailableHeroIds?: ReadonlySet<AgentId>;
  projectRoot?: string | null;
  onSelect: (hero: WorkingHero, preview?: RecruitPreview) => void | Promise<void>;
  onSelectIncarnation?: (source: IncarnationRecruitSource) => void | Promise<void>;
  onDismiss: () => void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const previous = useRef(document.activeElement instanceof HTMLElement ? document.activeElement : null);
  const mounted = useRef(true);
  const pendingRef = useRef(false);
  const delivering = useRef(false);
  const thisAgentsRef = useRef<readonly ProjectAgentIdentity[]>([]);
  const [pending, setPending] = useState(false);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<RecruitTab>('tavern');
  const [thisOpened, setThisOpened] = useState(false);
  const [otherOpened, setOtherOpened] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const tabs: RecruitTab[] = onSelectIncarnation ? (projectRoot ? ['tavern', 'this', 'other'] : ['tavern', 'other']) : ['tavern'];
  const visibleHeroes = tab === 'tavern' ? (showAll ? heroes : heroes.slice(0, TAVERN_PREVIEW_COUNT)) : [];
  const selectedPerson = selection && ('hero' in selection ? selection.hero : selection.source.agent);
  const sameProjectSelection = Boolean(
    selection && 'source' in selection && projectRoot && selection.source.projectRoot === projectRoot,
  );
  const selectTab = (next: RecruitTab) => {
    setTab(next);
    if (next === 'this') setThisOpened(true);
    if (next === 'other') setOtherOpened(true);
  };

  useEffect(() => {
    mounted.current = true;
    panel.current?.querySelector<HTMLElement>('button')?.focus();
    return () => {
      mounted.current = false;
      if (previous.current?.isConnected) previous.current.focus();
    };
  }, []);
  useEffect(() => {
    if (selection) panel.current?.querySelector<HTMLButtonElement>('[data-cancel-recruit]')?.focus();
  }, [selection]);
  const dismiss = () => { if (!delivering.current) onDismiss(); };
  const deliver = useCallback(async (next: Selection) => {
    if (delivering.current) return;
    delivering.current = true; pendingRef.current = true;
    setPending(true); setError(null);
    try {
      if ('hero' in next) await onSelect(next.hero, next.preview);
      else await onSelectIncarnation?.(next.source);
    } catch (failure) { if (mounted.current) setError(failureText(failure)); }
    finally {
      delivering.current = false; pendingRef.current = false;
      if (mounted.current) setPending(false);
    }
  }, [onSelect, onSelectIncarnation]);
  const selectHero = useCallback(async (hero: WorkingHero) => {
    if (pendingRef.current || hero.available === false || unavailableHeroIds?.has(hero.id)) return;
    pendingRef.current = true; setPending(true); setError(null);
    try {
      // A source may have become relic/dangling since Tavern was opened.
      // Preview has no materialization side effects; confirmation comes first.
      const preview = await readRecruitPreview({ heroId: hero.templateId ?? hero.id });
      if (!mounted.current) return;
      if (preview.state === 'native') await deliver({ hero, preview });
      else setSelection({ hero, preview });
    } catch (failure) { if (mounted.current) setError(failureText(failure)); }
    finally { pendingRef.current = false; if (mounted.current) setPending(false); }
  }, [deliver, unavailableHeroIds]);
  const selectIncarnation = async (sourceProjectRoot: string, identity: ProjectAgentIdentity) => {
    if (pendingRef.current) return;
    pendingRef.current = true; setPending(true); setError(null);
    try {
      const agent = await loadProjectAgentDetail({ projectRoot: sourceProjectRoot, agentId: identity.agentId }, { readOnly: true });
      if (!mounted.current) return;
      const preview = await readRecruitPreview({ sourceProjectRoot, sourceAgentId: identity.agentId });
      if (mounted.current) setSelection({ source: { projectRoot: sourceProjectRoot, agent }, preview });
    } catch (failure) { if (mounted.current) setError(failureText(failure)); }
    finally { pendingRef.current = false; if (mounted.current) setPending(false); }
  };
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.isComposing) return;
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); dismiss(); return; }
      if (selection || pendingRef.current || event.metaKey || event.ctrlKey || event.altKey || !/^[1-9]$/.test(event.key)) return;
      const index = Number(event.key) - 1;
      if (tab === 'tavern') {
        const hero = visibleHeroes[index];
        if (!hero || hero.available === false || unavailableHeroIds?.has(hero.id)) return;
        event.preventDefault(); event.stopPropagation(); void selectHero(hero);
      } else if (tab === 'this' && projectRoot) {
        const identity = thisAgentsRef.current[index];
        if (!identity) return;
        event.preventDefault(); event.stopPropagation(); void selectIncarnation(projectRoot, identity);
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  return createPortal(<div className={`shortcut-recruit-layer soul-recruit-layer ${pending ? 'pending' : ''}`}
    role="presentation" onClick={dismiss} data-testid="shortcut-recruit-modal" aria-busy={pending}
    onWheel={event => event.stopPropagation()} onTouchMove={event => event.stopPropagation()}>
    <div ref={panel} className={selection ? 'kota-confirm-card plain-copy soul-recruit-confirm' : 'shortcut-recruit-panel soul-recruit-panel'}
      role={selection ? 'alertdialog' : 'dialog'} aria-modal="true" aria-labelledby="shortcut-recruit-title" tabIndex={-1}
      onClick={event => event.stopPropagation()} onKeyDown={event => {
        if (event.key !== 'Tab') return;
        const controls = Array.from(panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled):not([tabindex="-1"]), [tabindex="0"]') ?? []).filter(node => !node.closest('[hidden]'));
        const first = controls[0], last = controls[controls.length - 1];
        if (!first) { event.preventDefault(); panel.current?.focus(); }
        else if (event.shiftKey && (document.activeElement === first || document.activeElement === panel.current)) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
      }}>
      {selection ? <>
        <h2 id="shortcut-recruit-title">Reincarnate {selection.preview.heroName}?</h2>
        <div className="soul-recruit-origin">
          <span className={`working-hero-avatar tavern-avatar-art ${avatarClassForId(selectedPerson?.avatarId, selectedPerson?.cli)}`} style={avatarImageStyleForId(selectedPerson?.avatarId)} aria-hidden><span /><i /><b /></span>
          <div>
            <small>{sameProjectSelection ? 'from this project' : `from project ${selection.preview.sourceProjectName}`}</small>
            <strong>{selection.preview.sourceAgentName ?? selection.preview.heroName}</strong>
          </div>
        </div>
        <p>The original agent will not be removed.</p>
        <p>{sameProjectSelection ? SAME_PROJECT_COPY : `The invited agent may bring information from project ${selection.preview.sourceProjectName}.`}</p>
        {selection.preview.reason && <p className="soul-recruit-reason">{selection.preview.reason}</p>}
        {error && <p role="alert" className="soul-recruit-error">{error}</p>}
        <div className="kota-confirm-actions">
          <button type="button" data-cancel-recruit disabled={pending} onClick={dismiss}>No</button>
          <button type="button" className="confirm" disabled={pending} onClick={() => void deliver(selection)}>Yes</button>
        </div>
      </> : <>
        <div className="shortcut-recruit-head">
          <div><div className="shortcut-recruit-kicker">Seat {seatNumber}</div><div id="shortcut-recruit-title" className="shortcut-recruit-title">Add agent</div></div>
          <button type="button" className="shortcut-recruit-close" onClick={dismiss} disabled={delivering.current} aria-label="Close add agent dialog">×</button>
        </div>
        {tabs.length > 1 && (
          <div className="soul-recruit-tabs-bar">
            <span className="soul-recruit-from-label">From :</span>
            <div className="soul-recruit-tabs" role="tablist" aria-label="Recruit source">
              {tabs.map(value => (
                <button
                  key={value}
                  type="button"
                  role="tab"
                  id={`recruit-tab-${value}`}
                  className="soul-recruit-tab"
                  aria-selected={tab === value}
                  tabIndex={tab === value ? 0 : -1}
                  aria-controls={`recruit-panel-${value}`}
                  disabled={pending}
                  onClick={() => selectTab(value)}
                  onKeyDown={event => {
                    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
                    event.preventDefault();
                    const index = event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1
                      : (tabs.indexOf(value) + (event.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length;
                    const next = tabs[index]!;
                    selectTab(next);
                    event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[index]?.focus();
                  }}
                >
                  {tabLabel(value)}
                </button>
              ))}
            </div>
          </div>
        )}
        <div className="soul-recruit-scroll">
          <section id="recruit-panel-tavern" className="soul-recruit-tabpanel" role="tabpanel" aria-labelledby="recruit-tab-tavern" hidden={tab !== 'tavern'}>
            <WorkingHeroPicker heroes={visibleHeroes} unavailableHeroIds={unavailableHeroIds} onSelect={hero => void selectHero(hero)} testIdPrefix="incarnate-shortcut" variant="modal" showHotkeys disabled={pending} />
            {heroes.length > TAVERN_PREVIEW_COUNT && <button type="button" className="soul-recruit-show" onClick={() => setShowAll(value => !value)}>{showAll ? 'Show fewer' : `Show all ${heroes.length}`}</button>}
          </section>
          {onSelectIncarnation && projectRoot && thisOpened && (
            <section id="recruit-panel-this" className="soul-recruit-tabpanel" role="tabpanel" aria-labelledby="recruit-tab-this" hidden={tab !== 'this'}>
              <ThisProjectIncarnations projectRoot={projectRoot} disabled={pending} agentsRef={thisAgentsRef} onSelect={identity => void selectIncarnation(projectRoot, identity)} />
            </section>
          )}
          {onSelectIncarnation && otherOpened && (
            <section id="recruit-panel-other" className="soul-recruit-tabpanel" role="tabpanel" aria-labelledby="recruit-tab-other" hidden={tab !== 'other'}>
              <OtherProjectIncarnations projectRoot={projectRoot} disabled={pending} onSelect={(project, identity) => void selectIncarnation(project.localRoot, identity)} />
            </section>
          )}
          {pending && <p role="status">Preparing incarnation profile.</p>}
          {error && <p role="alert" className="soul-recruit-error">{error}</p>}
        </div>
      </>}
    </div>
  </div>, document.body);
}

function IncarnationPills({ agents, disabled, showHotkeys = false, onSelect }: {
  agents: readonly ProjectAgentIdentity[];
  disabled: boolean;
  showHotkeys?: boolean;
  onSelect: (agent: ProjectAgentIdentity) => void;
}) {
  return (
    <div className="working-hero-picker modal">
      {agents.map((agent, index) => {
        const nameParts = splitProjectAgentName(agent.displayName);
        return (
          <button
            key={agent.agentId}
            type="button"
            className={`working-hero-pill soul-recruit-incarnation ${disabled ? 'pending' : ''}`}
            title={agent.displayName}
            data-full-name={agent.displayName}
            disabled={disabled}
            onClick={() => onSelect(agent)}
          >
            <span
              className={`working-hero-avatar tavern-avatar-art ${avatarClassForId(agent.avatarId, agent.provider ?? 'codex')}`}
              style={avatarImageStyleForId(agent.avatarId)}
              aria-hidden
            >
              <span /><i /><b />
            </span>
            <span className="working-hero-copy">
              {nameParts.title && (
                <span className={`working-hero-name-title title-${nameParts.title.id}`}>
                  {nameParts.title.abbr}
                </span>
              )}
              <b>{nameParts.base}</b>
            </span>
            {showHotkeys && index < 9 && <kbd className="working-hero-hotkey" aria-hidden>{index + 1}</kbd>}
          </button>
        );
      })}
    </div>
  );
}

function ThisProjectIncarnations({ projectRoot, disabled, agentsRef, onSelect }: {
  projectRoot: string;
  disabled: boolean;
  agentsRef: { current: readonly ProjectAgentIdentity[] };
  onSelect: (identity: ProjectAgentIdentity) => void;
}) {
  const [agents, setAgents] = useState<ProjectAgentIdentity[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let active = true; setAgents(null); setError(null); agentsRef.current = [];
    void listProjectAgentIdentities(projectRoot).then(items => {
      if (!active) return;
      const filtered = items.filter(item => item.status === 'active');
      agentsRef.current = filtered; setAgents(filtered);
    }).catch(failure => { if (active) setError(failureText(failure)); });
    return () => { active = false; agentsRef.current = []; };
  }, [agentsRef, projectRoot, revision]);
  return <>
    {error ? <p role="alert">{error} <button type="button" onClick={() => setRevision(value => value + 1)}>Retry</button></p>
      : !agents ? <p role="status">Loading agents…</p>
      : !agents.length ? <p className="soul-recruit-empty">No agents in this project yet</p>
      : <IncarnationPills agents={agents} disabled={disabled} showHotkeys onSelect={onSelect} />}
  </>;
}

function OtherProjectIncarnations({ projectRoot, disabled, onSelect }: {
  projectRoot?: string | null; disabled: boolean; onSelect: (project: WorkspaceProject, identity: ProjectAgentIdentity) => void;
}) {
  const [projects, setProjects] = useState<WorkspaceProject[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let active = true; setProjects(null); setError(null);
    void listWorkspaceProjects().then(items => {
      if (active) setProjects([...new Map(items.filter(item => !item.archived && item.localRoot !== projectRoot).map(item => [item.localRoot, item])).values()]);
    }).catch(failure => { if (active) setError(failureText(failure)); });
    return () => { active = false; };
  }, [projectRoot, revision]);
  return <>
    {error ? <p role="alert">{error} <button type="button" onClick={() => setRevision(value => value + 1)}>Retry</button></p>
      : !projects ? <p role="status">Loading projects…</p>
      : !projects.length ? <p className="soul-recruit-empty">No other projects</p>
      : projects.map(project => <ProjectIncarnations key={project.localRoot} project={project} disabled={disabled} onSelect={onSelect} />)}
  </>;
}

function ProjectIncarnations({ project, disabled, onSelect }: {
  project: WorkspaceProject; disabled: boolean; onSelect: (project: WorkspaceProject, identity: ProjectAgentIdentity) => void;
}) {
  const [open, setOpen] = useState(false);
  const [agents, setAgents] = useState<ProjectAgentIdentity[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    if (!open) return;
    let active = true; setAgents(null); setError(null);
    void listProjectAgentIdentities(project.localRoot).then(items => { if (active) setAgents(items.filter(item => item.status === 'active')); })
      .catch(failure => { if (active) setError(failureText(failure)); });
    return () => { active = false; };
  }, [open, project.localRoot, revision]);
  return <div className="soul-recruit-project">
    <button type="button" className="soul-recruit-project-toggle" aria-expanded={open} onClick={() => setOpen(value => !value)}><span>{project.displayName || project.repoFullName || project.projectId}</span><span>{agents ? `${agents.length} ${agents.length === 1 ? 'agent' : 'agents'}` : ''} {open ? '▾' : '▸'}</span></button>
    {open && <div className="soul-recruit-agents">
      {error ? <p role="alert">{error} <button type="button" onClick={() => setRevision(value => value + 1)}>Retry</button></p>
        : !agents ? <p role="status">Loading agents…</p>
        : !agents.length ? <p className="soul-recruit-empty">No active incarnations</p>
        : <IncarnationPills agents={agents} disabled={disabled} onSelect={agent => onSelect(project, agent)} />}
    </div>}
  </div>;
}
