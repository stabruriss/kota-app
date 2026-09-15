/** Public roster projection only. Never contains paths, credentials or image bytes. */
export type BbsRosterAvatar =
  | { kind: 'builtin'; id: string }
  | { kind: 'image'; sha256: string; ext: 'png' | 'jpg' | 'webp'; sizeBytes: number; available: boolean }
  | { kind: 'none' };

export interface BbsMentionTarget {
  deviceId: string;
  projectId: string;
  agentId: string;
}

export interface BbsRosterAgent {
  agentId: string;
  name: string;
  targetRef: string;
  avatar: BbsRosterAvatar;
}
export interface BbsRosterProject {
  projectId: string;
  name: string;
  agents: readonly BbsRosterAgent[];
}
export interface BbsRosterDevice {
  deviceId: string | null;
  name: string;
  local: boolean;
  online: boolean;
  rosterStatus: 'synced' | 'not_synced';
  receivedAt: string | null;
  projects: readonly BbsRosterProject[] | null;
}
export interface BbsRosterView {
  version: string;
  devices: readonly BbsRosterDevice[];
}
export type BbsRosterRow =
  | ({ kind: 'device' } & Omit<BbsRosterDevice, 'projects'>)
  | { kind: 'project'; deviceId: string | null; projectId: string; name: string }
  | ({ kind: 'agent'; deviceId: string | null; projectId: string } & BbsRosterAgent);
export interface BbsRosterPage {
  version: string;
  items: BbsRosterRow[];
  next: string | null;
}

/** Display snapshot preserves selected names even if a later roster removes them.
 * Only the three target IDs are submitted; names never become routing addresses. */
export interface BbsMentionSelection extends BbsMentionTarget {
  deviceName: string;
  projectName: string;
  agentName: string;
}
