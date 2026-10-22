import { useEffect, useRef, useState } from 'react';
import { listWorkspaceProjects, listProjectAgentIdentities, loadProjectAgentDetail, loadProjectAgentLayoutFile, type ProjectAgentDetail, type WorkspaceProject } from '../pty-client';
import { isStringAgent, saveAgentString } from '../lib/puppeteer-projects';
import { useDanceSettings, type DanceAgent } from '../lib/dance-settings';
import { useTavernLocal } from '../lib/tavern-local';
import { useJevSettings } from '../lib/jev';
import { stringDraftToGroup, stringGroupToDraft, stringErrorText, type StringRoutingDraft } from '../lib/string-routing';
import { StringRoutingEditor } from './StringRoutingEditor';
import { HeroAvatarArt } from './HeroAvatarPicker';
import { JevProviderSettings } from './JevProviderSettings';
import { PuppeteerPromptFile } from './PuppeteerPromptFile';
import { PuppeteerDanceCard } from './PuppeteerDanceCard';
import { PuppeteerMarkAndDreamSettings } from './PuppeteerMarkAndDreamSettings';

interface ProjectRow { project: WorkspaceProject; agents: ProjectAgentDetail[]; seats: DanceAgent[]; error: boolean }
const TABS = ['string', 'dance', 'memory'] as const;
type PuppeteerTab = typeof TABS[number];
const tabLabel = (tab: PuppeteerTab) => tab === 'memory' ? 'Mark and Dream' : tab === 'string' ? 'String' : 'Dance';

export function PuppeteerTavernConfig({ jevFocusRequest = 0 }: { jevFocusRequest?: number }) {
  const [tab, setTab] = useState<PuppeteerTab>('string');
  const [memoryOpened, setMemoryOpened] = useState(false);
  const [memoryJevFocus, setMemoryJevFocus] = useState(0);
  const selectTab = (value: PuppeteerTab) => { setTab(value); if (value === 'memory') setMemoryOpened(true); };
  const [rows, setRows] = useState<ProjectRow[]>([]);
  const [loading, setLoading] = useState(true); const [failed, setFailed] = useState(false); const [revision, setRevision] = useState(0);
  const [editing, setEditing] = useState<{ row: ProjectRow; agent: ProjectAgentDetail; draft: StringRoutingDraft } | null>(null);
  const [busy, setBusy] = useState(false); const busyRef = useRef(false);
  const [error, setError] = useState<string | null>(null);
  const local = useTavernLocal(!!editing); const jev = useJevSettings(!!editing);
  useEffect(() => {
    if (tab === 'memory') return;
    let active = true; setLoading(true); setFailed(false);
    void listWorkspaceProjects().then(async current => {
      const projects = [...new Map(current.map(project => [project.localRoot, project])).values()];
      const next: ProjectRow[] = [];
      // Bounded, serial project inspection. No Pi/model/network work in the overview.
      for (const project of projects) {
        if (!active) return;
        try {
          const identities = await listProjectAgentIdentities(project.localRoot);
          if (tab === 'string') {
            const agents: ProjectAgentDetail[] = [];
            for (const identity of identities.filter(agent => agent.status === 'active' && (!agent.provider || agent.provider === 'kota-local'))) {
              const detail = await loadProjectAgentDetail({ projectRoot: project.localRoot, agentId: identity.agentId });
              if (isStringAgent(detail)) agents.push(detail);
            }
            next.push({ project, agents, seats: [], error: false });
          } else {
            const layout = await loadProjectAgentLayoutFile(project.localRoot);
            if (!layout) throw new Error('roster unavailable');
            const byId = new Map(identities.map(agent => [agent.agentId, agent]));
            const seats = layout.tableSlots.slice(0, 8).flatMap(id => { const agent = id ? byId.get(id) : null; return agent?.status === 'active' ? [{ id: agent.agentId, name: agent.displayName, avatarId: agent.avatarId }] : []; });
            next.push({ project, agents: [], seats, error: false });
          }
        } catch { next.push({ project, agents: [], seats: [], error: true }); }
      }
      if (active) setRows(next);
    }).catch(() => { if (active) setFailed(true); }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [tab, revision]);
  const accept = (root: string, detail: ProjectAgentDetail) => setRows(previous => previous.map(row => row.project.localRoot !== root ? row : { ...row, agents: row.agents.map(agent => agent.agentId === detail.agentId ? detail : agent) }));
  const close = () => { if (!busyRef.current) { setEditing(null); setError(null); } };
  const save = async () => {
    const group = editing && stringDraftToGroup(editing.draft); if (!editing || !group || busyRef.current) return;
    busyRef.current = true; setBusy(true); setError(null);
    try { const detail = await saveAgentString(editing.row.project.localRoot, editing.agent.agentId, group, editing.agent.shellRevision); accept(editing.row.project.localRoot, detail); setEditing(null); }
    catch (failure) { setError(stringErrorText(failure)); }
    finally { busyRef.current = false; setBusy(false); }
  };
  const openEditor = async (row: ProjectRow, agent: ProjectAgentDetail) => {
    if (busyRef.current) return;
    setError(null); setEditing({ row, agent, draft: stringGroupToDraft(agent.autoRoute) });
    busyRef.current = true; setBusy(true);
    try {
      const fresh = await loadProjectAgentDetail({ projectRoot: row.project.localRoot, agentId: agent.agentId }, { readOnly: true });
      if (!isStringAgent(fresh)) throw { code: 'string_agent_changed' };
      setEditing(previous => previous?.agent.agentId === agent.agentId && previous.row.project.localRoot === row.project.localRoot
        ? { row, agent: fresh, draft: stringGroupToDraft(fresh.autoRoute) } : previous);
    } catch (failure) { setError(stringErrorText(failure)); }
    finally { busyRef.current = false; setBusy(false); }
  };
  const refreshCopiedFile = async () => {
    if (!editing) return;
    try {
      const fresh = await loadProjectAgentDetail({ projectRoot: editing.row.project.localRoot, agentId: editing.agent.agentId }, { readOnly: true });
      accept(editing.row.project.localRoot, fresh);
      // Do not pair an old editor draft with a new baseline, even for a comment-only change.
      if (fresh.shellRevision !== editing.agent.shellRevision) setError('Settings changed outside this editor. Reload before saving.');
    } catch (failure) { setError(stringErrorText(failure)); }
  };
  return <div className="tavern-system-config puppeteer-tavern-config"><JevProviderSettings focusRequest={jevFocusRequest + memoryJevFocus} />
    <div className="puppeteer-tabs" role="tablist" aria-label="Puppeteer configuration">{TABS.map(value => <button key={value} type="button" role="tab" aria-selected={tab === value} tabIndex={tab === value ? 0 : -1} aria-controls={`puppeteer-${value}-projects`} onClick={() => selectTab(value)} onKeyDown={event => {
      if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault(); const index = event.key === 'Home' ? 0 : event.key === 'End' ? TABS.length - 1 : (TABS.indexOf(value) + (event.key === 'ArrowRight' ? 1 : -1) + TABS.length) % TABS.length;
      selectTab(TABS[index]); event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role=tab]')[index]?.focus();
    }}>{tabLabel(value)}</button>)}</div>
    {tab !== 'memory' && <section id={`puppeteer-${tab}-projects`} role="tabpanel" aria-label={tabLabel(tab)}>
      <p>{tab === 'string' ? 'Auto select model for agent' : 'Auto select agent for task'}</p>
      {loading ? <p role="status">Loading projects…</p> : failed ? <p role="status">Projects unavailable. <button type="button" onClick={() => setRevision(value => value + 1)}>Retry</button></p> : <>
        {tab === 'string' && !rows.some(row => row.agents.length) && <p>No Local agents are using Auto String.</p>}
        <div className={tab === 'dance' ? 'puppeteer-dance-projects' : ''}>{rows.filter(row => row.error || tab === 'dance' || row.agents.length).map(row => <section key={row.project.localRoot} className="puppeteer-project"><header><h3>{row.project.displayName || row.project.repoFullName || row.project.projectId}</h3></header>
          {row.error ? <p role="status">Agent roster unavailable. <button type="button" onClick={() => setRevision(value => value + 1)}>Retry</button></p>
            : tab === 'dance' ? <ProjectDance row={row} /> : <div className="puppeteer-string-agents">{row.agents.map(agent => <button type="button" className="puppeteer-string-agent" key={agent.agentId} aria-label={`Edit Auto String for ${agent.displayName} in ${row.project.displayName || row.project.repoFullName || row.project.projectId}`}
              onClick={() => void openEditor(row, agent)}><HeroAvatarArt avatarId={agent.avatarId} className="dance-agent-avatar" />
              <span><b>{agent.displayName}</b><small>{agent.autoRoute?.models.length ?? 0} models · Threshold {(agent.autoRoute?.threshold ?? .5).toFixed(2)}</small></span><i>Edit ›</i></button>)}</div>}
        </section>)}</div>
      </>}
    </section>}
    {memoryOpened && <section id="puppeteer-memory-projects" role="tabpanel" aria-label="Mark and Dream" hidden={tab !== 'memory'}>
      <PuppeteerMarkAndDreamSettings onJevSettings={() => { selectTab('string'); setMemoryJevFocus(value => value + 1); }} />
    </section>}
    <PuppeteerPromptFile />
    {editing && <StringRoutingEditor draft={editing.draft} catalog={local.catalog.data} owner={editing.agent.displayName} surface="tavern" busy={busy}
      applySource={{ projectRoot: editing.row.project.localRoot, agentId: editing.agent.agentId }} onApplied={accept}
      ruleFile={{ projectRoot: editing.row.project.localRoot, kind: 'string', agentId: editing.agent.agentId }}
      ruleFileDirty={JSON.stringify(stringDraftToGroup(editing.draft)) !== JSON.stringify(editing.agent.autoRoute)} onRuleFilePrepared={refreshCopiedFile}
      error={error || (jev.loading ? 'Checking Jev…' : jev.failed ? 'Jev settings unavailable. Retry in Puppeteer.' : !jev.settings?.ready ? 'Set up Jev to use Auto String' : null)} enabled={!!jev.settings?.ready}
      onChange={draft => setEditing(previous => previous ? { ...previous, draft } : previous)} onCatalogOpen={() => void local.openModels()} onClose={close} onConfirm={() => void save()}
      extraAction={error && <button type="button" className="local-model-text" disabled={busy} onClick={() => void openEditor(editing.row, editing.agent)}>Reload</button>} />}
  </div>;
}
function ProjectDance({ row }: { row: ProjectRow }) {
  const controller = useDanceSettings(row.project.localRoot);
  return <PuppeteerDanceCard controller={controller} agents={row.seats} owner={row.project.displayName || row.project.repoFullName || row.project.projectId} />;
}
