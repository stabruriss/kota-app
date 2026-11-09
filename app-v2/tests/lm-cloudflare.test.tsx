import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import * as ipc from '../src/pty-client';
import { RightColumn } from '../src/chrome/RightColumn';
import { LaughingManSettings } from '../src/chrome/LaughingManSettings';
import { LaughingManCloudflareWarning } from '../src/chrome/LaughingManCloudflareWarning';
import { cloudflareReset, cloudflareSummary } from '../src/lib/lm-cloudflare';

const quota: ipc.LmCloudflareError = {
  code: null,
  message: 'Exceeded allowed volume of requests in Durable Objects free tier.',
  limit: 'volume of requests',
  resetAt: '2030-10-25T00:00:00Z',
  since: '2030-10-24T20:00:00Z',
};
const entry: ipc.LmLogEntry = {
  schema: 'kota.laughing-man.log.v1', id: 'fixture-entry', ts: quota.since,
  direction: 'in', projectId: 'fixture-project', projectName: 'Atlas',
  agentId: 'fixture-agent', agentName: 'Fixture', preview: 'A synthetic earlier message.',
};
function status(error: ipc.LmCloudflareError | null = quota): ipc.LmStatus {
  return {
    configured: true, enabled: true, running: true, botUsername: 'fixture_bot', ownerUserId: 42,
    selected: {projectId:'fixture-project', projectRoot:'/tmp/lm-fixture', projectName:'Atlas',
      agentId:'fixture-agent', agentName:'Fixture', muted:false},
    latest: entry,
    standby: {workerUrl:'https://worker.example', live:!error, lastError:error?.message,
      cloudflare:error, lastSyncAt:quota.since, recommendedVersion:'0.1.5', relayVersion:'0.1.5',
      updateAvailable:false, queueCount:0},
  };
}
function room() {
  return render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} />);
}

beforeEach(() => {
  vi.spyOn(Date, 'now').mockReturnValue(Date.parse('2030-10-24T20:00:00Z'));
  vi.spyOn(ipc, 'lmStatus').mockResolvedValue(status());
  vi.spyOn(ipc, 'lmMessageLog').mockResolvedValue([entry]);
  vi.spyOn(ipc, 'lmStandbyQueue').mockResolvedValue([]);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('Laughing Man Cloudflare status', () => {
  it('shows quota state on the real card and history without hiding earlier messages or controls', async () => {
    room();
    const card = screen.getByRole('button', {name:'Laughing Man Telegram bridge'});
    await waitFor(() => expect(card).toHaveTextContent('fixture_bot · standby unavailable'));
    expect(card).toHaveClass('muted');
    expect(card).toHaveTextContent(cloudflareSummary(quota));
    expect(card).toHaveTextContent(entry.preview);
    expect(within(card).getByRole('button', {name:'Pause Telegram messaging'})).toBeEnabled();
    expect(within(card).getByRole('button', {name:'Laughing Man settings'})).toBeEnabled();
    fireEvent.click(card);
    const dialog = screen.getByRole('dialog', {name:'Laughing Man history'});
    expect(within(dialog).getByRole('status')).toHaveTextContent(quota.message);
    expect(within(dialog).getByRole('status')).toHaveTextContent('(00:00 UTC)');
    expect(await within(dialog).findByText(entry.preview)).toBeVisible();
    expect(dialog).not.toHaveTextContent('Kota retries automatically');
    expect(within(dialog).queryByRole('button', {name:/retry/i})).not.toBeInTheDocument();
  });

  it('keeps the Standby Attention state and displays the same quota summary in settings', async () => {
    render(<LaughingManSettings />);
    expect(await screen.findByText('Attention')).toBeVisible();
    expect(screen.getByText(cloudflareSummary(quota))).toHaveAttribute('title', quota.message);
    expect(screen.getByRole('button', {name:'Disconnect'})).toBeEnabled();
    expect(screen.getByText('Live')).toBeVisible(); // Bot state is separate from Standby.
    expect(screen.queryByText(/Kota retries automatically/i)).not.toBeInTheDocument();
  });

  it('shows a known CPU error without claiming a daily reset', async () => {
    const error = {...quota, code:1102, message:'Worker exceeded CPU time limit.', limit:null, resetAt:null};
    vi.mocked(ipc.lmStatus).mockResolvedValue(status(error));
    room();
    expect(await screen.findByText('Cloudflare 1102 · CPU time limit')).toBeVisible();
    fireEvent.click(screen.getByRole('button', {name:'Laughing Man Telegram bridge'}));
    const warning = within(screen.getByRole('dialog', {name:'Laughing Man history'})).getByRole('status');
    expect(warning).toHaveTextContent('Cloudflare error 1102');
    expect(warning).toHaveTextContent(error.message);
    expect(warning).not.toHaveTextContent(/resets/i);
  });

  it('renders an unknown code alone and a non-quota DO exception as readable text', () => {
    const {rerender} = render(<LaughingManCloudflareWarning error={{...quota, code:1999, message:'', limit:null, resetAt:null}} />);
    expect(screen.getByRole('status').textContent).toBe('Cloudflare error 1999');
    rerender(<LaughingManCloudflareWarning error={{...quota, message:'Object temporarily unavailable.', limit:null, resetAt:null}} />);
    expect(screen.getByRole('status')).toHaveTextContent('Cloudflare errorObject temporarily unavailable.');
    expect(screen.getByRole('status')).not.toHaveTextContent(/limit reached|resets/i);
  });

  it('uses the supplied instant for local and UTC time and removes an expired reset', () => {
    const reset = cloudflareReset(quota)!;
    expect(reset.utc).toBe('00:00');
    expect(reset.local).toBe(new Date(quota.resetAt!).toLocaleTimeString(undefined,
      {hour:'2-digit',minute:'2-digit',hourCycle:'h23'}));
    const {rerender} = render(<LaughingManCloudflareWarning error={quota} />);
    expect(screen.getByRole('status')).toHaveTextContent(`Resets at ${reset.local} (00:00 UTC)`);
    vi.mocked(Date.now).mockReturnValue(Date.parse('2030-10-25T00:01:00Z'));
    rerender(<LaughingManCloudflareWarning error={quota} />);
    expect(screen.getByRole('status')).not.toHaveTextContent(/resets/i);
    expect(cloudflareSummary(quota)).toBe('Cloudflare limit reached');
    expect(quota.resetAt).toBe('2030-10-25T00:00:00Z');
  });

  it('omits absent, invalid and non-quota reset fields', () => {
    expect(cloudflareReset({...quota, resetAt:null})).toBeNull();
    expect(cloudflareReset({...quota, resetAt:'invalid'})).toBeNull();
    expect(cloudflareReset({...quota, limit:null})).toBeNull();
  });

  it('treats provider error text as text, never as markup', () => {
    const text = '<img src=x onerror="alert(1)">';
    const {container} = render(<LaughingManCloudflareWarning error={{...quota, message:text}} />);
    expect(screen.getByText(text)).toBeVisible();
    expect(container.querySelector('img')).toBeNull();
  });

  it('clears the card and open-history warning when existing polling receives recovery', async () => {
    vi.useFakeTimers({toFake:['setInterval','clearInterval']});
    room();
    await act(async () => {});
    fireEvent.click(screen.getByRole('button', {name:'Laughing Man Telegram bridge'}));
    await act(async () => {});
    expect(screen.getByRole('dialog', {name:'Laughing Man history'})).toHaveTextContent('Cloudflare limit reached');
    vi.mocked(ipc.lmStatus).mockResolvedValue(status(null));
    await act(async () => { await vi.advanceTimersByTimeAsync(12_000); });
    const card = screen.getByRole('button', {name:'Laughing Man Telegram bridge'});
    expect(card).toHaveTextContent('fixture_bot · connected');
    expect(card).not.toHaveClass('muted');
    expect(screen.queryByText(/Cloudflare/)).not.toBeInTheDocument();
    expect(screen.getByRole('dialog', {name:'Laughing Man history'})).toHaveTextContent(entry.preview);
  });

  it('preserves ordinary Standby errors and user-controlled mute when no Cloudflare error exists', async () => {
    const normal = status(null);
    normal.standby!.live = false;
    normal.standby!.lastError = 'standby: connection timed out';
    normal.selected!.muted = true;
    vi.mocked(ipc.lmStatus).mockResolvedValue(normal);
    room();
    expect(await screen.findByText('Telegram Messaging Paused')).toBeVisible();
    expect(screen.getByText('fixture_bot · muted')).toBeVisible();
    fireEvent.click(screen.getByRole('button', {name:'Laughing Man settings'}));
    expect(await screen.findByText('standby: connection timed out')).toBeVisible();
    expect(screen.queryByText(/Cloudflare/)).not.toBeInTheDocument();
  });
});
