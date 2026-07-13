import promptTemplate from '../src-tauri/prompts/magi-nl-translate.md?raw';
import { readSystemPrompt } from './pty-client';
import type { MagiProvider } from './types/smart-terminal';

export const SYSTEM_STORAGE_KEY = 'kota-v2.tavern.system-heroes';
export const MAGI_PROMPT_PATH = '$KOTA_HOME/heroes/system-magi/magi-nl-translate.md';
export const MAGI_PROMPT_TEMPLATE = promptTemplate.trimEnd();

const MAGI_HANDOFF_ARGS: Record<MagiProvider, string[]> = {
  claude: ['--dangerously-skip-permissions'],
  codex: ['--dangerously-bypass-approvals-and-sandbox'],
};

const MAGI_TRANSLATE_ARGS: Record<MagiProvider, string[]> = {
  claude: [
    '-p',
    '--output-format',
    'text',
    '--no-session-persistence',
    '--permission-mode',
    'bypassPermissions',
  ],
  codex: [
    'exec',
    '--skip-git-repo-check',
    '--sandbox',
    'read-only',
    '--ignore-rules',
    '--color',
    'never',
  ],
};

export function normalizeMagiProvider(value: unknown): MagiProvider {
  return value === 'codex' ? 'codex' : 'claude';
}

export function magiProviderOrder(provider: MagiProvider): MagiProvider[] {
  return provider === 'codex' ? ['codex', 'claude'] : ['claude', 'codex'];
}

export function magiHandoffArgs(provider: MagiProvider): string[] {
  return [...MAGI_HANDOFF_ARGS[provider]];
}

export function magiHandoffCommand(provider: MagiProvider): string {
  return [provider, ...MAGI_HANDOFF_ARGS[provider]].join(' ');
}

export function magiTranslateCommand(provider: MagiProvider): string {
  return [provider, ...MAGI_TRANSLATE_ARGS[provider], '<prompt>'].join(' ');
}

export function renderMagiPromptPreview(
  provider: MagiProvider,
  template = MAGI_PROMPT_TEMPLATE,
): string {
  return template
    .replaceAll('{{handoff_provider}}', provider)
    .replaceAll('{{ask}}', '<ask>');
}

export async function loadMagiPromptTemplate(): Promise<string> {
  const result = await readSystemPrompt(
    { path: MAGI_PROMPT_PATH },
    MAGI_PROMPT_TEMPLATE,
  );
  return result.content;
}

export function loadMagiProvider(): MagiProvider {
  if (typeof window === 'undefined') return 'claude';
  try {
    const raw = window.localStorage.getItem(SYSTEM_STORAGE_KEY);
    if (!raw) return 'claude';
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    const magi = parsed.magi as Record<string, unknown> | undefined;
    return normalizeMagiProvider(magi?.provider);
  } catch {
    return 'claude';
  }
}
