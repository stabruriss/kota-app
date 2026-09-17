import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  BbsSyncClientError, bbsSyncViewSource, bbsSyncInvitation, bbsSyncJoin, bbsSyncDisconnect,
  bbsSyncRename, bbsSyncRemove, bbsSyncStart, bbsSyncCancel,
} from '../bbs-sync-client';
import type { BbsSyncInvitationResult } from '../types/bbs-sync';
import type { BbsSyncView, BbsSyncInvitationView } from '../bbs-sync-view';
import { BbsDeviceSyncDialog, BbsSharingMark, BbsSyncManageButton, BbsSyncStatusBar } from './BbsDeviceSync';
import { useBbsSyncView, type BbsSyncViewSource } from './useBbsSyncView';
import { BBS_SYNC_ERROR_DETAILS, isBbsSyncSafeActionErrorCode, type BbsSyncSafeActionErrorCode } from '../bbs-sync-errors';
import { BbsSyncIndicator } from './BbsSyncIndicator';
import { BBS_SYNC_INDICATOR_COPY, bbsSyncActionIndicator } from '../bbs-sync-indicator';

const realActions = Object.freeze({
  invitation: bbsSyncInvitation, join: bbsSyncJoin, disconnect: bbsSyncDisconnect,
  rename: bbsSyncRename, remove: bbsSyncRemove, start: bbsSyncStart, cancel: bbsSyncCancel,
});
type Actions = typeof realActions;
type RunAction = <T>(expectedGroupId: string | null, action: () => Promise<T>) => Promise<T>;
interface Controls {
  view: BbsSyncView | null;
  loading: boolean;
  error: string | null;
  pending: boolean;
  refresh: () => Promise<void>;
  run: RunAction;
  actions: Actions;
}
const ControlContext = createContext<Controls | null>(null);
const SharingContext = createContext({ groupId: null as string | null, devices: 0 });
/** Stable membership-only projection; subscribing does not read/start sync. */
export function useBbsSharing() { return useContext(SharingContext); }

function useControls() {
  const value = useContext(ControlContext);
  if (!value) throw new Error('BBS sync controls require their visible scope.');
  return value;
}
function safeActionMessage(error: unknown) {
  return error instanceof BbsSyncClientError ? error.message : 'Could not complete this device action. Please retry.';
}

/** Mount only inside the open BBS. Progress changes render context consumers,
 * not the supplied children (the thread list/editor remain opaque to this scope).
 * Sharing has its own stable context so byte progress does not render each row. */
export function BbsSyncScope({ children, source = bbsSyncViewSource, actions = realActions }: {
  children: ReactNode;
  source?: BbsSyncViewSource;
  actions?: Actions;
}) {
  const { view, loading, error, refresh } = useBbsSyncView(true, source);
  const viewRef = useRef(view);
  viewRef.current = view;
  const alive = useRef(false);
  const inFlight = useRef(false);
  const [pending, setPending] = useState(false);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const run = useCallback<RunAction>(async (expectedGroupId, action) => {
    if (!viewRef.current || (viewRef.current.group?.id ?? null) !== expectedGroupId) {
      throw new Error('The BBS group changed. Please try again.');
    }
    if (inFlight.current) throw new Error('Another device action is still in progress.');
    inFlight.current = true;
    setPending(true);
    try {
      return await action();
    } catch (failure) {
      throw failure instanceof BbsSyncClientError ? failure : new Error(safeActionMessage(failure));
    } finally {
      inFlight.current = false;
      if (alive.current) { setPending(false); refresh(); }
    }
  }, [refresh]);
  const controls = useMemo(() => ({ view, loading, error, refresh, pending, run, actions }), [view, loading, error, refresh, pending, run, actions]);
  const groupId = view?.group?.id ?? null;
  const devices = view?.group?.members.length ?? 0;
  const sharing = useMemo(() => ({ groupId, devices }), [groupId, devices]);
  return <ControlContext.Provider value={controls}>
    <SharingContext.Provider value={sharing}>{children}</SharingContext.Provider>
  </ControlContext.Provider>;
}

export function BbsThreadSharing({ sharingGroupId }: { sharingGroupId?: string | null }) {
  const { groupId, devices } = useContext(SharingContext);
  return groupId && sharingGroupId === groupId ? <BbsSharingMark deviceCount={devices} /> : null;
}

export function BbsSyncControlButton() {
  const { view, loading } = useControls();
  const [opener, setOpener] = useState<HTMLButtonElement | null>(null);
  if (!view) return <button type="button" className="bbs-sync-manage" disabled aria-busy={loading}>Connect devices</button>;
  return <>
    <BbsSyncManageButton view={view} expanded={opener !== null} onManage={setOpener} />
    {opener && <BbsSyncManagement key={JSON.stringify([view.deviceId, view.group?.id ?? null])}
      view={view} opener={opener} onClose={() => setOpener(null)} />}
  </>;
}

export function BbsSyncActivity() {
  const { view, error, pending, refresh, run, actions } = useControls();
  const [failure, setFailure] = useState<{ stamp: string; message: string; code: BbsSyncSafeActionErrorCode | null } | null>(null);
  const [request, setRequest] = useState<'start' | null>(null);
  const alive = useRef(false);
  const viewRef = useRef(view);
  viewRef.current = view;
  const sequence = useRef(0);
  const requesting = useRef(false);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => {
    sequence.current += 1;
    requesting.current = false;
    setRequest(null);
    setFailure(null);
  }, [view?.deviceId, view?.group?.id]);
  async function start() {
    const expectedGroupId = view?.group?.id;
    if (!expectedGroupId || pending || requesting.current) return;
    requesting.current = true;
    const id = ++sequence.current;
    const stamp = syncErrorStamp(view);
    setRequest('start');
    setFailure(null);
    try {
      await run(expectedGroupId, () => actions.start({ expectedGroupId }));
      // An ACK only admits the command. Keep immediate local feedback through
      // the next fresh memory read, without inventing a round or a success time.
      if (alive.current && sequence.current === id) await refresh();
    } catch (error) {
      // A late rejection cannot cover a newer authoritative phase/error. Plain
      // status rereads do not erase an action error while nothing has changed.
      if (alive.current && sequence.current === id && syncErrorStamp(viewRef.current) === stamp) {
        const code = error instanceof BbsSyncClientError && isBbsSyncSafeActionErrorCode(error.code) ? error.code : null;
        setFailure({ stamp, code, message: code && error instanceof BbsSyncClientError
          ? error.message
          : BBS_SYNC_ERROR_DETAILS.unknown });
      }
    } finally {
      if (alive.current && sequence.current === id) {
        requesting.current = false;
        setRequest(null);
      }
    }
  }
  const actionFailure = failure?.stamp === syncErrorStamp(view) ? failure : null;
  const visibleError = actionFailure?.message || error || view?.error
    || (view?.phase === 'failed' ? BBS_SYNC_ERROR_DETAILS.unknown : null);
  // A transient local read/action failure cannot disprove an authoritative
  // blocker. Only a newer typed status can withdraw that evidence.
  const localIndicator = actionFailure
    ? bbsSyncActionIndicator(actionFailure.code)
    : error ? 'reconnecting' : undefined;
  const retainBlocker = localIndicator && BBS_SYNC_INDICATOR_COPY[localIndicator].tone !== 'red'
    && view?.indicator && BBS_SYNC_INDICATOR_COPY[view.indicator].tone === 'red';
  const indicator = retainBlocker ? view!.indicator : localIndicator;
  const detail = retainBlocker ? null : actionFailure?.message || error;
  if (!view?.group && !visibleError && (!view?.indicator || view.indicator === 'healthy')) return null;
  return <div className="bbs-sync-activity">
    {view ? <BbsSyncStatusBar view={view} pending={pending} request={request} error={visibleError}
      indicator={indicator} detail={detail} expiredRequest={!retainBlocker && actionFailure?.code === 'stale_signature'}
      onSync={() => void start()} />
      : visibleError && <BbsSyncIndicator indicator="reconnecting" detail={visibleError} />}
    {error && <div className="bbs-sync-controls"><button type="button" onClick={() => void refresh()}>Retry status</button></div>}
  </div>;
}

/** Error ownership is backend-private. This is only a presentation change key,
 * not a code/message classifier or an authorization to recover. */
function syncErrorStamp(view: BbsSyncView | null) {
  return JSON.stringify([view?.deviceId, view?.group?.id, view?.phase, view?.error,
    view?.controlRecoverable, view?.serviceRecoverable, view?.indicator, view?.lastSuccessfulAt, view?.progress]);
}

/** Owns all raw invitation material. Unmount/close drops it; the public provider
 * above never receives the code, and a different generation is never displayed. */
function BbsSyncManagement({ view, opener, onClose }: {
  view: BbsSyncView; opener: HTMLButtonElement; onClose: () => void;
}) {
  const { actions, run, pending } = useControls();
  const [code, setCode] = useState<BbsSyncInvitationResult | null>(null);
  const [attempt, setAttempt] = useState<{ key: string; state: BbsSyncInvitationView } | null>(null);
  const alive = useRef(false);
  const autoAttempt = useRef<string | null>(null);
  const expectedGroupId = view.group?.id ?? null;
  const generation = view.invitationGeneration ?? null;
  const key = JSON.stringify([expectedGroupId, generation]);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const fetchInvitation = useCallback(async (refresh: boolean) => {
    setCode(null);
    setAttempt({ key, state: { state: 'preparing' } });
    autoAttempt.current = key;
    try {
      const result = await run(expectedGroupId, () => actions.invitation({ expectedGroupId, refresh }));
      // Keep Preparing until public status confirms this generation/group. In
      // particular, creating a group must not re-enable Create in the read gap.
      if (alive.current) setCode(result);
    } catch (error) {
      const message = error instanceof BbsSyncClientError
        && isBbsSyncSafeActionErrorCode(error.code)
        ? error.message : 'Could not prepare an invitation. Please retry.';
      if (alive.current) setAttempt({ key, state: { state: 'error', message } });
      throw new Error(message);
    }
  }, [actions, expectedGroupId, key, run]);
  const matchingCode = code?.groupId === expectedGroupId && code.generation === generation ? code : null;
  useEffect(() => {
    // One attempt per authoritative generation, not one per status hint/tick.
    // Manual retry remains available after failure. No code fetch while closed,
    // unjoined or a member; opening a local BBS never creates a group.
    if (view.group?.role !== 'owner' || generation === null || pending || matchingCode || autoAttempt.current === key) return;
    void fetchInvitation(false).catch(() => {});
  }, [view.group?.role, generation, key, pending, matchingCode, fetchInvitation]);
  const invitation: BbsSyncInvitationView = matchingCode ? { state: 'ready', value: matchingCode.invitation }
    : attempt?.key === key ? attempt.state
      : generation !== null ? { state: 'preparing' } : view.invitation;
  const dialogActions = {
    createInvitation: () => fetchInvitation(false),
    refreshInvitation: () => fetchInvitation(true),
    copyInvitation: async (value: string) => {
      if (!matchingCode || value !== matchingCode.invitation) throw new Error('Invitation changed. Please copy the current code.');
      try { await navigator.clipboard.writeText(value); }
      catch { throw new Error('Could not copy the invitation. Please retry.'); }
    },
    join: (value: string) => run(expectedGroupId, () => actions.join({ expectedGroupId, invitation: value })),
    disconnect: () => run(expectedGroupId, () => actions.disconnect({ expectedGroupId })),
    rename: (name: string) => run(expectedGroupId, () => actions.rename({ expectedGroupId, name })),
    remove: (deviceId: string) => run(expectedGroupId, () => actions.remove({ expectedGroupId, deviceId })),
  };
  return <BbsDeviceSyncDialog view={{ ...view, invitation }} actions={dialogActions} blocked={pending} onClose={onClose} returnFocusTo={opener} />;
}
