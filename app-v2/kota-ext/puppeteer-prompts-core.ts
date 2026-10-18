import bundled from './puppeteer-prompts.json' with { type: 'json' };

export const PROMPT_FILE = 'puppeteer-prompts.json';
export const PROMPT_INVALID = 'Puppeteer prompt file is invalid. Using bundled defaults.';
export const PROMPT_UNREADABLE = 'Could not read the Puppeteer prompt file. Using bundled defaults.';
export interface ChoiceTemplate { type: 'choice'; instructions: string; criteria: Record<string, string> }
export interface PuppeteerPrompts {
  schema: 1;
  defaultIntents?: string[];
  stringFirst: { questions: { model: ChoiceTemplate } };
  stringLater: { questions: { model: ChoiceTemplate } };
  danceContext: { questions: { target: ChoiceTemplate } };
  danceRule: { questions: { target: ChoiceTemplate } };
}
export const BUNDLED_PROMPTS = bundled as PuppeteerPrompts;
const object = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);
const keys = (value: Record<string, unknown>, expected: string[]) => Object.keys(value).length === expected.length && expected.every(k => Object.hasOwn(value, k));
const text = (value: unknown): value is string => typeof value === 'string' && !value.includes('\0') && !/[\uD800-\uDFFF]/u.test(value);

/** One schema for String's runtime and the file preview. Never accept partial templates. */
export function validPuppeteerPrompts(value: unknown): value is PuppeteerPrompts {
  if (!object(value) || value.schema !== 1 || Object.keys(value).some(k => !['schema', 'defaultIntents', 'stringFirst', 'stringLater', 'danceContext', 'danceRule'].includes(k))) return false;
  if (Object.hasOwn(value, 'defaultIntents') && (!Array.isArray(value.defaultIntents) || !value.defaultIntents.every(text))) return false;
  for (const [name, question, criterion] of [
    ['stringFirst', 'model', '<model_id>'], ['stringLater', 'model', '<model_id>'],
    ['danceContext', 'target', '<agent_id>'], ['danceRule', 'target', '<agent_id>'],
  ]) {
    const part = value[name];
    if (!object(part) || !keys(part, ['questions']) || !object(part.questions) || !keys(part.questions, [question])) return false;
    const template = part.questions[question];
    if (!object(template) || !keys(template, ['type', 'instructions', 'criteria']) || template.type !== 'choice'
      || !text(template.instructions) || !/[^\p{White_Space}]/u.test(template.instructions) || Array.from(template.instructions).length > 512
      || !object(template.criteria) || !keys(template.criteria, [criterion]) || !text(template.criteria[criterion])) return false;
  }
  return true;
}
