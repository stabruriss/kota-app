import { useId } from 'react';
import { BBS_SYNC_INDICATOR_COPY } from '../bbs-sync-indicator';
import { BBS_SYNC_DISCUSSIONS_URL } from '../bbs-sync-errors';
import type { BbsSyncIndicator as Indicator } from '../types/bbs-sync';
import { openExternalUrl } from '../pty-client';

/** Reuses the Smart Shell status lamp and typography. No timer, IO, permission
 * inference or error-string classification lives in this presentation leaf. */
export function BbsSyncIndicator({ indicator, online = 0, detail, expiredRequest = false }: {
  indicator: Indicator;
  online?: number;
  /** A safe local IPC/read detail, never untrusted HTML or a URL. */
  detail?: string | null;
  expiredRequest?: boolean;
}) {
  const tooltip = useId();
  const copy = BBS_SYNC_INDICATOR_COPY[indicator];
  const text = expiredRequest ? 'Request expired' : indicator === 'healthy' ? `${online} online` : copy.text;
  return <span className="bbs-sync-indicator st-bar-status" data-tone={copy.tone}
    data-indicator={indicator} role="status" aria-live="polite" tabIndex={0} aria-describedby={tooltip}>
    <span className="st-dot live" aria-hidden="true" />
    <span className="bbs-sync-short-status">{text}</span>
    <span className="bbs-sync-tooltip" id={tooltip} role="tooltip">
      {detail || copy.detail}
      {indicator === 'device_identity_error' && <>{' '}
        <a href={BBS_SYNC_DISCUSSIONS_URL} target="_blank" rel="noopener noreferrer"
          onClick={(event) => { event.preventDefault(); void openExternalUrl(BBS_SYNC_DISCUSSIONS_URL).catch(() => {}); }}>
          GitHub Discussions<span aria-hidden="true">↗</span>
        </a>
      </>}
    </span>
  </span>;
}
