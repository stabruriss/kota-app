/** Presentation data, not the transport wire protocol. No IO or persistent state. */
import type { BbsSyncIndicator } from './types/bbs-sync';

export interface BbsSyncMemberView {
  id: string;
  name: string;
  role: 'owner' | 'member';
  online: boolean;
}

export interface BbsSyncGroupView {
  id: string;
  name: string;
  role: 'owner' | 'member';
  members: BbsSyncMemberView[];
}

export type BbsSyncInvitationView =
  | { state: 'none' | 'preparing' | 'unavailable' }
  | { state: 'ready'; value: string }
  | { state: 'error'; message: string };

export interface BbsSyncView {
  deviceId: string;
  deviceName: string;
  workerAvailable: boolean;
  group: BbsSyncGroupView | null;
  invitation: BbsSyncInvitationView;
  /** Public invalidation key only; raw invitation stays in the open dialog. */
  invitationGeneration?: string | null;
  phase: 'idle' | 'connecting' | 'syncing' | 'partial' | 'failed';
  progress: { completed: number; total: number } | null;
  lastSuccessfulAt: string | null;
  error: string | null;
  /** Only the backend may authorize recovery without an online peer. */
  controlRecoverable: boolean;
  /** Backend grants one explicit service-recovery attempt without online peers. */
  serviceRecoverable: boolean;
  /** Typed presentation, never inferred from a backend English error. */
  indicator?: BbsSyncIndicator;
}

export function bbsSyncOnlinePeers(view: BbsSyncView): number {
  return view.group?.members.filter((member) => member.id !== view.deviceId && member.online).length ?? 0;
}

/** A preview only. The backend must validate the complete invitation before use. */
export function bbsInvitationHost(value: string): string | null {
  if (value.length > 4096 || /[\u0000-\u001f\u007f]/.test(value.trim())) return null;
  try {
    const url = new URL(value.trim());
    if (url.protocol !== 'kota-bbs:' || !url.hostname || url.pathname !== '/join'
      || url.username || url.password || url.port || url.search || !url.hash.slice(1)) return null;
    return url.hostname;
  } catch {
    return null;
  }
}

export function bbsSyncTimeLabel(value: string | null): string {
  if (!value) return 'Not synced yet';
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return 'Not synced yet';
  return `Last sync at ${date.toLocaleString(undefined, {
    month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit',
  })}`;
}
