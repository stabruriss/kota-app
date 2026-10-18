import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { readDance, saveDance, type DanceSettings, type DanceSettingsSnapshot } from './puppeteer-dance';

export interface DanceAgent { id: string; name: string; avatarId?: string | null }
export function danceSettingsIssue(settings: DanceSettings, agents: readonly DanceAgent[]): string | null {
  if (settings.strategy === 'rule' && agents.filter(agent => settings.conditions[agent.id]?.trim()).length < 2) return 'Add routing conditions for at least 2 agents.';
  if (agents.some(agent => Array.from(settings.conditions[agent.id] ?? '').length > 200)) return 'Keep each routing condition within 200 characters.';
  if (!Number.isFinite(settings.threshold) || settings.threshold < .05 || settings.threshold > .95 || Math.abs(settings.threshold * 20 - Math.round(settings.threshold * 20)) > 1e-8) return 'Choose a valid probability threshold.';
  return null;
}
export function danceErrorText(error: unknown): string {
  const code = typeof error === 'string' ? error : (error as { code?: string } | null)?.code;
  switch (code) {
    case 'jev_auth': return 'Jev authentication failed. Update the key in Puppeteer.';
    case 'jev_key_rejected': return 'Jev key rejected. Update it in Puppeteer.';
    case 'jev_key_required': return 'Set up Jev in Puppeteer to enable Dance.';
    case 'jev_timeout': return 'Jev did not respond within 3 seconds.';
    case 'jev_unreachable': return 'Jev could not be reached.';
    case 'dance_rules_required': return 'Add routing conditions for at least 2 agents.';
    case 'dance_sentinel_required': return 'The sentinel has left the table. Choose a recipient for this message.';
    case 'dance_seat_required': return 'Seat at least one agent at the table before starting Dance.';
    case 'dance_roster_unavailable': return 'Agent roster unavailable. Try again.';
    case 'dance_busy': return 'Dance is deciding another recipient. Try again.';
    case 'dance_settings_conflict': return 'Settings changed outside this editor. Reload before saving.';
    case 'jev_settings_read_failed': return 'Jev settings unavailable.';
    case 'dance_disabled': case 'dance_settings_changed': return 'Dance settings changed. Send the message again.';
    case 'dance_record_failed': return 'Could not record this message. It was not sent.';
    default: return 'Dance unavailable. Try again.';
  }
}

/** Explicit file reads and existing save events only. No model probe or polling. */
export function useDanceSettings(projectRoot: string | null, enabled = true) {
  const [snapshot, setSnapshot] = useState<(DanceSettingsSnapshot & { root: string }) | null>(null);
  const [loading, setLoading] = useState(!!projectRoot && enabled);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const failure = useRef<unknown>(null);
  const sequence = useRef(0);
  const activeRoot = useRef(projectRoot); activeRoot.current = projectRoot;
  const saveTail = useRef<Promise<unknown>>(Promise.resolve());
  const saving = useRef(0);
  const latest = useRef<(DanceSettingsSnapshot & { root: string }) | null>(null);
  // The runtime sentinel moves every round via its own event. A read or save that was
  // in flight when an event arrived must not roll the pushed value back.
  const [sentinel, setSentinel] = useState<{ root: string; value: string | null } | null>(null);
  const pushes = useRef(0);
  const accept = useCallback((root: string, next: DanceSettingsSnapshot, pushMark?: number) => {
    if (activeRoot.current !== root) return;
    ++sequence.current; latest.current = { root, ...next }; setSnapshot(latest.current);
    if (pushMark === undefined || pushMark === pushes.current) setSentinel({ root, value: next.currentSentinel });
    setLoading(false); setError(null); failure.current = null;
  }, []);
  const refresh = useCallback(async (): Promise<DanceSettingsSnapshot | null> => {
    if (!projectRoot || !enabled) return null;
    const serial = ++sequence.current; setLoading(true); setError(null); failure.current = null;
    try {
      const mark = pushes.current;
      const next = await readDance(projectRoot);
      if (serial !== sequence.current || activeRoot.current !== projectRoot) return null;
      accept(projectRoot, next, mark); return next;
    } catch (reason) {
      if (serial === sequence.current) { failure.current = reason; setError(danceErrorText(reason)); }
      return null;
    } finally { if (serial === sequence.current) setLoading(false); }
  }, [projectRoot, enabled, accept]);
  useEffect(() => {
    if (!projectRoot || !enabled) return;
    let active = true; let stops: (() => void)[] = [];
    setLoading(true);
    void Promise.all([
      listen<{ projectRoot: string; currentSentinel: string | null }>('puppeteer-dance://sentinel', event => {
        if (!active || event.payload.projectRoot !== projectRoot) return;
        ++pushes.current; setSentinel({ root: projectRoot, value: event.payload.currentSentinel });
      }),
      listen<DanceSettingsSnapshot & { projectRoot: string }>('puppeteer-dance://changed', event => {
        if (!active || event.payload.projectRoot !== projectRoot) return;
        ++pushes.current; accept(projectRoot, event.payload);
      }),
    ]).then(unlisten => { if (active) { stops = unlisten; void refresh(); } else unlisten.forEach(stop => stop()); }).catch(() => { if (active) void refresh(); });
    return () => { active = false; ++sequence.current; stops.forEach(stop => stop()); };
  }, [projectRoot, enabled, accept, refresh]);
  const save = useCallback((input: DanceSettings | ((current: DanceSettings) => DanceSettings), expectedRevision?: string): Promise<DanceSettings | null> => {
    if (!projectRoot || !enabled) return Promise.resolve(null);
    // Object drafts retain the baseline they were edited from, including while queued.
    // Functional actions (notably Off) intentionally apply to the last completed save.
    const baseline = expectedRevision ?? (latest.current?.root === projectRoot ? latest.current.sourceRevision : undefined);
    ++saving.current; setBusy(true);
    const task = saveTail.current.then(async () => {
      try {
        if (activeRoot.current !== projectRoot) return null;
        const current = latest.current?.root === projectRoot ? latest.current : null;
        const settings = typeof input === 'function' ? current && input(current.settings) : input;
        const revision = typeof input === 'function' ? current?.sourceRevision : baseline;
        if (!settings || revision === undefined) return null;
        setError(null); failure.current = null;
        const mark = pushes.current;
        const next = await saveDance(projectRoot, settings, revision); accept(projectRoot, next, mark);
        return activeRoot.current === projectRoot ? next.settings : null;
      } catch (reason) {
        if (activeRoot.current === projectRoot) { failure.current = reason; setError(danceErrorText(reason)); }
        return null;
      } finally { if (--saving.current === 0) setBusy(false); }
    });
    saveTail.current = task;
    return task;
  }, [projectRoot, enabled, accept]);
  const current = snapshot?.root === projectRoot && enabled ? snapshot : null;
  const currentSentinel = sentinel?.root === projectRoot && enabled ? sentinel.value : null;
  return { projectRoot, settings: current?.settings ?? null, sourceRevision: current?.sourceRevision, currentSentinel,
    loading, busy, error, currentError: () => failure.current, save, refresh };
}
export type DanceController = ReturnType<typeof useDanceSettings>;
