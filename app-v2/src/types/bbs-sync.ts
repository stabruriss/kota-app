/** Frontend/backend IPC contract agreed with the backend owner, 2026-09-12.
 * Not the P2P manifest. No private key, invitation token or image bytes here. */
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
