import { useEffect, useState } from 'react';
import type { AgentId } from '../types/scene';

export const FILE_TREE_AGENT_HOVER_EVENT = 'kota:file-tree-agent-hover';

interface FileTreeAgentHoverDetail {
  agentId: AgentId;
  active: boolean;
}

export function emitFileTreeAgentHover(agentId: AgentId, active: boolean) {
  if (typeof window === 'undefined') return;
  window.dispatchEvent(new CustomEvent<FileTreeAgentHoverDetail>(FILE_TREE_AGENT_HOVER_EVENT, {
    detail: { agentId, active },
  }));
}

export function useFileTreeAgentHover(): AgentId | null {
  const [hoveredAgent, setHoveredAgent] = useState<AgentId | null>(null);

  useEffect(() => {
    const onHover = (event: Event) => {
      const detail = (event as CustomEvent<FileTreeAgentHoverDetail>).detail;
      if (!detail?.agentId) return;
      setHoveredAgent((current) => {
        if (detail.active) return detail.agentId;
        return current === detail.agentId ? null : current;
      });
    };
    window.addEventListener(FILE_TREE_AGENT_HOVER_EVENT, onHover);
    return () => window.removeEventListener(FILE_TREE_AGENT_HOVER_EVENT, onHover);
  }, []);

  return hoveredAgent;
}
