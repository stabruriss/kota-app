import { AGENTS } from '../mock/fixtures';
import type { AgentAtTable } from '../types/agentbar';
import type { Agent, AgentId } from '../types/scene';

export interface PrivacyPopoverProps {
  agents: readonly AgentAtTable[];
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  privateAgents: ReadonlySet<AgentId>;
  liveAgents: ReadonlySet<AgentId>;
  onToggleAgent: (id: AgentId) => void;
  onToggleAll: () => void;
}

export function PrivacyPopover({
  agents,
  agentMeta,
  privateAgents,
  liveAgents,
  onToggleAgent,
  onToggleAll,
}: PrivacyPopoverProps) {
  const privateCount = agents.filter((agent) => privateAgents.has(agent.id)).length;
  const allState =
    agents.length > 0 && privateCount === agents.length ? 'on' :
    privateCount > 0 ? 'mixed' :
    'off';

  return (
    <div
      className="privacy-popover"
      role="dialog"
      aria-label="Privacy"
      data-testid="privacy-popover"
    >
      <div className="privacy-title">Privacy</div>
      <button
        type="button"
        className="privacy-row all"
        onClick={onToggleAll}
        data-testid="privacy-all"
      >
        <span className="privacy-agent-main">
          <span className="privacy-agent-name">All agents</span>
        </span>
        <PrivacySwitch state={allState} />
      </button>

      <div className="privacy-divider" />

      <div className="privacy-agent-list">
        {agents.map((agent) => {
          const meta = agentMeta?.[agent.id] ?? AGENTS[agent.id] ?? {
            name: agent.id,
            emoji: '',
            role: 'Working agent',
            hue: '',
          };
          const live = liveAgents.has(agent.id);
          const privateState = privateAgents.has(agent.id);
          return (
            <button
              key={agent.id}
              type="button"
              className={['privacy-row', live ? 'live' : 'idle'].join(' ')}
              onClick={() => onToggleAgent(agent.id)}
              aria-pressed={privateState}
              data-testid={`privacy-agent-${agent.id}`}
            >
              <span className={`privacy-live-dot ${live ? 'live' : 'idle'}`} aria-hidden />
              <span className="privacy-agent-main">
                <span className="privacy-agent-name">
                  {meta.name}
                  {agent.captain && <span className="privacy-star">★</span>}
                </span>
                <span className="privacy-agent-role">{shortRole(meta.role)}</span>
              </span>
              <PrivacySwitch state={privateState ? 'on' : 'off'} />
            </button>
          );
        })}
      </div>

      <div className="privacy-hint">Click an agent to toggle privacy</div>
    </div>
  );
}

function PrivacySwitch({ state }: { state: 'off' | 'on' | 'mixed' }) {
  return (
    <span
      className={['privacy-switch', state].join(' ')}
      aria-hidden
    >
      <span className="privacy-switch-knob" />
    </span>
  );
}

function shortRole(role: string): string {
  const i = role.indexOf(' ·');
  return i > 0 ? role.slice(0, i) : role;
}
