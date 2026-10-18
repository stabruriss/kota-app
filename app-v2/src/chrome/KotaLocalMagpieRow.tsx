import { useEffect, useRef, useState } from 'react';
import magpieIcon from '../assets/tavern/icons/providers/magpie.svg';
import { LocalAuthLayer } from './KotaLocalSignInDialog';
import { magpieErrorText, onMagpieChanged, probeMagpie, readMagpie, saveMagpie,
  type MagpieSettings, type MagpieState } from '../lib/local-magpie';
import '../styles/local-magpie.css';

function MagpieLogo({ dialog = false }: { dialog?: boolean }) {
  return <span className={`${dialog ? 'local-auth-dialog-logo' : 'tavern-connection-logo local-auth-logo'} local-magpie-logo`} aria-hidden="true"><img src={magpieIcon} alt="" /></span>;
}
export function MagpieSettingsDialog({ initial, onClose }: { initial: MagpieState; onClose: () => void }) {
  const [settings, setSettings] = useState<MagpieSettings>(initial.settings);
  const [port, setPort] = useState(String(initial.settings.port));
  const [key, setKey] = useState(initial.settings.mode === 'local' ? initial.defaultKey : '');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const local = settings.mode === 'local';
  const validPort = Number.isInteger(Number(port)) && Number(port) >= 1 && Number(port) <= 65535;
  const reuse = !local && initial.keySaved && initial.settings.mode === 'lan' && settings.ip === initial.settings.ip && Number(port) === initial.settings.port;
  function mode(value: MagpieSettings['mode']) {
    setSettings(old => ({ ...old, mode: value }));
    setKey(value === 'local' ? initial.defaultKey : '');
    setError('');
  }
  const close = () => { if (!saving) { setKey(''); onClose(); } };
  return <LocalAuthLayer onEscape={close}>
    <form className="kota-confirm-card local-auth-dialog local-magpie-settings" role="dialog" aria-modal="true" aria-label="Magpie settings"
      onSubmit={async event => {
        event.preventDefault(); if (saving || !event.currentTarget.reportValidity()) return;
        const secret = key; setKey(''); setError(''); setSaving(true);
        try {
          const result = await saveMagpie({ ...settings, port: Number(port) }, secret || undefined);
          if (!mounted.current) return;
          const savedTarget = result.settings.mode === settings.mode && result.settings.ip === settings.ip && result.settings.port === Number(port);
          if (!savedTarget || result.error === 'save_failed' || result.error === 'key_rejected') setError(magpieErrorText(result.error)); else onClose();
        } catch (e) { if (mounted.current) setError(magpieErrorText(e)); }
        finally { if (mounted.current) setSaving(false); }
      }}>
      <div className="local-auth-dialog-head"><MagpieLogo dialog /><h2>Magpie settings</h2></div>
      <div className="local-auth-body">
        <fieldset className="local-auth-options" disabled={saving}><legend>Mode</legend>
          <div className="local-magpie-modes" role="radiogroup" aria-label="Mode">
            {(['local', 'lan'] as const).map(value => <button type="button" className="local-auth-option" key={value} role="radio"
              aria-checked={settings.mode === value} autoFocus={initial.settings.mode === value} disabled={saving}
              onClick={() => mode(value)} onKeyDown={event => {
                if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
                  event.preventDefault(); mode(local ? 'lan' : 'local');
                  (event.currentTarget.nextElementSibling as HTMLElement ?? event.currentTarget.previousElementSibling as HTMLElement)?.focus();
                }
              }}><b>{value === 'local' ? 'Local' : 'LAN'}</b></button>)}
          </div>
        </fieldset>
        <div className="local-magpie-host">
          <label className="local-auth-field"><span>IP</span><input aria-label="IP" autoComplete="off" spellCheck={false} readOnly={local} disabled={saving}
            required value={local ? '127.0.0.1' : settings.ip} placeholder={local ? undefined : 'Magpie LAN Gateway IP, e.g. 192.168.1.101'} onChange={event => setSettings(old => ({ ...old, ip: event.target.value }))} /></label>
          <label className="local-auth-field"><span>Port</span><input aria-label="Port" type="text" inputMode="numeric" pattern="[0-9]+" required disabled={saving}
            ref={input => { input?.setCustomValidity(port && !validPort ? 'Enter a port from 1 to 65535.' : ''); }}
            value={port} onChange={event => setPort(event.target.value)} /></label>
        </div>
        <label className="local-auth-field"><span>Key</span><input aria-label="Key" type={local ? 'text' : 'password'} autoComplete="new-password" spellCheck={false}
          disabled={saving} required={!local && !reuse} value={key} onChange={event => setKey(event.target.value)}
          placeholder={local ? initial.defaultKey : reuse ? 'Saved key' : 'sk-magpie-key-…'} /></label>
        {error && <p className="local-auth-field-error" role="alert">{error}</p>}
      </div>
      <div className="kota-confirm-actions"><button type="button" disabled={saving} onClick={close}>Cancel</button><button type="submit" className="confirm" disabled={saving}>Save</button></div>
    </form>
  </LocalAuthLayer>;
}

/** A regular provider row. Mounting it means that the user opened Link. */
export function KotaLocalMagpieRow({ onLinked }: { onLinked: (linked: boolean) => void }) {
  const [state, setState] = useState<MagpieState | null>(null);
  const [dialog, setDialog] = useState<MagpieState | null>(null);
  const [error, setError] = useState('');
  const alive = useRef(true);
  const changed = useRef(onLinked); changed.current = onLinked;
  useEffect(() => {
    alive.current = true;
    const update = (next: MagpieState) => { if (alive.current) { setState(next); changed.current(next.linked); } };
    const stop = onMagpieChanged(update);
    void probeMagpie().catch(() => { if (alive.current) { setState(null); changed.current(false); } });
    return () => { alive.current = false; stop(); };
  }, []);
  return <>
    <article className="local-auth-row" id="kota-local-provider-magpie" data-provider="magpie" data-state={state?.linked ? 'api_key' : 'off'} aria-label="Magpie">
      <MagpieLogo /><div className="local-auth-main"><div className="local-auth-name"><b>Magpie</b></div>
        <div className="local-auth-status"><span className={`tavern-dot ${state?.linked ? 'ok' : 'off'}`} aria-hidden="true" /><span className="local-auth-status-text">{state?.linked ? 'Linked' : 'Not connected'}</span></div>
      </div>
      <div className="local-auth-actions"><button type="button" className="local-auth-action quiet" onClick={async () => {
        try { const fresh = await readMagpie(); if (alive.current) { setError(''); setDialog(fresh); } }
        catch (e) { if (alive.current) setError(magpieErrorText(e)); }
      }}>Settings</button></div>
    </article>
    {error && <p className="local-auth-field-error" role="alert">{error}</p>}
    {dialog && <MagpieSettingsDialog initial={dialog} onClose={() => setDialog(null)} />}
  </>;
}
