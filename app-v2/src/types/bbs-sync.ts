/** Frontend/backend IPC contract agreed with the backend owner, 2026-09-12.
 * Not the P2P manifest. No private key, invitation token or image bytes here. */
export const BBS_SYNC_INDICATORS = [
  'healthy', 'connecting', 'reaching_service', 'reaching_peers', 'fetching_session',
  'finishing_sync', 'retrying_files', 'checking_protocol', 'reconnecting', 'cloudflare_limit',
  'update_worker', 'update_kota', 'group_access_denied', 'device_identity_error',
  'other_instance', 'file_access_error',
] as const;
export type BbsSyncIndicator = typeof BBS_SYNC_INDICATORS[number];

export interface BbsSyncStatus {
  protocolVersion: 1;
  device: { id: string; name: string };
  worker: { configured: boolean; canCreateGroup: boolean };
  group: {
    id: string | null;
    name: string | null;
    role: 'owner' | 'member' | null;
    members: BbsSyncMember[];
  };
  invitation: 'none' | 'preparing' | 'ready' | 'error';
  /** Canonical decimal, compared only for equality. Null without an owner code. */
  invitationGeneration: string | null;
  sync: {
    phase: 'idle' | 'connecting' | 'syncing' | 'partial' | 'failed';
    completed: number | null;
    total: number | null;
    lastSuccessfulAt: string | null;
    /** Backend-owned, display-safe diagnostic; never an HTTP response dump. */
    error: string | null;
    /** Explicit Retry can restore a joined device whose control worker is absent. */
    controlRecoverable: boolean;
    /** Explicit Retry may probe a daily-quota circuit breaker once; never a poll. */
    serviceRecoverable: boolean;
    /** Backend-owned recovery presentation; absent only in older protocol-1 snapshots. */
    indicator?: BbsSyncIndicator;
  };
}

export interface BbsSyncMember {
  id: string;
  name: string;
  role: 'owner' | 'member';
  online: boolean;
  publicKey: string;
}

/** Every mutation is fenced against the group visible when it was requested. */
export interface BbsSyncCommand { expectedGroupId: string | null }
export interface BbsSyncInvitationRequest extends BbsSyncCommand { refresh: boolean }
export interface BbsSyncJoinRequest extends BbsSyncCommand { invitation: string }
export interface BbsSyncRenameRequest extends BbsSyncCommand { name: string }
export interface BbsSyncRemoveRequest extends BbsSyncCommand { deviceId: string }

/** Sensitive: returned only by an explicit management read/action, never status
 * or changed events. Keep it in the open management surface, not a shared store. */
export interface BbsSyncInvitationResult { groupId: string; generation: string; invitation: string }
