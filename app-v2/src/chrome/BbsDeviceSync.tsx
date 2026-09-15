import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import {
  bbsInvitationHost, bbsSyncOnlinePeers, bbsSyncTimeLabel,
  type BbsSyncMemberView, type BbsSyncView,
} from '../bbs-sync-view';
import '../styles/bbs-sync.css';
import { BBS_SYNC_ERROR_DETAILS } from '../bbs-sync-errors';
import { BbsSyncError } from './BbsSyncError';

type IconKind = 'devices' | 'refresh' | 'copy' | 'close' | 'lock';

function Icon({ kind }: { kind: IconKind }) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false" className="bbs-sync-icon">
      {kind === 'devices' && <><rect x="2" y="3" width="14" height="11" rx="2" /><path d="M6 18h6M9 14v4" /><rect x="16" y="9" width="6" height="12" rx="1.5" /></>}
      {kind === 'refresh' && <path d="M20 10A8 8 0 0 0 6 6L3 9m0-6v6h6M4 14a8 8 0 0 0 14 4l3-3m0 6v-6h-6" />}
      {kind === 'copy' && <><rect x="8" y="8" width="12" height="13" rx="2" /><path d="M15 8V3H3v12h5" /></>}
      {kind === 'close' && <path d="m6 6 12 12M6 18 18 6" />}
      {kind === 'lock' && <><rect x="5" y="10" width="14" height="11" rx="2" /><path d="M8 10V6a4 4 0 0 1 8 0v4m-4 5v2" /></>}
    </svg>
  );
}

function Hint({ text, children }: { text: string; children: ReactNode }) {
  const id = useId();
  return (
    <span className="bbs-sync-hint" tabIndex={0} aria-describedby={id}>
      {children}
      <span className="bbs-sync-tooltip" id={id} role="tooltip">{text}</span>
    </span>
  );
}

export function BbsSharingMark({ deviceCount }: { deviceCount: number }) {
  if (!Number.isSafeInteger(deviceCount) || deviceCount < 1) return null;
  return <span className="bbs-sharing-mark"><Icon kind="devices" />Sharing on {deviceCount} devices</span>;
}

export function BbsSyncManageButton({ view, expanded = false, onManage }: {
  view: BbsSyncView;
  expanded?: boolean;
  onManage: (trigger: HTMLButtonElement) => void;
}) {
  return <button type="button" className={`bbs-sync-manage ${expanded ? 'expanded' : ''}`}
    aria-haspopup="dialog" aria-expanded={expanded}
    aria-label={view.group ? `Manage ${view.group.members.length} connected devices` : 'Connect devices to BBS'}
    onClick={(event) => onManage(event.currentTarget)}>
    <Icon kind="devices" />
    {view.group && <span className={`bbs-sync-dot ${bbsSyncOnlinePeers(view) === 0 ? 'offline' : ''}`} aria-hidden="true" />}
    {view.group ? `${view.group.members.length} devices` : 'Connect devices'}
  </button>;
}

/** Pure controls: opening BBS never starts a sync through this component. */
export function BbsSyncStatusBar({ view, pending = false, request = null,
  error = view.error ?? (view.phase === 'failed' ? BBS_SYNC_ERROR_DETAILS.unknown : null), onSync, onCancel }: {
  view: BbsSyncView;
  pending?: boolean;
  /** Local command feedback only; does not fabricate a backend round/progress. */
  request?: 'start' | 'cancel' | null;
  /** One merged status/action/read error, including when no group is available. */
  error?: string | null;
  onSync: () => void;
  onCancel: () => void;
}) {
  if (!view.group && !error) return null;
  const busy = !view.controlRecoverable && (view.phase === 'connecting' || view.phase === 'syncing');
  const waiting = request !== null;
  const peerCount = bbsSyncOnlinePeers(view);
  const needsPeer = peerCount === 0 && !view.controlRecoverable;
  const label = request === 'cancel' ? 'Cancelling…' : busy && view.phase === 'connecting' ? 'Connecting…'
    : busy ? view.progress ? `Syncing ${view.progress.completed}/${view.progress.total}` : 'Syncing…'
      : request === 'start' ? 'Starting…'
        : view.controlRecoverable || view.phase === 'partial' || view.phase === 'failed' ? 'Retry sync' : 'Manual sync';
  const syncButton = (
    <button type="button" className={`bbs-sync-now ${view.phase}`} onClick={onSync}
      aria-busy={waiting || busy} disabled={pending || waiting || busy || needsPeer}>
      {busy || waiting ? <span className="bbs-sync-spinner" aria-hidden="true" /> : <Icon kind="refresh" />}{label}
    </button>
  );
  return (
    <div className="bbs-sync-controls">
      {view.group && <div className="bbs-sync-round">
        <span className="bbs-sync-online">{view.group.members.filter((member) => member.online).length} online</span>
        {view.phase === 'partial' && !error && <span className="bbs-sync-result partial" role="status">Partially synced</span>}
        <span className="bbs-sync-last">{bbsSyncTimeLabel(view.lastSuccessfulAt)}</span>
        {needsPeer ? <Hint text="No other device online now.">{syncButton}</Hint> : syncButton}
        {busy && <button type="button" className="bbs-sync-cancel" onClick={onCancel} disabled={pending || waiting}>Cancel</button>}
      </div>}
      {error && <BbsSyncError detail={error} />}
    </div>
  );
}

export interface BbsDeviceSyncActions {
  createInvitation: () => Promise<void>;
  refreshInvitation: () => Promise<void>;
  copyInvitation: (value: string) => Promise<void>;
  join: (invitation: string) => Promise<void>;
  disconnect: () => Promise<void>;
  rename: (name: string) => Promise<void>;
  remove: (deviceId: string) => Promise<void>;
}

type Confirmation = { kind: 'disconnect' } | { kind: 'remove'; member: BbsSyncMemberView } | { kind: 'join'; invitation: string; host: string };

/** Mounted only while open. The caller owns authoritative state and all IO. */
export function BbsDeviceSyncDialog({ view, actions, onClose, returnFocusTo, blocked = false }: {
  view: BbsSyncView;
  actions: BbsDeviceSyncActions;
  onClose: () => void;
  /** Safari does not focus a mouse-clicked button; capture the actual trigger. */
  returnFocusTo?: HTMLElement | null;
  /** Shared controller action in flight (including after closing/reopening). */
  blocked?: boolean;
}) {
  const titleId = useId();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const confirmationCancelRef = useRef<HTMLButtonElement>(null);
  const mounted = useRef(false);
  const inFlight = useRef(false);
  const context = `${view.deviceId}:${view.group?.id ?? ''}`;
  const contextRef = useRef(context);
  contextRef.current = context;
  const [localPending, setPending] = useState(false);
  const pending = localPending || blocked;
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [invitation, setInvitation] = useState('');
  const [name, setName] = useState(view.deviceName);
  const [nameDirty, setNameDirty] = useState(false);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);

  useEffect(() => {
    mounted.current = true;
    const opener = returnFocusTo ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    const dialog = dialogRef.current!;
    dialog.showModal();
    closeRef.current?.focus();
    return () => {
      mounted.current = false;
      dialog.close();
      if (opener?.isConnected) opener.focus();
    };
  }, []);

  useEffect(() => {
    setConfirmation(null);
    setInvitation('');
    setError('');
    setMessage('');
    setNameDirty(false);
  }, [context]);

  useEffect(() => {
    if (!nameDirty) setName(view.deviceName);
  }, [view.deviceName, nameDirty]);

  useEffect(() => {
    if (confirmation) confirmationCancelRef.current?.focus();
  }, [confirmation]);

  async function run(action: () => Promise<void>, successMessage = '') {
    if (inFlight.current) return;
    const startedIn = contextRef.current;
    inFlight.current = true;
    setPending(true);
    setError('');
    setMessage('');
    try {
      await action();
      if (mounted.current && contextRef.current === startedIn) {
        setConfirmation(null);
        setMessage(successMessage);
      }
    } catch (failure) {
      if (mounted.current && contextRef.current === startedIn) {
        // Do not stringify payload objects (which can contain invitation material).
        setError(failure instanceof Error ? failure.message : 'Could not complete this action. Please retry.');
      }
    } finally {
      inFlight.current = false;
      if (mounted.current) setPending(false);
    }
  }

  const owner = view.group?.role === 'owner';
  const member = view.group?.role === 'member';
  // An owner with an empty group can join another group. The backend must revoke
  // its own empty group first; the UI never pretends a local toggle revokes it.
  const canJoin = !view.group || (owner && view.group.members.length === 1);
  const host = bbsInvitationHost(invitation);
  const codeReady = view.invitation.state === 'ready';
  const codePending = view.invitation.state === 'preparing';
  const createLabel = view.group ? 'Create invitation' : 'Create group';

  function joinConfirmation() {
    if (!host) {
      setError('Paste the complete invitation copied from another Kota.');
      return;
    }
    setError('');
    setConfirmation({ kind: 'join', invitation: invitation.trim(), host });
  }

  const confirmationTitle = confirmation?.kind === 'join' ? 'Join this BBS group?'
    : confirmation?.kind === 'remove' ? `Remove ${confirmation.member.name}?`
      : owner ? 'Disconnect and dissolve this group?' : 'Disconnect from this group?';
  const confirmationBody = confirmation?.kind === 'join' ? `Worker: ${confirmation.host}`
    : confirmation?.kind === 'remove' ? 'This device loses group access. Downloaded posts and attachments remain on that device.'
      : owner ? 'This ends the group for all members. Downloaded posts, replies, and attachments remain on every device.'
        : 'Sharing stops on this Mac. Downloaded posts, replies, and attachments remain here.';

  return createPortal(
    <dialog ref={dialogRef} className="bbs-device-dialog" aria-labelledby={titleId}
      onCancel={(event) => {
        event.preventDefault();
        event.stopPropagation();
        if (confirmation) { if (!pending) setConfirmation(null); } else onClose();
      }}
      onClick={(event) => {
        if (event.target !== event.currentTarget || confirmation) return;
        const rect = event.currentTarget.getBoundingClientRect();
        if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) onClose();
      }}>
      <header className="bbs-device-head">
        <h2 id={titleId}>{confirmation ? confirmationTitle : 'Connect other devices to BBS'}</h2>
        <button type="button" ref={closeRef} className="bbs-device-icon-button" aria-label="Close device sync" onClick={onClose}>
          <Icon kind="close" />
        </button>
      </header>
      {confirmation ? <div className="bbs-device-confirm">
        <p>{confirmationBody}</p>
        {error && <p className="bbs-device-error" role="alert">{error}</p>}
        <footer>
          <button type="button" ref={confirmationCancelRef} disabled={pending} onClick={() => { setConfirmation(null); setError(''); }}>Cancel</button>
          <button type="button" disabled={pending} className={confirmation.kind === 'join' ? 'primary' : 'danger'}
            onClick={() => void run(() => confirmation.kind === 'join' ? actions.join(confirmation.invitation)
              : confirmation.kind === 'remove' ? actions.remove(confirmation.member.id) : actions.disconnect())}>
            {pending ? 'Working…' : confirmation.kind === 'join' ? 'Join' : confirmation.kind === 'remove' ? 'Remove' : 'Disconnect'}
          </button>
        </footer>
      </div> : <>
        <div className="bbs-device-columns">
          <section className="bbs-device-column">
            <h3>{member ? 'Invite from this device' : 'Invite a device'}</h3>
            {member ? <div className="bbs-device-disabled">
              <Icon kind="lock" /><strong>Invitations are managed by the group owner.</strong>
              <p>Disconnect before inviting devices to your own group.</p>
            </div> : !view.workerAvailable ? <div className="bbs-device-disabled">
              <Icon kind="devices" /><strong>No LM Worker configured.</strong>
              <Hint text="Enable the LM Worker to create a group."><button type="button" disabled>{createLabel}</button></Hint>
            </div> : <>
              <div className="bbs-device-code-box">
                <code className={`bbs-device-code ${codePending ? 'preparing' : ''}`} aria-live="polite">
                  {view.invitation.state === 'ready' ? view.invitation.value : codePending ? 'Preparing…' : 'No invitation yet'}
                </code>
                <div className="bbs-device-code-actions">
                  {codeReady ? <button type="button" disabled={pending}
                    onClick={() => void run(() => actions.copyInvitation(view.invitation.state === 'ready' ? view.invitation.value : ''), 'Invitation copied.')}>
                    <Icon kind="copy" />Copy invitation
                  </button> : <button type="button" disabled={pending || codePending}
                    onClick={() => void run(actions.createInvitation)}>{codePending ? 'Preparing…' : createLabel}</button>}
                  <Hint text="Replace the unused invitation.">
                    <button type="button" className="bbs-device-icon-button" disabled={pending || !codeReady}
                      aria-label="Refresh invitation code" onClick={() => void run(actions.refreshInvitation)}><Icon kind="refresh" /></button>
                  </Hint>
                </div>
              </div>
              <p className="bbs-device-hint">Single-use code</p>
              {view.invitation.state === 'error' && view.invitation.message !== error
                && <p className="bbs-device-error" role="status">{view.invitation.message}</p>}
            </>}
          </section>
          <section className="bbs-device-column">
            <h3>{canJoin ? 'Join a group' : 'Current group'}</h3>
            {canJoin ? <>
              <textarea className="bbs-device-join-input" aria-label="Invitation" placeholder="Paste invitation…"
                spellCheck={false} autoCapitalize="off" autoCorrect="off" maxLength={4096}
                value={invitation} disabled={pending} onChange={(event) => { setInvitation(event.target.value); setError(''); }} />
              {host && <p className="bbs-device-host">Worker: {host}</p>}
              <button type="button" className="primary bbs-device-join" disabled={pending || !invitation.trim()} onClick={joinConfirmation}>Join</button>
            </> : <p className="bbs-device-group-name">{view.group?.name}</p>}
            {view.group && <>
              <form className="bbs-device-name" onSubmit={(event) => {
                event.preventDefault();
                if (!name.trim() || pending) return;
                void run(async () => { await actions.rename(name.trim()); }, 'Device name saved.');
              }}>
                <label htmlFor={`${titleId}-name`}>This device’s name</label>
                <div><input id={`${titleId}-name`} maxLength={64} value={name} disabled={pending}
                  onChange={(event) => { setName(event.target.value); setNameDirty(true); }} />
                  <button type="submit" disabled={pending || !name.trim() || name.trim() === view.deviceName}>Save</button>
                </div>
              </form>
              <ul className="bbs-device-members" aria-label="Group members">
                {view.group.members.map((person) => <li key={person.id}>
                  <Icon kind="devices" />
                  <div><b>{person.name}</b><small>
                    <span className={`bbs-sync-dot ${person.online ? '' : 'offline'}`} aria-hidden="true" />
                    {person.online ? 'Online' : 'Offline'}{person.id === view.deviceId ? ' · This device' : ''}{person.role === 'owner' ? ' · Owner' : ''}
                  </small></div>
                  {owner && person.id !== view.deviceId && <button type="button" className="bbs-device-remove" disabled={pending}
                    aria-label={`Remove ${person.name}`} onClick={() => { setError(''); setConfirmation({ kind: 'remove', member: person }); }}>Remove</button>}
                </li>)}
              </ul>
            </>}
          </section>
        </div>
        {(error || message) && <p className={`bbs-device-feedback ${error ? 'bbs-device-error' : ''}`} role={error ? 'alert' : 'status'}>{error || message}</p>}
        <footer className="bbs-device-footer">
          {view.group && <button type="button" className="danger" disabled={pending} onClick={() => { setError(''); setConfirmation({ kind: 'disconnect' }); }}>Disconnect</button>}
          <button type="button" className="bbs-device-done" onClick={onClose}>Done</button>
        </footer>
      </>}
    </dialog>, document.body,
  );
}
