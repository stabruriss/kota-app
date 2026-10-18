import { useEffect, useRef, useState } from 'react';
import { applyAgentString } from '../lib/puppeteer-projects';
import { useStringAgentTargets, type StringSource } from '../lib/string-agent-targets';
import type { AutoRouteGroup } from '../lib/string-routing';
import type { ProjectAgentDetail } from '../pty-client';
import { LocalModelDialog } from './LocalModelDialog';
import { HeroAvatarArt } from './HeroAvatarPicker';
import '../styles/string-templates.css';

export function StringApplyDialog({ group, sourceName, exclude, ready, onClose, onApplied }: {
  group: AutoRouteGroup; sourceName: string; exclude?: StringSource; ready: boolean; onClose: () => void;
  onApplied?: (root: string, detail: ProjectAgentDetail) => void;
}) {
  const { targets, loading, failed, reload } = useStringAgentTargets(exclude);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [results, setResults] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false), mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const apply = async () => {
    if (busyRef.current || !ready || loading) return;
    busyRef.current = true; setBusy(true);
    try {
      await applyAgentString(targets.filter(target => selected.has(target.key)).map(({ project, agent }) => ({ projectRoot: project.localRoot, agentId: agent.agentId })), group, (target, detail) => {
        if (!mounted.current) return;
        const key = `${target.projectRoot}\0${target.agentId}`;
        setResults(previous => ({ ...previous, [key]: !!detail }));
        if (detail) {
          onApplied?.(target.projectRoot, detail);
          setSelected(previous => { const next = new Set(previous); next.delete(key); return next; });
        }
      });
    } finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  };
  const projects = [...new Map(targets.map(target => [target.project.localRoot, target.project])).values()];
  return <LocalModelDialog title="Apply Auto String settings" subtitle={`From ${sourceName}`} onClose={() => { if (!busyRef.current) onClose(); }} closeLabel="Close Apply settings"
    footer={<><p role="status">{Object.keys(results).length ? `${Object.values(results).filter(Boolean).length} applied.${Object.values(results).some(value => !value) ? ` ${Object.values(results).filter(value => !value).length} failed. Select them to try again.` : ''}` : 'Copies models, intents, default and threshold.'}</p>
      <button type="button" className="string-ok" disabled={busy || loading || !targets.some(target => selected.has(target.key)) || !ready} onClick={() => void apply()}>{busy ? 'Applying…' : 'Apply'}</button></>}>
    <div className="string-apply-list">
      {loading && <p role="status">Loading projects…</p>}
      {failed && <p role="status">Agent roster unavailable. <button className="local-model-text" disabled={busy || loading} onClick={reload}>Retry</button></p>}
      {!loading && !failed && !targets.length && <p>No other Local agents are using Auto String.</p>}
      {projects.map(project => <fieldset key={project.localRoot} disabled={busy || loading}><legend>{project.repoFullName || project.projectId}</legend>
        {targets.filter(target => target.project.localRoot === project.localRoot).map(target => <label key={target.key}>
          <input type="checkbox" checked={selected.has(target.key)} onChange={event => setSelected(previous => { const next = new Set(previous); if (event.target.checked) next.add(target.key); else next.delete(target.key); return next; })} />
          <HeroAvatarArt avatarId={target.agent.avatarId} className="string-apply-avatar" /><span>{target.agent.displayName}</span>
          {target.key in results && <small>{results[target.key] ? 'Applied' : 'Not applied'}</small>}
        </label>)}
      </fieldset>)}
    </div>
  </LocalModelDialog>;
}
