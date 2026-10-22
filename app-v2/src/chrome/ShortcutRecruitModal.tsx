import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { WorkingHero } from '../types/agentbar';
import type { AgentId } from '../types/scene';
import { listWorkspaceProjects, listProjectAgentIdentities, loadProjectAgentDetail, type ProjectAgentIdentity, type WorkspaceProject } from '../pty-client';
import { readRecruitPreview, type IncarnationRecruitSource, type RecruitPreview } from '../lib/reincarnation';
import { avatarClassForId, avatarImageStyleForId } from '../lib/hero-avatars';
import { WorkingHeroPicker } from './WorkingHeroPicker';
import '../styles/recruit-picker.css';

type Selection = { hero: WorkingHero; preview: RecruitPreview } | { source: IncarnationRecruitSource; preview: RecruitPreview };
const TAVERN_PREVIEW_COUNT = 4;
const failureText = (error: unknown) => error instanceof Error ? error.message : String(error);

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
  const [pending, setPending] = useState(false);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tavernOpen, setTavernOpen] = useState(true);
  const [showAll, setShowAll] = useState(false);
  const [otherOpen, setOtherOpen] = useState(true);
  const visibleHeroes = tavernOpen ? showAll ? heroes : heroes.slice(0, TAVERN_PREVIEW_COUNT) : [];
  const selectedPerson = selection && ('hero' in selection ? selection.hero : selection.source.agent);

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
  const selectIncarnation = async (project: WorkspaceProject, identity: ProjectAgentIdentity) => {
    if (pendingRef.current) return;
    pendingRef.current = true; setPending(true); setError(null);
    try {
      const agent = await loadProjectAgentDetail({ projectRoot: project.localRoot, agentId: identity.agentId }, { readOnly: true });
      if (!mounted.current) return;
      const preview = await readRecruitPreview({ sourceProjectRoot: project.localRoot, sourceAgentId: identity.agentId });
      if (mounted.current) setSelection({ source: { projectRoot: project.localRoot, agent }, preview });
    } catch (failure) { if (mounted.current) setError(failureText(failure)); }
    finally { pendingRef.current = false; if (mounted.current) setPending(false); }
  };
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.isComposing) return;
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); dismiss(); return; }
      if (selection || pendingRef.current || event.metaKey || event.ctrlKey || event.altKey || !/^[1-9]$/.test(event.key)) return;
      const hero = visibleHeroes[Number(event.key) - 1];
      if (!hero || hero.available === false || unavailableHeroIds?.has(hero.id)) return;
      event.preventDefault(); event.stopPropagation(); void selectHero(hero);
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
        const controls = Array.from(panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled), [tabindex="0"]') ?? []).filter(node => !node.closest('[hidden]'));
        const first = controls[0], last = controls[controls.length - 1];
        if (!first) { event.preventDefault(); panel.current?.focus(); }
        else if (event.shiftKey && (document.activeElement === first || document.activeElement === panel.current)) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
      }}>
      {selection ? <>
        <h2 id="shortcut-recruit-title">Reincarnate {selection.preview.heroName}?</h2>
        <div className="soul-recruit-origin">
          <span className={`working-hero-avatar tavern-avatar-art ${avatarClassForId(selectedPerson?.avatarId, selectedPerson?.cli)}`} style={avatarImageStyleForId(selectedPerson?.avatarId)} aria-hidden><span /><i /><b /></span>
          <div><small>from project {selection.preview.sourceProjectName}</small><strong>{selection.preview.sourceAgentName ?? selection.preview.heroName}</strong></div>
        </div>
        <p>The original agent will not be removed.</p>
        <p>The invited agent may bring information from project {selection.preview.sourceProjectName}.</p>
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
        <div className="soul-recruit-scroll">
          <section className="soul-recruit-group">
            <button type="button" className="soul-recruit-group-toggle" aria-expanded={tavernOpen} onClick={() => setTavernOpen(value => !value)}><span>From Tavern</span><span>{heroes.length} heroes {tavernOpen ? '▾' : '▸'}</span></button>
            {tavernOpen && <>
              <WorkingHeroPicker heroes={visibleHeroes} unavailableHeroIds={unavailableHeroIds} onSelect={hero => void selectHero(hero)} testIdPrefix="incarnate-shortcut" variant="modal" showHotkeys disabled={pending} />
              {heroes.length > TAVERN_PREVIEW_COUNT && <button type="button" className="soul-recruit-show" onClick={() => setShowAll(value => !value)}>{showAll ? 'Show fewer' : `Show all ${heroes.length}`}</button>}
            </>}
          </section>
          {onSelectIncarnation && <section className="soul-recruit-group">
            <button type="button" className="soul-recruit-group-toggle" aria-expanded={otherOpen} onClick={() => setOtherOpen(value => !value)}><span>From other project</span><span>{otherOpen ? '▾' : '▸'}</span></button>
            <div hidden={!otherOpen}><OtherProjectIncarnations projectRoot={projectRoot} disabled={pending} onSelect={selectIncarnation} /></div>
          </section>}
          {pending && <p role="status">Preparing incarnation profile.</p>}
          {error && <p role="alert" className="soul-recruit-error">{error}</p>}
        </div>
      </>}
    </div>
  </div>, document.body);
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
        : agents.map(agent => <button key={agent.agentId} type="button" className="soul-recruit-incarnation" disabled={disabled} onClick={() => onSelect(project, agent)}>
          <span className={`working-hero-avatar tavern-avatar-art ${avatarClassForId(agent.avatarId, agent.provider ?? 'codex')}`} style={avatarImageStyleForId(agent.avatarId)} aria-hidden><span /><i /><b /></span><span>{agent.displayName}</span>
        </button>)}
    </div>}
  </div>;
}
