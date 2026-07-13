import type { AgentId, SeatState } from './scene';
import type { AgentCli } from './agent-pty';

/** Row model for an agent currently sitting at the round table. */
export interface AgentAtTable {
  id: AgentId;
  state: Exclude<SeatState, 'empty'>;
  captain?: boolean;
}

/** Row model for an agent parked off the table (still in the session,
 *  not occupying a seat). */
export interface OffTableAgent {
  id: AgentId;
  /** Whether the agent's PTY is actively emitting output. */
  live: boolean;
}

/** Working hero visible in the quick incarnate picker. */
export interface WorkingHero {
  id: AgentId;
  /** Template hero id when this is a project-local incarnation. */
  templateId?: AgentId;
  cli: AgentCli;
  name: string;
  record: string;
  avatarId?: string | null;
  avatarClass?: string;
  available?: boolean;
  unavailableReason?: string;
}
