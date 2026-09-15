import { BBS_SYNC_DISCUSSIONS_URL, BBS_SYNC_ERROR_DETAILS, BBS_SYNC_PROTOCOL_ERROR_DETAIL } from '../bbs-sync-errors';
import { openExternalUrl } from '../pty-client';

/** Only the explicit fixed messages get a fixed external link. Everything
 * else is ordinary React text: no backend HTML, URL detection or string markup. */
export function BbsSyncError({ detail }: { detail: string }) {
  const linked = detail === BBS_SYNC_ERROR_DETAILS.unknown || detail === BBS_SYNC_ERROR_DETAILS.identity
    || detail === BBS_SYNC_PROTOCOL_ERROR_DETAIL;
  return <p className="bbs-sync-round-error" role="status">
    <strong>Sync error:</strong>{' '}
    {linked ? <>{detail.slice(0, -'GitHub Discussions.'.length)}
      <a href={BBS_SYNC_DISCUSSIONS_URL} target="_blank" rel="noopener noreferrer"
        onClick={(event) => {
          event.preventDefault();
          void openExternalUrl(BBS_SYNC_DISCUSSIONS_URL).catch(() => {});
        }}>GitHub Discussions<span aria-hidden="true">↗</span></a>.</> : detail}
  </p>;
}
