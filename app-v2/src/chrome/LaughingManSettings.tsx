/* Laughing Man (Telegram bridge) settings form.
   Shared by the room right-column setup modal and the Tavern system hero
   profile. Three states (2026-06-11 UX decision):
     A  no token        → token input
     B  token verified, owner unclaimed → display item (Awaiting owner) +
        step-by-step claim guide; NOT usable yet and never looks usable
     C  owner claimed   → Live item
   No enabled/paused switch: token present == bridge runs, Remove == stop.
   Remove keeps the owner uid (it identifies who may use the bridge,
   independent of which bot carries it). */
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  lmClaimOwner,
  lmRevoke,
  lmSaveToken,
  lmStart,
  lmStatus,
  lmStandbyConnect,
  lmStandbyDeployWorker,
  lmStandbyDisconnect,
  onLmStandbyDeployEvent,
  type LmStandbyDeployEvent,
  type LmStatus,
} from '../pty-client';

type StandbyDeployLogLine = Pick<LmStandbyDeployEvent, 'phase' | 'level' | 'line'>;

function waitForNextPaint(): Promise<void> {
  return new Promise((resolve) => {
    window.requestAnimationFrame(() => resolve());
  });
}

export function LaughingManSettings({ onChanged }: { onChanged?: () => void }) {
  const [status, setStatus] = useState<LmStatus | null>(null);
  const [tokenDraft, setTokenDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [removeArmed, setRemoveArmed] = useState(false);
  const [standbyExpanded, setStandbyExpanded] = useState<1 | 2 | null>(2);
  const [standbyUrl, setStandbyUrl] = useState('');
  const [standbyStatusText, setStandbyStatusText] = useState<string | null>(null);
  const [standbyDeployLog, setStandbyDeployLog] = useState<StandbyDeployLogLine[]>([]);
  const [standbyDeploying, setStandbyDeploying] = useState(false);
  const [standbyLogExpanded, setStandbyLogExpanded] = useState(false);
  const [standbyManualOpen, setStandbyManualOpen] = useState(false);
  const [standbyUpgradeOpen, setStandbyUpgradeOpen] = useState(false);
  const [standbyUpgradeAwaitingHeartbeat, setStandbyUpgradeAwaitingHeartbeat] = useState(false);
  const removeTimerRef = useRef<number | null>(null);

  const refresh = useCallback(async () => {
    try {
      setStatus(await lmStatus());
    } catch (err) {
      console.warn('[laughing-man] status failed', err);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => {
      void refresh();
    }, 5000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  useEffect(() => () => {
    if (removeTimerRef.current != null) window.clearTimeout(removeTimerRef.current);
  }, []);

  useEffect(() => {
    if (status?.standby && !status.standby.updateAvailable) {
      setStandbyUpgradeAwaitingHeartbeat(false);
      setStandbyUpgradeOpen(false);
    }
  }, [status?.standby?.updateAvailable]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void | Promise<void>) | null = null;
    void onLmStandbyDeployEvent((event) => {
      if (cancelled) return;
      setStandbyDeployLog((lines) => [
        ...lines.slice(-79),
        { phase: event.phase, level: event.level, line: event.line },
      ]);
      if (event.workerUrl) {
        setStandbyUrl(event.workerUrl);
      }
    }).then((off) => {
      if (cancelled) {
        void off();
      } else {
        unlisten = off;
      }
    });
    return () => {
      cancelled = true;
      if (unlisten) void unlisten();
    };
  }, []);

  const run = useCallback(async (action: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      await refresh();
      onChanged?.();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }, [onChanged, refresh]);

  const onRemove = useCallback(() => {
    if (!removeArmed) {
      setRemoveArmed(true);
      removeTimerRef.current = window.setTimeout(() => setRemoveArmed(false), 3000);
      return;
    }
    if (removeTimerRef.current != null) window.clearTimeout(removeTimerRef.current);
    setRemoveArmed(false);
    void run(() => lmRevoke());
  }, [removeArmed, run]);

  const onStandbyConnect = useCallback(async () => {
    const workerUrl = standbyUrl.trim();
    if (!workerUrl) return;
    setBusy(true);
    setError(null);
    setStandbyStatusText('Checking Worker and connecting Telegram webhook...');
    try {
      const next = await lmStandbyConnect(workerUrl);
      if (next) setStatus(next);
      setStandbyUrl('');
      setStandbyExpanded(null);
      setStandbyManualOpen(false);
      setStandbyStatusText('Connected.');
      await refresh();
      onChanged?.();
    } catch (err) {
      setStandbyStatusText(`Could not connect: ${String(err)}`);
    } finally {
      setBusy(false);
    }
  }, [onChanged, refresh, standbyUrl]);

  const onStandbyDisconnect = useCallback(() => {
    setStandbyStatusText(null);
    void run(() => lmStandbyDisconnect());
  }, [run]);

  const onStandbyDeploy = useCallback(async (connectAfterDeploy = true) => {
    if (standbyDeploying) return;
    setBusy(true);
    setStandbyDeploying(true);
    setError(null);
    setStandbyDeployLog([]);
    setStandbyLogExpanded(false);
    setStandbyManualOpen(false);
    setStandbyUpgradeAwaitingHeartbeat(false);
    if (!connectAfterDeploy) setStandbyUpgradeOpen(true);
    setStandbyStatusText(connectAfterDeploy ? 'Preparing Cloudflare Worker deployment...' : 'Preparing Worker upgrade...');
    try {
      await waitForNextPaint();
      const result = await lmStandbyDeployWorker();
      if (!result.workerUrl) {
        if (!connectAfterDeploy) {
          setStandbyStatusText(`Upgrade command finished in ${result.workerDir}. Waiting for Standby heartbeat to report the new Worker version.`);
          setStandbyUpgradeAwaitingHeartbeat(true);
          await refresh();
          onChanged?.();
          return;
        }
        setStandbyManualOpen(true);
        setStandbyStatusText(`Deployment finished in ${result.workerDir}, but Kota could not detect the Worker URL. Paste it below.`);
        return;
      }
      if (!connectAfterDeploy) {
        setStandbyUrl('');
        setStandbyExpanded(null);
        setStandbyManualOpen(false);
        setStandbyStatusText('Worker upgraded. Waiting for Standby heartbeat to report the new version.');
        setStandbyUpgradeAwaitingHeartbeat(true);
        await refresh();
        onChanged?.();
        return;
      }
      setStandbyUrl(result.workerUrl);
      setStandbyStatusText('Worker deployed. Connecting Telegram webhook...');
      try {
        const next = await lmStandbyConnect(result.workerUrl);
        if (next) setStatus(next);
        setStandbyUrl('');
        setStandbyExpanded(null);
        setStandbyManualOpen(false);
        setStandbyStatusText('Connected.');
        await refresh();
        onChanged?.();
      } catch (err) {
        setStandbyManualOpen(true);
        setStandbyStatusText(`Worker deployed, but automatic connect failed: ${String(err)}`);
      }
    } catch (err) {
      if (connectAfterDeploy) {
        setStandbyManualOpen(true);
      }
      if (!connectAfterDeploy) {
        setStandbyUpgradeAwaitingHeartbeat(false);
      }
      setStandbyStatusText(`${connectAfterDeploy ? 'Deploy' : 'Upgrade'} failed: ${String(err)}`);
    } finally {
      setStandbyDeploying(false);
      setBusy(false);
    }
  }, [onChanged, refresh, standbyDeploying]);

  const hasBot = !!status?.botUsername;
  const claimed = !!status?.ownerUserId;
  const botHandle = (status?.botUsername ?? '').replace(/^@/, '');
  const pending = status?.pendingClaim ?? null;
  const standby = status?.standby ?? null;
  const standbyBusy = busy || standbyDeploying;
  const standbyLatestLog = standbyDeployLog.at(-1) ?? null;
  const standbyUpdateAvailable = !!standby?.updateAvailable;
  const standbyUpgradeBusy = standbyBusy || standbyUpgradeAwaitingHeartbeat;

  const renderStandbyDeployProgress = (showManualUrl: boolean) => (
    <>
      {standbyStatusText && <span className="lm-standby-note">{standbyStatusText}</span>}
      {standbyLatestLog && (
        <div className={`lm-standby-current-log ${standbyLatestLog.level}`} aria-live="polite">
          <span>{standbyLatestLog.phase}</span>
          <code>{standbyLatestLog.line}</code>
        </div>
      )}
      {standbyDeployLog.length > 1 && (
        <button
          type="button"
          className="lm-standby-log-toggle"
          onClick={() => setStandbyLogExpanded((expanded) => !expanded)}
        >
          {standbyLogExpanded ? 'Hide previous log' : `Show previous log (${standbyDeployLog.length - 1})`}
        </button>
      )}
      {standbyLogExpanded && standbyDeployLog.length > 1 && (
        <div className="lm-standby-log">
          {standbyDeployLog.slice(0, -1).map((entry, index) => (
            <div key={`${entry.phase}-${index}`} className={`lm-standby-log-line ${entry.level}`}>
              <span>{entry.phase}</span>
              <code>{entry.line}</code>
            </div>
          ))}
        </div>
      )}
      {showManualUrl && standbyManualOpen && (
        <div className="lm-standby-connect-row">
          <input
            value={standbyUrl}
            placeholder="https://kota-laughing-man-relay.<account>.workers.dev"
            onChange={(event) => setStandbyUrl(event.currentTarget.value)}
          />
          <button
            type="button"
            className="lm-standby-primary"
            disabled={standbyBusy || !standbyUrl.trim()}
            onClick={() => void onStandbyConnect()}
          >
            Connect
          </button>
        </div>
      )}
    </>
  );

  return (
    <div className="lm-setup">
      <div className="lm-privacy-banner">
        Only the Telegram account you claim below can talk to the bot.
      </div>

      {!hasBot ? (
        /* ── State A: no token ── */
        <div className="lm-field">
          <label>Bot token</label>
          <div className="lm-hint">Create a bot with @BotFather on Telegram, then paste its token here.</div>
          <div className="lm-token-row">
            <input
              type="password"
              value={tokenDraft}
              placeholder="123456789:AA…"
              onChange={(event) => {
                const value = event.currentTarget.value;
                setTokenDraft(value);
              }}
            />
            <button
              type="button"
              disabled={busy || !tokenDraft.trim()}
              onClick={() => void run(async () => {
                await lmSaveToken(tokenDraft.trim());
                setTokenDraft('');
                await lmStart();
              })}
            >
              {busy ? 'Checking…' : 'Save & Verify'}
            </button>
          </div>
        </div>
      ) : (
        /* ── State B/C: display item, no input ── */
        <div className="lm-field">
          <label>Bot</label>
          <div className="lm-bot-item">
            <span className="lm-avatar tavern-avatar-art system-laughing-man" aria-hidden>
              <span />
              <i />
              <b />
            </span>
            <span className="lm-bot-copy">
              <b>{status?.botUsername}</b>
              <small>token stored locally (0600)</small>
            </span>
            <span className={`lm-status-pill ${claimed ? 'live' : 'awaiting'}`}>
              <span className="d" aria-hidden />
              {claimed ? 'Live' : 'Awaiting owner'}
            </span>
            <button
              type="button"
              className={`lm-remove-btn ${removeArmed ? 'confirm' : ''}`}
              disabled={busy}
              onClick={onRemove}
            >
              {removeArmed ? 'Confirm?' : 'Remove'}
            </button>
          </div>
          <div className="lm-hint">One Laughing Man per Kota — remove this bot before adding a different one.</div>
        </div>
      )}

      {hasBot && !claimed && (
        /* ── State B: claim guide ── */
        <div className="lm-field">
          <label>Finish setup — claim ownership</label>
          <div className="lm-steps">
            <div className={`lm-step ${pending ? 'done' : ''}`}>
              <span className="n">{pending ? '✓' : '1'}</span>
              <span>Open <b>t.me/{botHandle}</b> in your Telegram and tap Start</span>
            </div>
            <div className={`lm-step ${pending ? 'done' : ''}`}>
              <span className="n">{pending ? '✓' : '2'}</span>
              <span>Send it any message ("hi" is fine)</span>
            </div>
            <div className="lm-step">
              <span className="n">3</span>
              {pending ? (
                <span className="lm-claim-inline">
                  <span className="lm-claim-uid">
                    uid {pending.userId}
                    {pending.username ? ` (@${pending.username})` : ''}
                  </span>
                  <button type="button" disabled={busy} onClick={() => void run(() => lmClaimOwner())}>
                    Claim as Owner
                  </button>
                </span>
              ) : (
                <span className="lm-step-waiting">The sender will appear here for one-click claim</span>
              )}
            </div>
          </div>
          <div className="lm-hint lm-warn">⚠ Not usable yet — messages are not delivered to any agent until ownership is claimed.</div>
        </div>
      )}

      {hasBot && claimed && (
        /* ── State C ── */
        <div className="lm-field">
          <label>Owner</label>
          <div className="lm-hint">
            ✓ Claimed · Telegram uid {status?.ownerUserId} — swapping bots keeps the owner.
          </div>
        </div>
      )}

      {hasBot && claimed && (
        <div className="lm-field">
          <label>24/7 Standby</label>
          {standby ? (
            <>
              <div className="lm-standby-status">
                <span className="lm-standby-copy">
                  <b>{standby.workerUrl}</b>
                  <small>
                    {standby.lastError
                      ? standby.lastError
                      : standby.lastSyncAt
                        ? `last sync ${standby.lastSyncAt}`
                        : standby.lastHeartbeatAt
                          ? `last heartbeat ${standby.lastHeartbeatAt}`
                          : 'waiting for first heartbeat'}
                  </small>
                </span>
                <span className={`lm-status-pill ${standby.live ? 'live' : 'awaiting'}`}>
                  <span className="d" aria-hidden />
                  {standby.live ? 'Live' : 'Attention'}
                </span>
                {standbyUpdateAvailable && (
                  <button
                    type="button"
                    className="lm-standby-upgrade"
                    disabled={standbyUpgradeBusy}
                    onClick={() => setStandbyUpgradeOpen((open) => !open)}
                  >
                    {standbyUpgradeOpen ? 'Hide Upgrade' : 'Upgrade'}
                  </button>
                )}
                <button
                  type="button"
                  className="lm-standby-disconnect"
                  disabled={standbyBusy}
                  onClick={onStandbyDisconnect}
                >
                  Disconnect
                </button>
              </div>
              {standbyUpgradeOpen && (
                <div className="lm-standby-step-card open lm-standby-upgrade-card">
                  <button type="button" className="lm-standby-step-head" onClick={() => setStandbyUpgradeOpen((open) => !open)}>
                    <span className="lm-standby-step-num">↻</span>
                    <span>Upgrade Worker</span>
                  </button>
                  <div className="lm-standby-step-body">
                    <div className="lm-standby-actions">
                      <button
                        type="button"
                        className="lm-standby-primary"
                        disabled={standbyUpgradeBusy}
                        onClick={() => void onStandbyDeploy(false)}
                      >
                        {standbyDeploying
                          ? 'Upgrading…'
                          : standbyUpgradeAwaitingHeartbeat
                            ? 'Waiting for heartbeat…'
                            : 'Upgrade with Wrangler'}
                      </button>
                    </div>
                    <span className="lm-standby-note">
                      Kota runs Wrangler locally and redeploys the current Standby Worker without changing its pairing.
                    </span>
                    {standbyUpdateAvailable && (
                      <span className="lm-standby-note">
                        Worker update available: live {standby.relayVersion ?? 'unknown'} · bundled {standby.recommendedVersion}
                      </span>
                    )}
                    {renderStandbyDeployProgress(false)}
                  </div>
                </div>
              )}
              {!standbyUpgradeOpen && standbyUpdateAvailable && (
                <div className="lm-standby-live-detail">
                  <span className="lm-standby-note">
                    Worker update available: live {standby.relayVersion ?? 'unknown'} · bundled {standby.recommendedVersion}
                  </span>
                </div>
              )}
            </>
          ) : (
            <div className="lm-standby-setup">
              <div className="lm-standby-title">24/7 Online Standby Setup</div>
              <div className={`lm-standby-step-card ${standbyExpanded === 1 ? 'open' : ''}`}>
                <button type="button" className="lm-standby-step-head" onClick={() => setStandbyExpanded(standbyExpanded === 1 ? null : 1)}>
                  <span className="lm-standby-step-check">✓</span>
                  <span>Telegram bot is ready</span>
                </button>
                {standbyExpanded === 1 && (
                  <div className="lm-standby-step-body">
                    <span className="lm-hint">Your claimed bot is ready for Standby.</span>
                  </div>
                )}
              </div>
              <div className={`lm-standby-step-card ${standbyExpanded === 2 ? 'open' : ''}`}>
                <button type="button" className="lm-standby-step-head" onClick={() => setStandbyExpanded(standbyExpanded === 2 ? null : 2)}>
                  <span className="lm-standby-step-num">2</span>
                  <span>Deploy 7x24 monitor (Cloudflare Account Required)</span>
                </button>
                {standbyExpanded === 2 && (
                  <div className="lm-standby-step-body">
                    <div className="lm-standby-actions">
                      <button
                        type="button"
                        className="lm-standby-primary"
                        disabled={standbyBusy}
                        onClick={() => void onStandbyDeploy()}
                      >
                        {standbyDeploying ? 'Deploying…' : 'Deploy with Wrangler'}
                      </button>
                      <button
                        type="button"
                        className="lm-standby-secondary"
                        disabled={standbyBusy}
                        onClick={() => setStandbyManualOpen((open) => !open)}
                      >
                        {standbyManualOpen ? 'Hide Manual URL' : 'Enter Worker URL'}
                      </button>
                    </div>
                    <span className="lm-standby-note">Kota runs Wrangler locally. Cloudflare may open your browser for sign-in, then Kota deploys and connects automatically.</span>
                    {renderStandbyDeployProgress(true)}
                  </div>
                )}
              </div>
            </div>
          )}
        </div>
      )}

      {error && <div className="bbs-error">{error}</div>}
    </div>
  );
}
