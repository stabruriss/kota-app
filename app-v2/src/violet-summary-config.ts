import promptTemplate from '../src-tauri/prompts/violet-summary.md?raw';
import { SYSTEM_STORAGE_KEY } from './magi-config';
import { readSystemPrompt } from './pty-client';

export type VioletSummaryProvider = 'codex' | 'claude';

export interface VioletSummaryConfig {
  provider: VioletSummaryProvider;
  triggerAMessages: number;
  triggerBHours: number;
  triggerBMinOutstanding: number;
}

export const VIOLET_SUMMARY_PROMPT_PATH = '$KOTA_HOME/heroes/system-violet/violet-summary.md';
export const VIOLET_SUMMARY_LOG_PATH = 'project-memory/chathistory/summaries/recent.json';
export const VIOLET_SUMMARY_PROMPT_TEMPLATE = promptTemplate.trimEnd();
export const VIOLET_SUMMARY_CLI_TIMEOUT_SECS = 225;
export const DEFAULT_VIOLET_SUMMARY_CONFIG: VioletSummaryConfig = {
  provider: 'codex',
  triggerAMessages: 30,
  triggerBHours: 2,
  triggerBMinOutstanding: 5,
};

export function normalizeVioletSummaryProvider(value: unknown): VioletSummaryProvider {
  return value === 'claude' ? 'claude' : 'codex';
}

export function violetSummaryCommand(provider: VioletSummaryProvider): string {
  if (provider === 'claude') {
    return [
      'claude',
      '-p',
      '--output-format',
      'text',
      '--no-session-persistence',
      '--permission-mode',
      'bypassPermissions',
      '<',
      VIOLET_SUMMARY_PROMPT_PATH,
    ].join(' ');
  }
  return [
    'codex',
    'exec',
    '--skip-git-repo-check',
    '--sandbox',
    'read-only',
    '--ignore-rules',
    '--color',
    'never',
    '<',
    VIOLET_SUMMARY_PROMPT_PATH,
  ].join(' ');
}

export function loadVioletSummaryConfig(): VioletSummaryConfig {
  if (typeof window === 'undefined') return DEFAULT_VIOLET_SUMMARY_CONFIG;
  try {
    const raw = window.localStorage.getItem(SYSTEM_STORAGE_KEY);
    const parsed = raw ? JSON.parse(raw) as Record<string, unknown> : {};
    const violet = parsed.violet as Record<string, unknown> | undefined;
    return {
      provider: normalizeVioletSummaryProvider(violet?.provider),
      triggerAMessages: positiveNumber(violet?.summaryTriggerMessages, DEFAULT_VIOLET_SUMMARY_CONFIG.triggerAMessages),
      triggerBHours: positiveNumber(violet?.summaryTriggerHours, DEFAULT_VIOLET_SUMMARY_CONFIG.triggerBHours),
      triggerBMinOutstanding: positiveNumber(
        violet?.summaryTriggerMinOutstanding,
        DEFAULT_VIOLET_SUMMARY_CONFIG.triggerBMinOutstanding,
      ),
    };
  } catch {
    return DEFAULT_VIOLET_SUMMARY_CONFIG;
  }
}

export async function loadVioletSummaryPromptTemplate(): Promise<string> {
  const result = await readSystemPrompt(
    { path: VIOLET_SUMMARY_PROMPT_PATH },
    VIOLET_SUMMARY_PROMPT_TEMPLATE,
  );
  return result.content;
}

function positiveNumber(value: unknown, fallback: number): number {
  const number = typeof value === 'number' ? value : Number(value);
  return Number.isFinite(number) && number > 0 ? number : fallback;
}
