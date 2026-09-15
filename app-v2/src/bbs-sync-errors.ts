/** Fixed display copy shared by safe action errors and the status renderer.
 * Backend status contains plain details, never HTML, links or this UI prefix. */
export const BBS_SYNC_ERROR_DETAILS = {
  unknown: 'Sync could not finish; retry or report it on GitHub Discussions.',
  identity: 'Device identity is incomplete; report it on GitHub Discussions.',
  sync_busy: 'Another Kota app is using sync; quit it and click Retry.',
  stale_signature: 'Request expired; check this device’s clock and the other devices’ clocks, then retry.',
  worker_update_required: 'Worker is outdated; update it from the Laughing Man card and retry.',
} as const;

export const BBS_SYNC_DISCUSSIONS_URL = 'https://github.com/stabruriss/kota-app/discussions';

/** A malformed exchange is not evidence that the installed versions differ. */
export const BBS_SYNC_PROTOCOL_ERROR_DETAIL = 'Sync protocol error; retry or report it on GitHub Discussions.';
