import { invoke } from '@tauri-apps/api/core';

export interface MagpieSettings { mode: 'local' | 'lan'; ip: string; port: number }
export interface MagpieState {
  settings: MagpieSettings; linked: boolean; keySaved: boolean; modelCount: number;
  /** Supplied by Rust's single DEFAULT_KEY constant, never a stored gateway credential. */
  defaultKey: string;
  error?: string;
}
export const MAGPIE_CHANGED = 'kota:magpie-changed';
export const readMagpie = () => invoke<MagpieState>('local_rpc_magpie_read');
let pending: Promise<MagpieState> | null = null;
function publish(request: Promise<MagpieState>) {
  const result = request.then(state => {
    window.dispatchEvent(new CustomEvent(MAGPIE_CHANGED, { detail: state }));
    return state;
  });
  pending = result;
  void result.finally(() => { if (pending === result) pending = null; }).catch(() => {});
  return result;
}
/** Only explicit Link/model-picker openings call this. Coalesce concurrent UI openings. */
export async function probeMagpie(): Promise<MagpieState> {
  return pending ?? publish(invoke<MagpieState>('local_rpc_magpie_probe'));
}
/** Saving includes its one probe; closing the dialog must not trigger another. */
export function saveMagpie(settings: MagpieSettings, key?: string): Promise<MagpieState> {
  const preceding = pending;
  return publish((async () => {
    await preceding?.catch(() => {});
    return invoke<MagpieState>('local_rpc_magpie_save', { request: { settings, key } });
  })());
}
export function onMagpieChanged(receive: (state: MagpieState) => void) {
  const listener = (event: Event) => receive((event as CustomEvent<MagpieState>).detail);
  window.addEventListener(MAGPIE_CHANGED, listener);
  return () => window.removeEventListener(MAGPIE_CHANGED, listener);
}
export function magpieErrorText(error: unknown) {
  const code = typeof error === 'string' ? error : (error as { code?: string } | null)?.code;
  switch (code) {
    case 'unreachable': return 'Magpie could not be reached.';
    case 'not_magpie': return 'This address did not identify itself as Magpie.';
    case 'key_rejected': return 'Magpie rejected the gateway key.';
    case 'catalog_failed': return 'The model list could not be read.';
    default: return 'Could not save Magpie settings.';
  }
}
export function localModelOption(provider: string, id: string, name: string) {
  return { id: `${provider}/${id}`, label: name, source: 'provider',
    ...(provider === 'magpie' ? { displayLabel: localModelLabel(provider, id, name) } : {}) };
}
export function localModelLabel(provider: string, id: string, name?: string) {
  if (provider !== 'magpie') return `${provider} / ${name || id}`;
  const slash = id.indexOf('/');
  const upstream = slash < 0 ? '' : id.slice(0, slash);
  const label = !name || name === id ? id.slice(slash + 1) : name;
  return `Magpie / ${upstream ? upstream + ' / ' : ''}${label}`;
}
