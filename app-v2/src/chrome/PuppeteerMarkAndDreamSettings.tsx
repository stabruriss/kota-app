import { soulPushErrorText, useSoulPushSettings } from '../lib/soul-push';
import { PuppeteerMarkAndDreamControls } from './PuppeteerMarkAndDreamControls';

export function PuppeteerMarkAndDreamSettings({ onJevSettings }: { onJevSettings: () => void }) {
  const controller = useSoulPushSettings();
  const { settings, error, busy, loading, jevReady } = controller;
  const conflict = error?.code === 'soul_settings_conflict';
  const jevProblem = error?.code.startsWith('jev_');
  const status = busy ? 'Saving…' : loading ? 'Loading…' : error ? <>
    <span>{soulPushErrorText(error)}</span>
    <span className="puppeteer-mark-actions">
      {jevProblem && <button type="button" onClick={onJevSettings}>Jev settings</button>}
      {!conflict && <button type="button" disabled={jevProblem && jevReady !== true} onClick={controller.retry}>Retry</button>}
      {settings && <button type="button" onClick={controller.reload}>Reload</button>}
    </span>
  </> : null;
  if (!settings) return <p className="puppeteer-mark-status" role="status">{status}</p>;
  return <PuppeteerMarkAndDreamControls key={controller.resetKey} value={settings} jevReady={jevReady}
    disabled={busy || loading || error?.operation === 'read'} onChange={controller.commit}
    onEditStart={controller.beginEdit} onEditCancel={controller.cancelEdit} onJevSettings={onJevSettings} status={status} />;
}
