import type { BbsSyncView } from './bbs-sync-view';
import type { BbsSyncIndicator } from './types/bbs-sync';
import type { BbsSyncSafeActionErrorCode } from './bbs-sync-errors';

export type BbsSyncIndicatorTone = 'green' | 'yellow' | 'red';
interface IndicatorCopy { tone: BbsSyncIndicatorTone; text: string; detail: string }

/** Approved short copy. The backend owns the cause and recovery window; this
 * table neither classifies English errors nor schedules recovery. */
export const BBS_SYNC_INDICATOR_COPY: Record<BbsSyncIndicator, IndicatorCopy> = {
  healthy: { tone: 'green', text: '', detail: 'Device sync is healthy. Online devices are not a guarantee that all content has finished syncing.' },
  connecting: { tone: 'yellow', text: 'Connecting', detail: 'Sync is reconnecting automatically. Manual sync is optional.' },
  reaching_service: { tone: 'yellow', text: 'Reaching Service', detail: 'The sync service is temporarily unreachable. Kota will keep retrying automatically.' },
  reaching_peers: { tone: 'yellow', text: 'Reaching Peers', detail: 'Another device is temporarily unreachable. Kota will keep retrying automatically.' },
  fetching_session: { tone: 'yellow', text: 'Fetching Session', detail: 'The relay session was interrupted. Kota will keep rebuilding it automatically.' },
  finishing_sync: { tone: 'yellow', text: 'Finishing Sync', detail: 'Some content has not finished syncing. Kota will continue automatically.' },
  retrying_files: { tone: 'yellow', text: 'Retrying files', detail: 'A received file failed verification. Kota will retry without installing the invalid file.' },
  checking_protocol: { tone: 'yellow', text: 'Checking Protocol', detail: 'The exchange could not finish correctly. Kota will retry; a version mismatch has not been confirmed.' },
  reconnecting: { tone: 'yellow', text: 'Reconnecting', detail: 'Sync was interrupted. Kota will keep retrying automatically.' },
  cloudflare_limit: { tone: 'yellow', text: 'Cloudflare limit', detail: 'Cloudflare resource limit reached. Kota will retry; if it continues, ask the group owner to check Cloudflare.' },
  update_worker: { tone: 'red', text: 'Update Worker', detail: 'The Worker version is incompatible. Update it from the Laughing Man card.' },
  update_kota: { tone: 'red', text: 'Update Kota', detail: 'A device uses an incompatible sync protocol. Update Kota on that device.' },
  group_access_denied: { tone: 'red', text: 'Group access denied', detail: 'Group access was explicitly denied. Ask the group owner to check this device’s membership.' },
  device_identity_error: { tone: 'red', text: 'Device ID error', detail: 'Device identity is incomplete. Report it on GitHub Discussions for recovery help.' },
  other_instance: { tone: 'red', text: '1 Kota per device', detail: 'Another Kota instance holds the sync lease. Close that instance, then use Manual sync.' },
  file_access_error: { tone: 'red', text: 'File access error', detail: 'A confirmed file access problem is blocking sync. Check disk space and permissions.' },
};

/** Compatibility only: an old snapshot may show a generic recoverable state,
 * but cannot prove an identity/version/lease fault from its English text. */
export function bbsSyncIndicatorForView(view: BbsSyncView): BbsSyncIndicator {
  if (view.indicator) return view.indicator;
  if (view.phase === 'connecting') return 'connecting';
  if (view.phase === 'partial') return 'finishing_sync';
  if (view.error || view.phase === 'failed') return 'reconnecting';
  return 'healthy';
}

/** A public sync_busy rejection is ambiguous: only the typed status may prove
 * another instance owns the lease. Do not tell the user to quit for mailbox contention. */
export function bbsSyncActionIndicator(code: BbsSyncSafeActionErrorCode | null): BbsSyncIndicator {
  switch (code) {
    case 'worker_update_required': return 'update_worker';
    case 'cloudflare_quota_exceeded':
    case 'cloudflare_resource_limit': return 'cloudflare_limit';
    case 'relay_session_lost': return 'fetching_session';
    default: return 'reconnecting';
  }
}
