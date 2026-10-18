import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/** Global settings; first-use defaults and the CAS revision belong to the backend. */
export interface SoulPushSettings {
  schema: 1;
  enabled: boolean;
  markThreshold: number;
  dreamThreshold: number;
  maxMarks: number;
  maxDreams: number;
  cooldownReplies: number;
  minimumMessageLength: number;
}
export interface SoulPushSettingsSnapshot {
  settings: SoulPushSettings;
  sourceRevision: string;
  jevReady: boolean;
}
export type SoulPushPatch = Partial<Omit<SoulPushSettings, 'schema'>>;
export const readSoulPushSettings = () => invoke<SoulPushSettingsSnapshot>('puppeteer_soul_read');
export const saveSoulPushSettings = (settings: SoulPushSettings, sourceRevision: string) =>
  invoke<SoulPushSettingsSnapshot>('puppeteer_soul_save', { settings, sourceRevision });

interface Failure { operation: 'read' | 'save'; code: string }
interface EditorState {
  snapshot: SoulPushSettingsSnapshot | null;
  /** Captured at the start of an edit, never rebased onto an external revision. */
  draft: SoulPushSettingsSnapshot | null;
  jevReady: boolean | null;
  loading: boolean;
  busy: boolean;
  error: Failure | null;
  resetKey: number;
}
const codeOf = (error: unknown, fallback: string) =>
  typeof error === 'string' ? error : (error as { code?: string } | null)?.code ?? fallback;
const equalSettings = (a: SoulPushSettings, b: SoulPushSettings) =>
  (Object.keys(a) as (keyof SoulPushSettings)[]).every(key => a[key] === b[key]);
export function soulPushErrorText(error: Failure): string {
  if (error.code === 'soul_settings_conflict') return 'Settings changed outside this editor. Reload before saving.';
  if (error.code.startsWith('jev_')) return 'Set up Jev under String to enable.';
  return error.operation === 'read' ? 'Mark and Dream settings unavailable.' : 'Could not save Mark and Dream settings.';
}

/** Subscribe before reading. Edits save one at a time, without retries or polling. */
export function useSoulPushSettings() {
  const [state, setState] = useState<EditorState>({ snapshot: null, draft: null, jevReady: null, loading: true, busy: false, error: null, resetKey: 0 });
  const current = useRef(state);
  const mounted = useRef(true);
  const readSequence = useRef(0);
  const jevSequence = useRef(0);
  const update = useCallback((patch: Partial<EditorState>) => {
    if (!mounted.current) return;
    current.current = { ...current.current, ...patch };
    setState(current.current);
  }, []);
  const accept = useCallback((snapshot: SoulPushSettingsSnapshot, discard = false) => {
    update({ snapshot, jevReady: snapshot.jevReady, loading: false,
      ...(discard ? { draft: null, error: null, resetKey: current.current.resetKey + 1 } : current.current.error?.operation === 'read' ? { error: null } : {}) });
  }, [update]);
  const read = useCallback(async (discard = false) => {
    const serial = ++readSequence.current;
    update({ loading: true });
    try {
      const next = await readSoulPushSettings();
      if (mounted.current && serial === readSequence.current) accept(next, discard);
    } catch (error) {
      if (mounted.current && serial === readSequence.current) update({ loading: false, jevReady: null,
        error: current.current.error?.operation === 'save' ? current.current.error : { operation: 'read', code: codeOf(error, 'soul_settings_read_failed') } });
    }
  }, [accept, update]);
  useEffect(() => {
    mounted.current = true;
    let active = true;
    const stops: (() => void)[] = [];
    const subscribe = async <T,>(name: string, callback: (payload: T) => void) => {
      try {
        const stop = await listen<T>(name, event => { if (active) callback(event.payload); });
        if (active) stops.push(stop); else stop();
      } catch { /* A failed subscription must not prevent the explicit read. */ }
    };
    void Promise.all([
      subscribe<SoulPushSettingsSnapshot>('puppeteer-soul://changed', snapshot => { ++readSequence.current; accept(snapshot); }),
      subscribe('local-jev://changed', () => { ++jevSequence.current; update({ jevReady: null }); void read(); }),
    ]).then(() => { if (active) void read(); });
    return () => { active = false; mounted.current = false; ++readSequence.current; stops.forEach(stop => stop()); };
  }, [accept, read, update]);

  const beginEdit = useCallback(() => {
    const view = current.current;
    if (!view.snapshot || view.busy || view.loading || view.draft) return;
    update({ draft: { ...view.snapshot, settings: { ...view.snapshot.settings } } });
  }, [update]);
  const cancelEdit = useCallback(() => {
    if (!current.current.busy && current.current.error?.operation !== 'save') update({ draft: null });
  }, [update]);
  const save = useCallback(async (draft: SoulPushSettingsSnapshot) => {
    if (current.current.busy) return;
    ++readSequence.current;
    const jevAtStart = jevSequence.current;
    update({ draft, busy: true, error: null });
    try {
      const next = await saveSoulPushSettings(draft.settings, draft.sourceRevision);
      if (!mounted.current) return;
      ++readSequence.current;
      // A newer event may have arrived after our save. Do not roll it back when
      // the invoke response arrives; no local edit is allowed during this write.
      const observed = current.current.snapshot;
      const latest = observed && observed.sourceRevision !== draft.sourceRevision && observed.sourceRevision !== next.sourceRevision ? observed : next;
      const jevChanged = jevAtStart !== jevSequence.current;
      update({ snapshot: latest, draft: null, jevReady: jevChanged ? null : latest.jevReady, busy: false, loading: false, error: null });
      if (jevChanged) void read();
    } catch (error) {
      if (!mounted.current) return;
      const code = codeOf(error, 'soul_settings_write_failed');
      update({ busy: false, error: { operation: 'save', code } });
      if (code.startsWith('jev_')) { update({ jevReady: null }); void read(); }
    }
  }, [read, update]);
  const commit = useCallback((patch: SoulPushPatch) => {
    const view = current.current;
    if (view.busy || view.loading || !view.snapshot) return;
    const base = view.draft ?? view.snapshot;
    const draft = { ...base, settings: { ...base.settings, ...patch } };
    if (equalSettings(draft.settings, base.settings)) {
      if (!view.error) update({ draft: null });
      return;
    }
    update({ draft });
    // Keep editing a failed draft, but only the explicit Retry can send it again.
    if (!view.error) void save(draft);
  }, [save, update]);
  const retry = useCallback(() => {
    const view = current.current;
    if (view.busy || view.loading || view.error?.code === 'soul_settings_conflict') return;
    if (view.error?.operation === 'save' && view.draft) void save(view.draft);
    else void read();
  }, [read, save]);
  const reload = useCallback(() => { if (!current.current.busy) void read(true); }, [read]);
  return { ...state, settings: state.draft?.settings ?? state.snapshot?.settings ?? null, beginEdit, cancelEdit, commit, retry, reload };
}
