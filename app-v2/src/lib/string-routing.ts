import { localConfigurationSaveFailure } from './local-configuration';
import { invoke } from '@tauri-apps/api/core';

/** IPC-CONTRACT v5.1. Editor keys never cross the backend boundary. */
export interface AutoRouteModel {
  id: string; providerId: string; modelId: string; thinkingLevel: string; intent: string;
}
export interface AutoRouteGroup { models: AutoRouteModel[]; defaultModel: string; threshold: number }
export interface StringTemplate { id: string; name: string; savedAt: string; group: AutoRouteGroup }
export interface StringTemplateLibrary { schema: 1; revision: string; templates: StringTemplate[] }
export const readStringTemplates = () => invoke<StringTemplateLibrary>('puppeteer_string_templates_read');
export const saveStringTemplates = (library: StringTemplateLibrary) =>
  invoke<StringTemplateLibrary>('puppeteer_string_templates_save', { library });
/** Convenience for the ASCII default names only; the backend owns name equality. */
export function nextStringTemplateName(templates: readonly StringTemplate[]): string {
  const names = new Set(templates.map(item => item.name.normalize('NFC').trim().toLowerCase()));
  let number = 1;
  while (names.has(`string ${number}`)) number++;
  return `String ${number}`;
}
export type StringRouteEvent = { id: number; occurredAt?: string } & (
  | { kind: 'selection'; selected: { provider: string; model: string; thinkingLevel: string };
      candidate: { routeModel: string; provider: string; model: string; probability: number };
      /** manual: the composer chose this model; candidate is that choice and probability carries no Jev meaning. */
      threshold: number; outcome: 'selected' | 'retained' | 'default' | 'manual' }
  | { kind: 'model_error'; provider: string; model: string; message: string }
  | { kind: 'jev_error'; code: string; message: string }
  | { kind: 'string_error'; code: string; message: string; provider?: string; model?: string }
);
export interface LocalAutoRouteView {
  currentModel: string | null;
  turn: { selectedModel: string; routeModel: string; fallback: boolean; provider: string; model: string; thinkingLevel: string } | null;
  events: StringRouteEvent[];
  error: { code: 'jev_auth' | 'jev_timeout' | 'jev_unreachable' | 'route_empty' | 'route_exhausted'; eventId?: number | null } | null;
}

/** Local editor state; incomplete cards cannot be serialized as a group. */
export interface StringModelCard {
  key: string; model: { providerId: string; modelId: string } | null; thinking: string | null; intent: string;
}
export interface StringRoutingDraft { cards: StringModelCard[]; defaultKey: string; threshold: number }
export const AUTO_ROUTE_MODEL = 'kota/auto';
export const STRING_DEFAULT_INTENTS = [
  'Complex work where quality matters, decisions, or work where a mistake is costly.',
  'Light work or simple lookups.',
] as const;
export const STRING_MAX_MODELS = 6;
export const STRING_INCOMPLETE_COPY = 'Configure 2–6 models, an intent for each, and a default model.';
export function newStringRoutingDraft(): StringRoutingDraft {
  return { threshold: .5, defaultKey: 'card-1', cards: STRING_DEFAULT_INTENTS.map((intent, index) => ({
    key: `card-${index + 1}`, model: null, thinking: null, intent,
  })) };
}
export function stringDraftIssue(draft: StringRoutingDraft): string | null {
  if (draft.cards.length < 2) return 'Add at least 2 models.';
  if (draft.cards.length > STRING_MAX_MODELS) return 'Keep at most 6 models.';
  for (const [index, card] of draft.cards.entries()) {
    if (!card.model?.providerId || !card.model.modelId) return `Choose a model for Model ${index + 1}.`;
    if (`${card.model.providerId}/${card.model.modelId}` === AUTO_ROUTE_MODEL) return `Choose a specific model for Model ${index + 1}.`;
    if (!card.thinking) return `Choose thinking for Model ${index + 1}.`;
    if (!card.intent.trim()) return `Add an intent description for Model ${index + 1}.`;
    if ([...card.intent].length > 200) return `Keep Model ${index + 1}'s intent within 200 characters.`;
  }
  if (!draft.cards.some(card => card.key === draft.defaultKey)) return 'Choose a default model.';
  if (new Set(draft.cards.map(card => card.key)).size !== draft.cards.length) return STRING_INCOMPLETE_COPY;
  if (!Number.isFinite(draft.threshold) || draft.threshold < .05 || draft.threshold > .95
    || Math.abs(draft.threshold * 20 - Math.round(draft.threshold * 20)) >= 1e-8) return 'Choose a valid probability threshold.';
  return null;
}
export function stringDraftComplete(draft: StringRoutingDraft): boolean {
  return stringDraftIssue(draft) === null;
}
export function stringDraftToGroup(draft: StringRoutingDraft): AutoRouteGroup | null {
  if (!stringDraftComplete(draft)) return null;
  return {
    threshold: draft.threshold,
    defaultModel: `model-${draft.cards.findIndex(card => card.key === draft.defaultKey) + 1}`,
    models: draft.cards.map((card, index) => ({ id: `model-${index + 1}`, providerId: card.model!.providerId,
      modelId: card.model!.modelId, thinkingLevel: card.thinking!, intent: card.intent })),
  };
}
export function stringGroupToDraft(group?: AutoRouteGroup | null): StringRoutingDraft {
  if (!group) return newStringRoutingDraft();
  return { threshold: group.threshold, defaultKey: group.defaultModel, cards: group.models.map(item => ({
    key: item.id, model: { providerId: item.providerId, modelId: item.modelId }, thinking: item.thinkingLevel, intent: item.intent,
  })) };
}
export function stringGroupYaml(group: AutoRouteGroup): string {
  return ['auto-route:', `  threshold: ${group.threshold}`, `  default-model: ${JSON.stringify(group.defaultModel)}`, '  models:',
    ...group.models.flatMap(item => [
      `    - id: ${JSON.stringify(item.id)}`,
      `      model: ${JSON.stringify(`${item.providerId}/${item.modelId}`)}`,
      `      effort: ${JSON.stringify(item.thinkingLevel)}`,
      `      intent: ${JSON.stringify(item.intent)}`,
    ]),
  ].join('\n');
}
export const AUTO_ROUTE_RESPONSE_COPY = {
  jev_auth: 'Jev authentication failed. Update the key in Puppeteer.',
  jev_timeout: 'Jev did not respond within 3 seconds.',
  jev_unreachable: 'Jev could not be reached.',
  route_empty: 'No models are configured for this Auto String.',
  route_exhausted: 'The selected model failed for this turn.',
} as const;
export function stringErrorText(error: unknown): string {
  const configurationFailure = localConfigurationSaveFailure(error);
  if (configurationFailure) return configurationFailure.message;
  const code = typeof error === 'string' ? error : (error as { code?: string } | null)?.code;
  if (code && code in AUTO_ROUTE_RESPONSE_COPY) return AUTO_ROUTE_RESPONSE_COPY[code as keyof typeof AUTO_ROUTE_RESPONSE_COPY];
  switch (code) {
    case 'local_config_changed': return 'Settings changed outside this editor. Reload before saving.';
    case 'auto_route_models_required': return STRING_INCOMPLETE_COPY;
    case 'auto_route_invalid': return 'Auto String settings are incomplete or invalid.';
    case 'jev_key_required': return 'Set up Jev to use Auto String';
    case 'jev_key_rejected': return 'Jev key rejected. Update it in Puppeteer.';
    case 'provider_not_signed_in': return 'This provider is not signed in. Sign in or choose another model.';
    case 'credential_invalid': return 'This provider needs to sign in again.';
    case 'model_not_found': return 'This model is unavailable. Choose another model.';
    case 'thinking_invalid': return 'Choose a supported thinking level for this model.';
    default: return 'Could not save Auto String settings. Try again.';
  }
}
