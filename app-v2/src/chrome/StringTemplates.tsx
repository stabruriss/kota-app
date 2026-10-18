import { useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { catalogModel, type LocalModelCatalog } from '../lib/local-auth';
import { nextStringTemplateName, readStringTemplates, saveStringTemplates, type AutoRouteGroup, type StringTemplate, type StringTemplateLibrary } from '../lib/string-routing';
import { LocalModelDialog } from './LocalModelDialog';
import { StringIcon, LocalProviderIcon, localModelName } from './LocalModelPicker';
import { LocalModelCredit } from './LocalModelCredit';
import '../styles/string-templates.css';

function TemplateCard({ title, children, actions, danger, onClose }: {
  title: string; children: ReactNode; actions: ReactNode; danger?: boolean; onClose: () => void;
}) {
  const panel = useRef<HTMLElement>(null), close = useRef(onClose); close.current = onClose;
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    (panel.current?.querySelector<HTMLElement>('[data-initial-focus]') ?? panel.current)?.focus();
    const key = (event: KeyboardEvent) => {
      if (event.isComposing) return;
      if (event.key === 'Escape') { event.preventDefault(); event.stopImmediatePropagation(); close.current(); }
      if (event.key !== 'Tab') return;
      const controls = [...(panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)') ?? [])];
      const index = controls.indexOf(document.activeElement as HTMLElement);
      if (!controls.length) { event.preventDefault(); panel.current?.focus(); }
      else if (event.shiftKey && index <= 0) { event.preventDefault(); controls.at(-1)?.focus(); }
      else if (!event.shiftKey && (index < 0 || index === controls.length - 1)) { event.preventDefault(); controls[0]?.focus(); }
    };
    window.addEventListener('keydown', key, true);
    return () => { window.removeEventListener('keydown', key, true); if (previous?.isConnected) previous.focus(); };
  }, []);
  return createPortal(<div className="string-template-small-layer" onPointerDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <section ref={panel} className={`kota-confirm-card plain-copy local-model-dialog string-template-small-card${danger ? ' danger' : ''}`}
      role={danger ? 'alertdialog' : 'dialog'} aria-label={title} aria-modal="true" tabIndex={-1}>
      <h2>{title}</h2>{children}<div className="kota-confirm-actions">{actions}</div>
    </section>
  </div>, document.body);
}

interface Failure { code: string; operation: 'read' | 'save' }
function failureCopy(failure: Failure) {
  if (failure.code === 'string_templates_conflict') return 'Settings changed outside this editor. Reload before saving.';
  if (failure.code === 'string_templates_full') return 'Keep at most 50 templates.';
  return failure.operation === 'read' ? 'String templates unavailable.' : 'Could not save String templates.';
}

/** One explicit read on opening; whole-library writes are single-flight CAS.
 * No agent IPC, subscriptions, auto-rebase or background retry. */
export function StringTemplates({ mode, group, catalog, onImport, onClose, onSaved }: {
  mode: 'save' | 'import'; group: AutoRouteGroup | null; catalog: LocalModelCatalog | null;
  onImport: (group: AutoRouteGroup, name: string) => void; onClose: () => void; onSaved: (name: string) => void;
}) {
  const [library, setLibrary] = useState<StringTemplateLibrary | null>(null);
  const [name, setName] = useState('');
  const [conflict, setConflict] = useState<StringTemplate | null>(null);
  const [deleting, setDeleting] = useState<StringTemplate | null>(null);
  const [loading, setLoading] = useState(true), [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const mounted = useRef(true), writing = useRef(false), readSerial = useRef(0);
  const pending = useRef<{ library: StringTemplateLibrary; savedName?: string } | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const read = async (initial = false) => {
    if (writing.current) return;
    const serial = ++readSerial.current; setLoading(true);
    try {
      const next = await readStringTemplates();
      if (!mounted.current || serial !== readSerial.current) return;
      setLibrary(next); setFailure(null); setConflict(null); setDeleting(null); pending.current = null;
      if (initial) { setName(nextStringTemplateName(next.templates)); requestAnimationFrame(() => input.current?.focus()); }
    } catch (error) {
      if (mounted.current && serial === readSerial.current) setFailure({ operation: 'read', code: (error as { code?: string })?.code ?? 'string_templates_read_failed' });
    } finally { if (mounted.current && serial === readSerial.current) setLoading(false); }
  };
  useEffect(() => {
    mounted.current = true; void read(true);
    return () => { mounted.current = false; ++readSerial.current; };
  }, []);
  const close = () => { if (!writing.current) onClose(); };
  const write = async (next: StringTemplateLibrary, savedName?: string) => {
    if (writing.current) return;
    writing.current = true; setBusy(true); setFailure(null); pending.current = { library: next, savedName };
    try {
      const saved = await saveStringTemplates(next);
      if (!mounted.current) return;
      setLibrary(saved); pending.current = null; setDeleting(null);
      if (savedName) { onSaved(savedName); onClose(); }
    } catch (error) {
      if (!mounted.current) return;
      const { code = 'string_templates_write_failed', duplicateId } = (error ?? {}) as { code?: string; duplicateId?: string };
      const duplicate = code === 'string_template_name_exists' && library?.templates.find(item => item.id === duplicateId);
      if (duplicate) { setConflict(duplicate); pending.current = null; }
      else setFailure({ code, operation: 'save' });
    } finally { writing.current = false; if (mounted.current) setBusy(false); }
  };
  const save = (overwrite?: StringTemplate) => {
    if (!library || !group || !name.trim() || [...name.normalize('NFC').trim()].length > 40 || busy || loading || failure) return;
    const item: StringTemplate = { id: overwrite?.id ?? crypto.randomUUID(), name: overwrite?.name ?? name,
      savedAt: new Date().toISOString(), group };
    void write({ ...library, templates: overwrite ? library.templates.map(old => old.id === overwrite.id ? item : old) : [...library.templates, item] }, item.name.trim());
  };
  const status = <>
    {loading && <p role="status">Loading templates…</p>}
    {failure && <p role="status" className="local-model-warning">{failureCopy(failure)}{' '}
      <button type="button" className="local-model-text" disabled={busy || loading} onClick={() => {
        if (failure.code === 'string_templates_conflict') void read();
        else if (failure.operation === 'save' && pending.current) void write(pending.current.library, pending.current.savedName);
        else void read(!name);
      }}>{failure.code === 'string_templates_conflict' ? 'Reload' : 'Retry'}</button>
    </p>}
  </>;
  const blocked = busy || loading || !!failure || !library;
  if (mode === 'save') return <TemplateCard key={conflict?.id ?? 'name'} title={conflict ? 'Template already exists' : 'Save template'} onClose={close}
    actions={conflict ? <><button data-initial-focus disabled={busy || loading} onClick={() => { setConflict(null); setFailure(null); pending.current = null; requestAnimationFrame(() => { input.current?.focus(); input.current?.select(); }); }}>Rename</button>
      <button className="confirm" disabled={blocked || !group} onClick={() => save(conflict)}>Overwrite</button></>
      : <><button disabled={busy} onClick={close}>Cancel</button><button className="confirm" disabled={blocked || !name.trim() || !group} onClick={() => save()}>{busy ? 'Saving…' : 'Save'}</button></>}>
    {conflict ? <><p className="template-card-copy">A template named “{conflict.name}” already exists.</p><p className="template-card-detail">Overwrite replaces the saved template. Earlier imports are unchanged.</p></>
      : <><p className="template-card-copy">Save the current models, intents, default, and threshold.</p>
        <label className="template-name"><span>Name <small>{[...name.normalize('NFC').trim()].length} / 40</small></span>
          <input ref={input} data-initial-focus aria-label="Template name" autoComplete="off" value={name} disabled={busy || loading || failure?.code === 'string_templates_conflict' || failure?.operation === 'read'}
            onChange={event => { setName([...event.target.value].slice(0, 40).join('')); setFailure(null); pending.current = null; }}
            onKeyDown={event => { if (event.key === 'Enter' && !event.nativeEvent.isComposing) { event.preventDefault(); save(); } }} />
        </label></>}{status}
  </TemplateCard>;
  return <>
    <LocalModelDialog title="Import template" subtitle="Saved on this device." onClose={close} closeLabel="Close templates"
      footer={<><p>{library?.templates.length ?? 0} / 50 templates</p><button type="button" className="string-add" disabled={busy} onClick={close}>Cancel</button></>}>
      <div className="template-library-body"><p className="template-import-note">Import replaces the current models, default, and threshold.</p>
        {!deleting && status}
        {library && !loading && !library.templates.length && <div className="template-empty"><StringIcon /><p>No templates yet.</p><small>Save the current Auto String to create one.</small></div>}
        <div className="template-list">{library?.templates.map(item => <article className="template-card" key={item.id} aria-label={item.name}>
          <header><div><h3>{item.name}</h3><time dateTime={item.savedAt}>Saved {new Date(item.savedAt).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' })}</time></div>
            <span className="template-threshold">Threshold <b>{item.group.threshold.toFixed(2)}</b></span></header>
          <ul>{item.group.models.map((model, index) => {
            const provider = catalog?.providers.find(value => value.providerId === model.providerId);
            const entry = catalogModel(catalog, model.providerId, model.modelId);
            const label = localModelName(model.providerId, entry?.name || model.modelId, model.modelId);
            return <li key={model.id}><LocalProviderIcon providerId={model.providerId} /><div>
              <span className="template-model-line" title={`Model ${index + 1} · ${label} · ${model.thinkingLevel}`}><span className="template-model-number">Model {index + 1}</span> · {label}<span className="template-thinking"> · {model.thinkingLevel}</span><LocalModelCredit providerId={model.providerId} /></span>
              <span className="template-model-status">{model.id === item.group.defaultModel && <b className="template-default">Default</b>}{!provider?.linked && <span className="local-model-warning">Not linked</span>}</span>
            </div></li>;
          })}</ul>
          <footer><button type="button" className="local-model-text template-delete" disabled={blocked} onClick={() => setDeleting(item)}>Delete</button>
            <button type="button" className="string-ok" disabled={blocked} onClick={() => { onImport(structuredClone(item.group), item.name); onClose(); }}>Import</button></footer>
        </article>)}</div>
      </div>
    </LocalModelDialog>
    {deleting && <TemplateCard title={`Delete “${deleting.name}”?`} danger onClose={() => { if (!writing.current) setDeleting(null); }}
      actions={<><button data-initial-focus disabled={busy} onClick={() => setDeleting(null)}>Cancel</button>
        <button className="confirm" disabled={blocked} onClick={() => { if (library) void write({ ...library, templates: library.templates.filter(item => item.id !== deleting.id) }); }}>{busy ? 'Deleting…' : 'Delete template'}</button></>}>
      <p className="template-card-copy">This removes the saved template from this device.</p><p className="template-card-detail">Agents and Hero templates that imported it are unchanged.</p>{status}
    </TemplateCard>}
  </>;
}
