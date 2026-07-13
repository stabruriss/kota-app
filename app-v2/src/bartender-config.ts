import syncConflictPromptTemplate from '../src-tauri/prompts/bartender-sync-conflict.md?raw';
import pullConflictPromptTemplate from '../src-tauri/prompts/bartender-pull-conflict.md?raw';
import { readSystemPrompt } from './pty-client';

export const TAVERN_SYSTEM_CONFIG_CHANGED_EVENT = 'kota-v2:tavern-profile-changed';

export const BARTENDER_SYNC_CONFLICT_PROMPT_PATH = '$KOTA_HOME/heroes/system-bartender/bartender-sync-conflict.md';
export const BARTENDER_PULL_CONFLICT_PROMPT_PATH = '$KOTA_HOME/heroes/system-bartender/bartender-pull-conflict.md';

export const BARTENDER_SYNC_CONFLICT_PROMPT = syncConflictPromptTemplate.trimEnd();
export const BARTENDER_PULL_CONFLICT_PROMPT = pullConflictPromptTemplate.trimEnd();

export interface BartenderConflictPrompts {
  conflictPrompt: string;
  pullConflictPrompt: string;
}

export function loadBartenderConflictPrompts(): BartenderConflictPrompts {
  return {
    conflictPrompt: BARTENDER_SYNC_CONFLICT_PROMPT,
    pullConflictPrompt: BARTENDER_PULL_CONFLICT_PROMPT,
  };
}

export async function loadBartenderFactoryConflictPrompts(): Promise<BartenderConflictPrompts> {
  const [conflictPrompt, pullConflictPrompt] = await Promise.all([
    readSystemPrompt(
      { path: BARTENDER_SYNC_CONFLICT_PROMPT_PATH },
      BARTENDER_SYNC_CONFLICT_PROMPT,
    ).then((result) => result.content),
    readSystemPrompt(
      { path: BARTENDER_PULL_CONFLICT_PROMPT_PATH },
      BARTENDER_PULL_CONFLICT_PROMPT,
    ).then((result) => result.content),
  ]);
  return { conflictPrompt, pullConflictPrompt };
}

export async function loadBartenderConflictPromptsFromFiles(): Promise<BartenderConflictPrompts> {
  return loadBartenderFactoryConflictPrompts();
}
