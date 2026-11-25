import { useEffect, useLayoutEffect, useRef, useState, type FormEvent } from 'react';
import { createPortal } from 'react-dom';
import { createRoot } from 'react-dom/client';
import { Stage } from '../chrome/Stage';
import { TopBar } from '../chrome/TopBar';
import type { Centerpiece } from '../chrome/Hearth';
import type { DeskTheme, RoomTheme } from '../chrome/ColorPicker';
import '../styles/kota-tokens.css';
import '../styles/canvas.css';
import './project-settings-preview.css';

// Isolated design entry: no App mount, Git commands, auth flow, IPC, or
// workspace persistence. All settings live only in this preview's React state.
const PROJECTS = [
  { id: 'mock-kota', name: 'Kota' },
  { id: 'mock-sample', name: 'Sample project' },
];
const LIVE_AGENTS = new Set(['judy', 'alice', 'bob']);
const TABLE_SLOTS = [null, 'bob', 'judy', null, null, null, null, 'alice'];
const noop = () => {};

function SettingsIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinejoin="round" aria-hidden>
      <path d="m9.5 3-.6 2.3-2 .9-2.1-.7-2.5 4.3 1.6 1.6v2.3l-1.6 1.6 2.5 4.3 2.1-.7 2 .9.6 2.2h5l.6-2.2 2-.9 2.1.7 2.5-4.3-1.6-1.6v-2.3l1.6-1.6-2.5-4.3-2.1.7-2-.9L14.5 3Z" />
      <circle cx="12" cy="12.5" r="3.2" />
    </svg>
  );
}

function MailIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <rect x="3" y="5" width="18" height="14" rx="2.5" />
      <path d="m4 7 8 6 8-6" />
    </svg>
  );
}

function ProjectSettingsMock({ projectName, email, onSave }: {
  projectName: string;
  email: string | null;
  onSave: (value: string | null) => void;
}) {
  const [open, setOpen] = useState(true);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const wrapper = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const action = useRef<HTMLButtonElement>(null);

  const close = () => {
    setOpen(false);
    setEditing(false);
  };

  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !wrapper.current?.contains(event.target)) close();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      close();
      trigger.current?.focus();
    };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('pointerdown', outside);
      document.removeEventListener('keydown', escape);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    if (editing) {
      input.current?.focus();
      input.current?.select();
    } else {
      action.current?.focus();
    }
  }, [open, editing, email]);

  const edit = () => {
    setDraft(email ?? '');
    setEditing(true);
  };
  const save = (event: FormEvent) => {
    event.preventDefault();
    // Deliberately no email syntax check: any nonempty text can be saved.
    onSave(draft === '' ? null : draft);
    setEditing(false);
  };

  return (
    <div className="ps-mock-tool" ref={wrapper}>
      <button
        ref={trigger}
        className={`picker-trigger ps-mock-trigger ${open ? 'open' : ''}`}
        type="button"
        title="Project settings"
        aria-label="Project settings"
        aria-expanded={open}
        aria-controls="project-settings-mock"
        onClick={() => {
          setOpen(!open);
          setEditing(false);
        }}
      >
        <SettingsIcon />
      </button>
      {open && (
        <section
          id="project-settings-mock"
          className="ps-mock-panel"
          role="dialog"
          aria-labelledby="ps-mock-title"
          data-state={editing ? 'editing' : email === null ? 'empty' : 'configured'}
        >
          <header className="ps-mock-head">
            <div>
              <h1 id="ps-mock-title">Project Settings</h1>
              <p className="ps-mock-project"><span aria-hidden />{projectName}</p>
            </div>
            <button className="ps-mock-close" type="button" aria-label="Close project settings" onClick={() => {
              close();
              trigger.current?.focus();
            }}>
              <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden><path d="m5 5 10 10M15 5 5 15" /></svg>
            </button>
          </header>

          <div className="ps-mock-body">
            <div className="ps-mock-setting-title"><MailIcon /><h2>Commit email</h2></div>
            {editing ? (
              <form noValidate onSubmit={save} className="ps-mock-editor">
                <label htmlFor="ps-mock-email">Email used for new commits</label>
                <input
                  ref={input}
                  id="ps-mock-email"
                  type="text"
                  value={draft}
                  onChange={event => setDraft(event.target.value)}
                  placeholder="Enter commit email"
                  autoComplete="off"
                  autoCapitalize="none"
                  spellCheck={false}
                />
                <p className="ps-mock-hint">Agent names stay unchanged.</p>
                <div className="ps-mock-edit-actions">
                  <button className="ps-mock-button secondary" type="button" onClick={() => setEditing(false)}>Cancel</button>
                  <button className="ps-mock-button primary" type="submit">Save</button>
                </div>
              </form>
            ) : email === null ? (
              <div className="ps-mock-empty">
                <p className="ps-mock-status">Commits will use agent dummy emails.</p>
                <button ref={action} className="ps-mock-button primary ps-mock-add" type="button" onClick={edit}>
                  <span aria-hidden>+</span> Add commit email
                </button>
              </div>
            ) : (
              <div className="ps-mock-configured">
                <p className="ps-mock-status" aria-live="polite">Commits will use <span className="ps-mock-email">{email}</span></p>
                <div className="ps-mock-value-actions">
                  <button ref={action} className="ps-mock-button secondary" type="button" onClick={edit}>Edit</button>
                  <button className="ps-mock-button remove" type="button" onClick={() => onSave(null)}>Remove</button>
                </div>
              </div>
            )}
          </div>
          <footer className="ps-mock-foot">Applies to this project only.</footer>
        </section>
      )}
    </div>
  );
}

function Preview() {
  const stage = useRef<HTMLDivElement>(null);
  const [toolbarSlot, setToolbarSlot] = useState<HTMLDivElement | null>(null);
  const [projectId, setProjectId] = useState(PROJECTS[0].id);
  const [emails, setEmails] = useState<Record<string, string | null>>({});
  const [centerpiece, setCenterpiece] = useState<Centerpiece>('fire');
  const [roomTheme, setRoomTheme] = useState<RoomTheme>('classic');
  const [deskTheme, setDeskTheme] = useState<DeskTheme>('warm');
  const [roomColor, setRoomColor] = useState('#C5BBAA');
  const [deskColor, setDeskColor] = useState('#AAA397');
  const project = PROJECTS.find(value => value.id === projectId)!;

  useLayoutEffect(() => {
    // Preview-only slot: place the mock beside the real painter without
    // adding a settings feature or experimental props to production Stage.
    const tools = stage.current?.querySelector('.stage-tools');
    if (!tools) return;
    const slot = document.createElement('div');
    slot.className = 'ps-mock-toolbar-slot';
    tools.insertBefore(slot, tools.children[1] ?? null);
    setToolbarSlot(slot);
    return () => slot.remove();
  }, []);

  return (
    <>
      <TopBar
        projects={PROJECTS}
        activeProjectId={projectId}
        onSelectProject={setProjectId}
        onNewProject={noop}
        onCloseProject={noop}
        onOpenTavern={noop}
      />
      <main className="ps-mock-stage" ref={stage}>
        <Stage
          sceneKey="conversation"
          projectName={project.name}
          liveAgents={LIVE_AGENTS}
          tableSlots={TABLE_SLOTS}
          targetAgent={null}
          onOpenAgent={noop}
          centerpiece={centerpiece}
          roomColor={roomColor}
          deskColor={deskColor}
          roomTheme={roomTheme}
          deskTheme={deskTheme}
          onChangeCenter={setCenterpiece}
          onChangeRoom={setRoomColor}
          onChangeDesk={setDeskColor}
          onChangeRoomTheme={setRoomTheme}
          onChangeDeskTheme={setDeskTheme}
          composer={<div className="ps-mock-composer"><span>Message the room…</span><span className="ps-mock-send" aria-hidden>↑</span></div>}
        />
        {toolbarSlot && createPortal(
          <ProjectSettingsMock
            key={projectId}
            projectName={project.name}
            email={emails[projectId] ?? null}
            onSave={value => setEmails(previous => ({ ...previous, [projectId]: value }))}
          />,
          toolbarSlot,
        )}
      </main>
      <footer className="ps-mock-preview-note"><span>UI MOCK</span> Changes stay in this preview. Refresh to reset.</footer>
    </>
  );
}

createRoot(document.getElementById('root')!).render(<Preview />);
