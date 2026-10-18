import { lstatSync, readFileSync } from 'node:fs';
import { basename, dirname, isAbsolute, join } from 'node:path';
import { BUNDLED_PROMPTS, PROMPT_FILE, PROMPT_INVALID, PROMPT_UNREADABLE, validPuppeteerPrompts, type PuppeteerPrompts } from './puppeteer-prompts-core.ts';

export interface LoadedPrompts { prompts: PuppeteerPrompts; path: string; warning?: string }
/** Only the launcher's owned kota-local root determines the account path. No HOME/cwd fallback. */
export function puppeteerPromptPath(ownedRoot: string): string | undefined {
  if (!isAbsolute(ownedRoot) || basename(ownedRoot) !== 'kota-local' || basename(dirname(ownedRoot)) !== 'providers') return undefined;
  return join(dirname(dirname(ownedRoot)), 'heroes', 'system-puppeteer', PROMPT_FILE);
}
/** Read once for a fresh classification. The extension never creates/repairs the file. */
export function loadPuppeteerPrompts(ownedRoot: string): LoadedPrompts {
  const path = puppeteerPromptPath(ownedRoot);
  const fallback = (warning: string): LoadedPrompts => ({ prompts: BUNDLED_PROMPTS, path: path ?? '', warning });
  if (!path) return fallback(PROMPT_UNREADABLE);
  let raw: string;
  try {
    const stat = lstatSync(path);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 1024 * 1024) return fallback(PROMPT_INVALID);
    const bytes = readFileSync(path);
    if (bytes.length > 1024 * 1024) return fallback(PROMPT_INVALID);
    raw = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch { return fallback(PROMPT_UNREADABLE); }
  try {
    const value: unknown = JSON.parse(raw);
    return validPuppeteerPrompts(value) ? { prompts: value, path } : fallback(PROMPT_INVALID);
  } catch { return fallback(PROMPT_INVALID); }
}
