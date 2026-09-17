/** Fixed display copy shared by safe action errors and the status renderer.
 * Backend status contains plain details, never HTML, links or this UI prefix. */
export const BBS_SYNC_ERROR_DETAILS = {
  unknown: 'Sync could not finish; retry or report it on GitHub Discussions.',
  identity: 'Device identity is incomplete; report it on GitHub Discussions.',
  sync_busy: 'Sync is busy; try again shortly.',
  stale_signature: 'Request expired; check this device’s clock and the other devices’ clocks, then retry.',
  worker_update_required: 'Worker is outdated; update it from the Laughing Man card and retry.',
  cloudflare_quota_exceeded: 'Cloudflare daily quota exceeded. Sync will retry after 00:00 UTC.',
  cloudflare_resource_limit: 'Cloudflare resource limit reached. Retry later; if it continues, ask the group owner to check Cloudflare.',
  relay_session_lost: 'The sync relay session was interrupted. Keep Kota running on the other devices and retry.',
} as const;

export type BbsSyncSafeActionErrorCode = 'stale_signature' | 'worker_update_required' | 'sync_busy'
  | 'cloudflare_quota_exceeded' | 'cloudflare_resource_limit' | 'relay_session_lost';

/** Only agreed backend rejection codes may replace a generic action error.
 * HTTP status, provider HTML and resource-limit classification stay backend-side. */
export function isBbsSyncSafeActionErrorCode(value: unknown): value is BbsSyncSafeActionErrorCode {
  return value === 'stale_signature' || value === 'worker_update_required' || value === 'sync_busy'
    || value === 'cloudflare_quota_exceeded' || value === 'cloudflare_resource_limit' || value === 'relay_session_lost';
}

export const BBS_SYNC_DISCUSSIONS_URL = 'https://github.com/stabruriss/kota-app/discussions';

/** A malformed exchange is not evidence that the installed versions differ. */
export const BBS_SYNC_PROTOCOL_ERROR_DETAIL = 'Sync protocol error; retry or report it on GitHub Discussions.';
