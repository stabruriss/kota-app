import type { LmCloudflareError } from '../pty-client';
import { cloudflareReset, cloudflareTitle } from '../lib/lm-cloudflare';

export function LaughingManCloudflareWarning({ error }: { error: LmCloudflareError }) {
  const reset = cloudflareReset(error);
  return <div className="lm-cf-warning" role="status">
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <path d="M12 3 2 21h20L12 3Z" /><path d="M12 9v5m0 3v1" />
    </svg>
    <div className="lm-cf-warning-copy">
      <strong>{cloudflareTitle(error)}</strong>
      {error.message && <p>{error.message}</p>}
      {reset && <div className="lm-cf-warning-foot">Resets at {reset.local} ({reset.utc} UTC)</div>}
    </div>
  </div>;
}
