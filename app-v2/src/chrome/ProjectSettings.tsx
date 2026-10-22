import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { readProjectSettings, saveProjectCommitEmail, type ProjectSettingsValue, type WorkspaceProject } from '../pty-client';
import '../styles/project-settings.css';
import { ProjectGithubLink } from './ProjectGithubLink';
import { ProjectConnectionControl } from './ProjectConnectionControl';
import { PROJECT_CONNECTION_REQUEST } from '../lib/project-connection-ui';

export function ProjectSettings({ projectId, projectName, workspace }: { projectId: string; projectName: string; workspace?: WorkspaceProject | null }) {
  const [open, setOpen] = useState(false);
  const [connectionBlocked, setConnectionBlocked] = useState(false);
  const [connectRequested, setConnectRequested] = useState(false);
  const requestHandled = useCallback(() => setConnectRequested(false), []);
  const wrapper = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    setOpen(false); setConnectRequested(false); setConnectionBlocked(false);
    const request = (event: Event) => {
      if ((event as CustomEvent<{ projectId: string }>).detail?.projectId !== projectId) return;
      setOpen(true); setConnectRequested(true);
    };
    window.addEventListener(PROJECT_CONNECTION_REQUEST, request);
    return () => window.removeEventListener(PROJECT_CONNECTION_REQUEST, request);
  }, [projectId]);

  useEffect(() => {
    if (!open || connectionBlocked) return;
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !wrapper.current?.contains(event.target)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      setOpen(false);
      trigger.current?.focus();
    };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('pointerdown', outside);
      document.removeEventListener('keydown', escape);
    };
  }, [open, connectionBlocked]);

  return (
    <div className="project-settings-tool" ref={wrapper}>
      <button ref={trigger} className={`picker-trigger project-settings-trigger ${open ? 'open' : ''}`}
        type="button" title="Project settings" aria-label="Project settings"
        aria-expanded={open} aria-controls="project-settings-panel" onClick={() => { if (!connectionBlocked) setOpen(!open); }}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinejoin="round" aria-hidden>
          <path d="m9.5 3-.6 2.3-2 .9-2.1-.7-2.5 4.3 1.6 1.6v2.3l-1.6 1.6 2.5 4.3 2.1-.7 2 .9.6 2.2h5l.6-2.2 2-.9 2.1.7 2.5-4.3-1.6-1.6v-2.3l1.6-1.6-2.5-4.3-2.1.7-2-.9L14.5 3Z" />
          <circle cx="12" cy="12.5" r="3.2" />
        </svg>
      </button>
      {open && <ProjectSettingsPanel key={projectId} projectId={projectId} projectName={projectName} workspace={workspace}
        connectionBlocked={connectionBlocked} onBlockingChange={setConnectionBlocked} connectRequested={connectRequested} onRequestHandled={requestHandled} onClose={() => {
        setOpen(false);
        trigger.current?.focus();
      }} />}
    </div>
  );
}

function ProjectSettingsPanel({ projectId, projectName, workspace, onClose, connectionBlocked, onBlockingChange, connectRequested, onRequestHandled }: {
  projectId: string; projectName: string; workspace?: WorkspaceProject | null; onClose: () => void;
  connectionBlocked: boolean; onBlockingChange: (blocked: boolean) => void; connectRequested: boolean; onRequestHandled: () => void;
}) {
  const localOnly = workspace?.connection?.kind === 'local';
  const login = workspace?.connection?.kind === 'github' ? workspace.connection.accountLogin : undefined;
  const [settings, setSettings] = useState<ProjectSettingsValue | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [retry, setRetry] = useState(0);
  const alive = useRef(false);
  const saving = useRef(false);
  const input = useRef<HTMLInputElement>(null);
  const action = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  useEffect(() => {
    let cancelled = false;
    setBusy(true);
    setError(null);
    void readProjectSettings(projectId).then(value => {
      if (!cancelled) setSettings(value);
    }).catch(reason => {
      if (!cancelled) setError(String(reason));
    }).finally(() => { if (!cancelled) setBusy(false); });
    return () => { cancelled = true; };
  }, [projectId, retry]);

  useEffect(() => {
    if (busy || document.querySelector('.github-dialog-layer')) return;
    if (editing) {
      input.current?.focus();
      input.current?.select();
    } else {
      action.current?.focus();
    }
  }, [busy, editing]);

  const edit = () => {
    if (localOnly) return;
    setDraft(settings?.commitEmail ?? '');
    setError(null);
    setSaved(false);
    setEditing(true);
  };
  const save = async (value: string | null) => {
    if (saving.current || localOnly) return;
    saving.current = true;
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      const next = await saveProjectCommitEmail(projectId, value);
      if (!alive.current) return;
      setSettings(next);
      setEditing(false);
      setSaved(true);
    } catch (reason) {
      if (alive.current) setError(String(reason));
    } finally {
      saving.current = false;
      if (alive.current) setBusy(false);
    }
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    void save(draft === '' ? null : draft);
  };

  return (
    <section id="project-settings-panel" className="project-settings-panel" role="dialog" aria-labelledby="project-settings-title" aria-busy={busy}>
      <header className="project-settings-head">
        <div><h1 id="project-settings-title">Project Settings</h1><p className="project-settings-project"><span aria-hidden />{projectName}</p></div>
        <button className="project-settings-close" type="button" aria-label="Close project settings" disabled={connectionBlocked} onClick={onClose}>
          <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden><path d="m5 5 10 10M15 5 5 15" /></svg>
        </button>
      </header>
      <div className="project-settings-body">
        <section className="project-settings-connection" aria-label="GitHub connection">
          {workspace ? <ProjectConnectionControl workspace={workspace} onBlockingChange={onBlockingChange} connectRequested={connectRequested} onRequestHandled={onRequestHandled} />
          : <ProjectGithubLink linked={!localOnly} disabled>
            {localOnly ? 'Local only' : login ? `Connected to GH ID ${login}` : 'Connected'}
          </ProjectGithubLink>}
        </section>
        <div className={localOnly ? 'project-commit-email unavailable' : 'project-commit-email'}>
        <div className="project-settings-setting-title">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden><rect x="3" y="5" width="18" height="14" rx="2.5" /><path d="m4 7 8 6 8-6" /></svg>
          <h2>Commit email</h2>
        </div>
        {settings === null ? (
          <div className="project-settings-status">
            {busy ? 'Loading…' : <button ref={action} className="project-settings-button secondary" type="button" onClick={() => setRetry(value => value + 1)}>Retry</button>}
          </div>
        ) : editing && !localOnly ? (
          <form noValidate onSubmit={submit} className="project-settings-editor">
            <label htmlFor="project-commit-email">Email used for new commits</label>
            <input ref={input} id="project-commit-email" type="text" value={draft} disabled={busy || localOnly}
              onChange={event => setDraft(event.target.value)} placeholder="Enter commit email"
              autoComplete="off" autoCapitalize="none" spellCheck={false} />
            <p className="project-settings-hint">Agent names stay unchanged.</p>
            <div className="project-settings-edit-actions">
              <button className="project-settings-button secondary" type="button" disabled={busy || localOnly} onClick={() => { setEditing(false); setError(null); }}>Cancel</button>
              <button className="project-settings-button primary" type="submit" disabled={busy || localOnly}>{busy ? 'Saving…' : 'Save'}</button>
            </div>
          </form>
        ) : settings.commitEmail === null ? (
          <div>
            <p className="project-settings-status">Commits will use agent dummy emails.</p>
            <button ref={action} className="project-settings-button primary project-settings-add" type="button" disabled={busy || localOnly} onClick={edit}><span aria-hidden>+</span> Add commit email</button>
          </div>
        ) : (
          <div>
            <p className="project-settings-status">{localOnly ? 'Saved commit email' : 'Commits will use'} <span className="project-settings-email">{settings.commitEmail}</span></p>
            <div className="project-settings-value-actions">
              <button ref={action} className="project-settings-button secondary" type="button" disabled={busy || localOnly} onClick={edit}>Edit</button>
              <button className="project-settings-button remove" type="button" disabled={busy || localOnly} onClick={() => void save(null)}>{busy ? 'Removing…' : 'Remove'}</button>
            </div>
          </div>
        )}
        {localOnly && <p className="project-settings-hint">Available when this project is connected to GitHub.</p>}
        </div>
        {error && <p className="project-settings-error" role="alert">{error}</p>}
        {saved && <p className="project-settings-hint" role="status">Saved. Running agents keep their current email until their next start.</p>}
      </div>
      <footer className="project-settings-foot">Applies to this project only. Changes apply to new Bartender commits immediately and to agents on their next start. Existing commits are unchanged.</footer>
    </section>
  );
}
