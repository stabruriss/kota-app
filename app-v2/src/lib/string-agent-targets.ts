import { useEffect, useRef, useState } from 'react';
import { listWorkspaceProjects, listProjectAgentIdentities, loadProjectAgentDetail, type ProjectAgentDetail, type WorkspaceProject } from '../pty-client';
import { isStringAgent } from './puppeteer-projects';

export interface StringSource { projectRoot?: string; agentId: string }
export interface StringTarget { key: string; project: WorkspaceProject; agent: ProjectAgentDetail }
/** Mounted only by Apply. Active projects, serial reads, no model discovery or polling. */
export function useStringAgentTargets(exclude?: StringSource) {
  const [targets, setTargets] = useState<StringTarget[]>([]);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const [revision, setRevision] = useState(0);
  const sequence = useRef(0);
  useEffect(() => {
    const serial = ++sequence.current;
    const active = () => serial === sequence.current;
    setLoading(true); setFailed(false);
    void (async () => {
      const next: StringTarget[] = [];
      let partial = false;
      try {
        const projects = await listWorkspaceProjects();
        for (const project of new Map(projects.map(item => [item.localRoot, item])).values()) {
          if (!active()) return;
          try {
            const agents = await listProjectAgentIdentities(project.localRoot);
            for (const agent of agents) {
              if (!active()) return;
              if (agent.status !== 'active' || (agent.provider && agent.provider !== 'kota-local')) continue;
              if (agent.agentId === exclude?.agentId && (!exclude.projectRoot || exclude.projectRoot === project.localRoot)) continue;
              try {
                const detail = await loadProjectAgentDetail({ projectRoot: project.localRoot, agentId: agent.agentId }, { readOnly: true });
                if (isStringAgent(detail)) next.push({ key: `${project.localRoot}\0${agent.agentId}`, project, agent: detail });
              } catch { partial = true; }
            }
          } catch { partial = true; }
        }
      } catch { partial = true; }
      if (active()) { setTargets(next); setFailed(partial); setLoading(false); }
    })();
    return () => { ++sequence.current; };
  }, [exclude?.projectRoot, exclude?.agentId, revision]);
  return { targets, loading, failed, reload: () => setRevision(value => value + 1) };
}
