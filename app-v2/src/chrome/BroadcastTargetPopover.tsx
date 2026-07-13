import { AGENTS } from '../mock/fixtures';
import { AGENT_SLOT_KEY_RANGE_LABEL } from '../lib/agent-slots';
import type { Agent, AgentId } from '../types/scene';
import { splitProjectAgentName } from './ProjectAgentName';

export interface BroadcastTargetPopoverProps {
  onTableAgents: readonly AgentId[];
  offTableAgents: readonly AgentId[];
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  selectedAgents: ReadonlySet<AgentId>;
  liveAgents: ReadonlySet<AgentId>;
  onToggleAgent: (id: AgentId) => void;
  onConfirm: () => void;
  onCancel: () => void;
  onClear: () => void;
}

export function BroadcastTargetPopover({
  onTableAgents,
  offTableAgents,
  agentMeta,
  selectedAgents,
  liveAgents,
  onToggleAgent,
  onConfirm,
  onCancel,
  onClear,
}: BroadcastTargetPopoverProps) {
  const selectedCount = selectedAgents.size;
  return (
    <div
      className="broadcast-target-popover"
      data-testid="broadcast-target-popover"
      role="dialog"
      aria-label="Broadcast target selection"
    >
      <div className="btp-head">
        <div>
          <div className="btp-title">Broadcast targets</div>
          <div className="btp-sub">{AGENT_SLOT_KEY_RANGE_LABEL} toggles table agents · Enter confirms · Esc cancels</div>
        </div>
        <button
          type="button"
          className="btp-close"
          onClick={onCancel}
          aria-label="Close broadcast target selection"
        >
          ×
        </button>
      </div>

      <TargetGroup
        label="On table"
        agents={onTableAgents}
        agentMeta={agentMeta}
        selectedAgents={selectedAgents}
        liveAgents={liveAgents}
        onToggleAgent={onToggleAgent}
        numbered
      />

      {offTableAgents.length > 0 && (
        <TargetGroup
          label="Off table"
          agents={offTableAgents}
          agentMeta={agentMeta}
          selectedAgents={selectedAgents}
          liveAgents={liveAgents}
          onToggleAgent={onToggleAgent}
        />
      )}

      <div className="btp-actions">
        <button type="button" className="btp-ghost" onClick={onClear}>
          Clear
        </button>
        <div className="btp-count">{selectedCount} selected</div>
        <button
          type="button"
          className="btp-confirm"
          onClick={onConfirm}
          disabled={selectedCount === 0}
          data-testid="broadcast-confirm"
        >
          Confirm
        </button>
      </div>
    </div>
  );
}

function TargetGroup({
  label,
  agents,
  selectedAgents,
  agentMeta,
  liveAgents,
  onToggleAgent,
  numbered,
}: {
  label: string;
  agents: readonly AgentId[];
  selectedAgents: ReadonlySet<AgentId>;
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  liveAgents: ReadonlySet<AgentId>;
  onToggleAgent: (id: AgentId) => void;
  numbered?: boolean;
}) {
  return (
    <div className="btp-group">
      <div className="btp-group-label">{label}</div>
      <div className="btp-chip-grid">
        {agents.map((id, index) => {
          const agent = agentMeta?.[id] ?? AGENTS[id] ?? {
            name: id,
            emoji: '',
            role: '',
            hue: '',
          };
          const selected = selectedAgents.has(id);
          const live = liveAgents.has(id);
          return (
            <button
              key={id}
              type="button"
              className={[
                'btp-chip',
                selected ? 'selected' : '',
                live ? 'live' : 'not-live',
              ].filter(Boolean).join(' ')}
              onClick={() => onToggleAgent(id)}
              aria-pressed={selected}
              data-testid={`broadcast-option-${id}`}
              data-live={live ? 'true' : 'false'}
            >
              {numbered && <span className="btp-num">{index + 1}</span>}
              <span className="btp-dot" aria-hidden />
              <span className="btp-name">{shortAgentName(agent.name)}</span>
              {!live && <span className="btp-muted">not live</span>}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function shortAgentName(name: string): string {
  const parts = splitProjectAgentName(name);
  return [
    parts.title?.abbr,
    parts.base,
    parts.bunshin,
  ].filter(Boolean).join(' ');
}
