import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/** Shared Jev API, fixed by IPC-CONTRACT v5 §4. */
export type JevProviderId = 'openrouter' | 'vercel-ai-gateway' | 'opencode' | 'typesafe';
export interface JevKeyState { providerId: JevProviderId; status: 'none' | 'verified' | 'rejected' }
export interface JevSettings { schema: 1; provider: JevProviderId; keys: JevKeyState[]; ready: boolean }
export const readJevSettings = () => invoke<JevSettings>('local_rpc_jev_read');
export const selectJevProvider = (providerId: JevProviderId) => invoke<JevSettings>('local_rpc_jev_select', { providerId });
export const saveJevKey = (request: { requestId: string; providerId: JevProviderId; key: string }) =>
  invoke<JevSettings>('local_rpc_jev_key_save', { request });
export const cancelJevKey = (requestId: string) => invoke<void>('local_rpc_jev_key_cancel', { requestId });
// The four questions templates are opaque read-only JSON. The UI neither
// assembles a model request nor loads an editable account prompt copy.
export type PuppeteerJevPrompt = 'stringFirst' | 'stringLater' | 'danceContext' | 'danceRule';
export type PuppeteerJevPrompts = Record<PuppeteerJevPrompt, unknown>;
export const readPuppeteerJevPrompts = () => invoke<PuppeteerJevPrompts>('puppeteer_jev_prompts');

/** One read on mount, explicit refresh and shared events. Never polls. */
export function useJevSettings(enabled: boolean) {
  const [settings, setSettings] = useState<JevSettings | null>(null);
  const [loading, setLoading] = useState(enabled);
  const [failed, setFailed] = useState(false);
  const sequence = useRef(0);
  const accept = useCallback((next: JevSettings) => {
    sequence.current += 1;
    setSettings(next); setFailed(false); setLoading(false);
  }, []);
  const refresh = useCallback(async () => {
    if (!enabled) return null;
    const current = ++sequence.current;
    setLoading(true); setFailed(false);
    try {
      const next = await readJevSettings();
      if (sequence.current === current) { setSettings(next); setFailed(false); return next; }
      return null;
    } catch { if (sequence.current === current) setFailed(true); return null; }
    finally { if (sequence.current === current) setLoading(false); }
  }, [enabled]);
  useEffect(() => {
    if (!enabled) return;
    setLoading(true);
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<JevSettings>('local-jev://changed', event => { if (active) accept(event.payload); }).then(stop => {
      if (active) { unlisten = stop; void refresh(); } else stop();
    }).catch(() => { if (active) void refresh(); });
    return () => { active = false; sequence.current += 1; unlisten?.(); };
  }, [enabled, accept, refresh]);
  return { settings, loading, failed, refresh, accept };
}
