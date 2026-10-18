import { useEffect, useRef, useState } from 'react';
import { cancelJevKey, saveJevKey, selectJevProvider, useJevSettings, type JevProviderId, type JevSettings } from '../lib/jev';
import { LocalModelDialog } from './LocalModelDialog';
import { StringIcon } from './LocalModelPicker';
import '../styles/jev-settings.css';

const PROVIDERS: Array<[JevProviderId, string]> = [
  ['openrouter', 'OpenRouter'], ['vercel-ai-gateway', 'Vercel AI Gateway'], ['opencode', 'OpenCode Zen'], ['typesafe', 'TypeSafe'],
];
function JevKeyDialog({ providerId, onSaved, onClose }: { providerId: JevProviderId; onSaved: (settings: JevSettings) => void; onClose: () => void }) {
  const [key, setKey] = useState('');
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState(false);
  const active = useRef<string | null>(null);
  useEffect(() => () => { const requestId = active.current; active.current = null; if (requestId) void cancelJevKey(requestId).catch(() => {}); }, []);
  const close = () => {
    const requestId = active.current; active.current = null;
    if (requestId) void cancelJevKey(requestId).catch(() => {});
    setKey(''); onClose();
  };
  const save = async () => {
    if (active.current) return;
    if (!key.trim()) { setFailed(true); return; }
    const requestId = crypto.randomUUID();
    active.current = requestId; setSaving(true); setFailed(false);
    try {
      const settings = await saveJevKey({ requestId, providerId, key });
      if (active.current !== requestId) return;
      active.current = null; setKey(''); onSaved(settings); onClose();
    } catch {
      if (active.current !== requestId) return;
      active.current = null; setFailed(true); setSaving(false);
    }
  };
  return <LocalModelDialog title="Jev key" subtitle={PROVIDERS.find(([id]) => id === providerId)?.[1]} onClose={close} closeLabel="Close Jev key"
    footer={<><button type="button" className="jev-key-cancel" onClick={close}>Cancel</button><button type="button" className="jev-key-save" disabled={saving} onClick={() => void save()}>{saving ? 'Verifying…' : 'Save'}</button></>}>
    <div className="jev-key-body"><label>Key<input data-initial-focus autoFocus type="password" autoComplete="off" placeholder="Enter a key" value={key} disabled={saving} onChange={event => setKey(event.target.value)} /></label>
      <p>The key is verified before saving.</p>{failed && <p className="local-model-warning" role="status">Verification failed.</p>}
    </div>
  </LocalModelDialog>;
}

/** The shared key belongs to Puppeteer. It never substitutes for a model provider sign-in. */
export function JevProviderSettings({ focusRequest = 0 }: { focusRequest?: number }) {
  const jev = useJevSettings(true);
  const [keyFor, setKeyFor] = useState<JevProviderId | null>(null);
  const [selecting, setSelecting] = useState(false);
  const [selectFailed, setSelectFailed] = useState(false);
  const alive = useRef(true);
  const panel = useRef<HTMLElement>(null);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => {
    if (!focusRequest || !panel.current) return;
    const target = panel.current;
    target.scrollIntoView?.({ block: 'center', behavior: 'smooth' }); target.focus({ preventScroll: true });
    target.classList.add('is-highlighted');
    const timer = setTimeout(() => target.classList.remove('is-highlighted'), 2400);
    return () => { clearTimeout(timer); target.classList.remove('is-highlighted'); };
  }, [focusRequest]);
  const change = async (providerId: JevProviderId) => {
    if (selecting) return;
    setSelecting(true); setSelectFailed(false);
    try { const next = await selectJevProvider(providerId); if (alive.current) jev.accept(next); }
    catch { if (alive.current) setSelectFailed(true); }
    finally { if (alive.current) setSelecting(false); }
  };
  const settings = jev.settings;
  const status = settings?.keys.find(key => key.providerId === settings.provider)?.status;
  const unknown = jev.failed || selectFailed;
  return <section ref={panel} className="puppeteer-jev-settings" id="puppeteer-jev" tabIndex={-1} aria-label="Jev provider">
    <header><StringIcon /><div><h3>Jev provider</h3><p>For task and model auto routing</p></div></header>
    <div className="jev-settings-fields"><label>Jev provider<select aria-label="Jev provider" value={settings?.provider ?? 'openrouter'} disabled={!settings || selecting || jev.loading || unknown}
      onChange={event => void change(event.target.value as JevProviderId)}>{PROVIDERS.map(([id, name]) => <option key={id} value={id}>{name}</option>)}</select></label>
      <div className="jev-key-field"><span>Jev key</span><div className="jev-key-state" role="status"><i className={status ?? ''} />
        <span>{unknown ? 'Status unavailable' : !settings ? 'Checking…' : status === 'verified' ? 'Verified' : status === 'rejected' ? 'Rejected' : 'Not set'}</span>
        {unknown ? <button type="button" onClick={() => { setSelectFailed(false); void jev.refresh(); }}>Retry</button>
          : <button type="button" disabled={!settings || selecting || jev.loading} onClick={() => settings && setKeyFor(settings.provider)}>{status === 'none' || !status ? 'Add key' : 'Update key'}</button>}
      </div></div>
    </div>{!unknown && status === 'rejected' && <p className="jev-rejected-note">This Jev key was rejected. Update it to resume routing.</p>}
    {keyFor && <JevKeyDialog providerId={keyFor} onSaved={jev.accept} onClose={() => setKeyFor(null)} />}
  </section>;
}
