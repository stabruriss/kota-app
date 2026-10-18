import type { LocalStartupOptions } from '../lib/local-startup';
import type { Agent, AgentId } from '../types/scene';
import type { ProjectAgentRecord } from '../pty-client';
import { useLocalQueue, type LocalQueueSend } from '../hooks/useLocalQueue';
import { LocalAgentWorkbench } from './LocalAgentWorkbench';

interface Props {
  projectRoot: string | null;
  open: { projectRoot: string; agentId: AgentId; section?: 'queue'; focusRequest: number; resumeOnOpen?: boolean; linkReturnRequest?: number } | null;
  agents: Readonly<Record<AgentId, Agent>>; records: Readonly<Record<AgentId, ProjectAgentRecord>>;
  targets: readonly AgentId[];
  privateAgents?: ReadonlySet<AgentId>;
  onClose: () => void; onDetails: (id: AgentId) => void; onFresh: (id: AgentId) => void;
  onResume: (id: AgentId, startup?: LocalStartupOptions) => Promise<unknown>; onSend: LocalQueueSend;
  onGuardsChange: (keys: ReadonlySet<string>) => void;
}
/** Only the small editor owner survives hiding; visible streaming stays unmounted. */
export function LocalWorkbenchHost({ projectRoot, open, agents, records, targets, privateAgents, onGuardsChange, onSend, ...callbacks }: Props) {
  const queue = useLocalQueue(projectRoot, onGuardsChange, onSend, privateAgents);
  if (!open || open.projectRoot !== projectRoot || !agents[open.agentId]) return null;
  return <LocalAgentWorkbench key={`${open.projectRoot}:${open.agentId}`} {...open} agent={agents[open.agentId]!}
    agents={agents} record={records[open.agentId]} targets={targets} privateMessage={privateAgents?.has(open.agentId)} queueController={queue} {...callbacks} />;
}
