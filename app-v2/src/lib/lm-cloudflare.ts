import type { LmCloudflareError } from '../pty-client';

export function cloudflareTitle(error: LmCloudflareError): string {
  if (error.limit) return 'Cloudflare limit reached';
  return error.code == null ? 'Cloudflare error' : `Cloudflare error ${error.code}`;
}

export function cloudflareReset(error: LmCloudflareError): { local: string; utc: string } | null {
  if (!error.limit || !error.resetAt) return null;
  const at = new Date(error.resetAt);
  if (!Number.isFinite(at.getTime()) || at.getTime() <= Date.now()) return null;
  const options: Intl.DateTimeFormatOptions = {hour:'2-digit',minute:'2-digit',hourCycle:'h23'};
  return {local:at.toLocaleTimeString(undefined,options),utc:at.toLocaleTimeString('en-GB',{...options,timeZone:'UTC'})};
}

export function cloudflareSummary(error: LmCloudflareError): string {
  const title = error.code === 1102 ? 'Cloudflare 1102 · CPU time limit' : cloudflareTitle(error);
  const reset = cloudflareReset(error);
  return reset ? `${title} · resets ${reset.local}` : title;
}
