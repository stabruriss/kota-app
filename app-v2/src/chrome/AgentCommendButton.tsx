import { useEffect, useState } from 'react';
import type { ProjectAgentCommendSource } from '../pty-client';
import iconCommends from '../assets/tavern/icons/commends.svg';
import type { AgentId } from '../types/scene';

export function AgentCommendButton({
  agentId,
  agentName,
  source,
  count,
  onCommend,
}: {
  agentId: AgentId;
  agentName: string;
  source: ProjectAgentCommendSource;
  count?: number;
  onCommend?: (id: AgentId, source: ProjectAgentCommendSource) => void;
}) {
  const [confirmed, setConfirmed] = useState(false);

  useEffect(() => {
    if (!confirmed) return;
    const timer = window.setTimeout(() => setConfirmed(false), 850);
    return () => window.clearTimeout(timer);
  }, [confirmed]);

  if (!onCommend) return null;

  return (
    <button
      type="button"
      className={`agent-commend-button ${confirmed ? 'commended' : ''}`}
      title={`Commend ${agentName}`}
      aria-label={`Commend ${agentName}`}
      onPointerDown={(event) => {
        event.preventDefault();
        event.stopPropagation();
      }}
      onClick={(event) => {
        event.preventDefault();
        event.stopPropagation();
        setConfirmed(true);
        onCommend(agentId, source);
      }}
    >
      <img src={iconCommends} alt="" aria-hidden />
      {count && count > 0 ? <span>{count}</span> : null}
    </button>
  );
}
