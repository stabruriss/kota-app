import { describe, expect, it } from 'vitest';
import { buildShellYaml } from '../src/chrome/TavernModal';
import { defaultAvatarIdForProvider } from '../src/lib/hero-avatars';
import { newStringRoutingDraft, stringDraftToGroup, stringGroupToDraft, stringGroupYaml, type AutoRouteGroup } from '../src/lib/string-routing';

const group = (): AutoRouteGroup => ({ threshold: .85, defaultModel: 'model-2', models: [
  { id: 'model-1', providerId: 'openrouter', modelId: 'anthropic/claude-opus-4.6:beta', thinkingLevel: 'high', intent: 'Review code: "carefully"\nThen explain 🧵.' },
  { id: 'model-2', providerId: 'ollama', modelId: 'qwen3:32b', thinkingLevel: 'off', intent: 'Light work or simple lookups.' },
] });
const shell = (provider: 'kota-local' | 'codex', model = 'kota/auto') => buildShellYaml({ name: 'Aster', provider, model, effort: 'high',
  avatarId: defaultAvatarIdForProvider(provider), skills: [], ghost: 'Fixture', shell: '', autoRoute: group() });

describe('String saved group boundary', () => {
  it('round-trips quoted nested YAML scalars with colons, slashes, quotes and newlines', () => {
    const yaml = shell('kota-local');
    expect(yaml).toContain('model: kota/auto\neffort: off\nauto-route:');
    expect(yaml).toContain('  - "--thinking"\n  - "off"');
    // JSON quoted strings are valid YAML scalars. Decode the exact emitted
    // values; Rust separately covers the full serde_yaml parser.
    const decoded: Array<Record<string, string>> = [];
    for (const line of yaml.split('\n')) {
      const item = line.match(/^    - id: (".*")$/);
      if (item) { decoded.push({ id: JSON.parse(item[1]) }); continue; }
      const value = line.match(/^      (model|effort|intent): (".*")$/);
      if (value) decoded.at(-1)![value[1]] = JSON.parse(value[2]);
    }
    const roundtrip = decoded.map(item => {
      const slash = item.model.indexOf('/');
      return { id: item.id, providerId: item.model.slice(0, slash), modelId: item.model.slice(slash + 1), thinkingLevel: item.effort, intent: item.intent };
    });
    expect(roundtrip).toEqual(group().models);
    expect(JSON.parse(yaml.match(/^  default-model: (".*")$/m)![1])).toBe(group().defaultModel);
    expect(Number(yaml.match(/^  threshold: (.*)$/m)![1])).toBe(group().threshold);
    expect(yaml).toContain(stringGroupYaml(group()));
  });
  it('retains the group with a concrete Local model and omits it for a terminal provider', () => {
    expect(shell('kota-local', 'openrouter/anthropic/claude-opus-4.6:beta')).toContain('auto-route:');
    expect(shell('codex', 'gpt-6.1-sol')).not.toContain('auto-route:');
  });
  it('renumbers ordered models and the default without exposing editor keys', () => {
    const draft = stringGroupToDraft(group());
    draft.cards = [draft.cards[1], draft.cards[0]];
    const result = stringDraftToGroup(draft)!;
    expect(result.models.map(item => item.id)).toEqual(['model-1', 'model-2']);
    expect(result.defaultModel).toBe('model-1');
    expect(result.models[0].thinkingLevel).toBe('off');
    expect(stringDraftToGroup(stringGroupToDraft(result))).toEqual(result);
  });
  it('round-trips six models and refuses a seventh', () => {
    const value = group();
    value.models = Array.from({ length: 6 }, (_, index) => ({ ...value.models[0], id: `model-${index + 1}` }));
    value.defaultModel = 'model-6';
    expect(stringDraftToGroup(stringGroupToDraft(value))).toEqual(value);
    value.models.push({ ...value.models[0], id: 'model-7' });
    expect(stringDraftToGroup(stringGroupToDraft(value))).toBeNull();
  });
  it('round-trips every 0.05 threshold step from 0.05 to 0.95 with a 0.50 default', () => {
    expect(newStringRoutingDraft().threshold).toBe(.5);
    for (let step = 1; step <= 19; step++) {
      const value = { ...group(), threshold: step / 20 };
      expect(stringDraftToGroup(stringGroupToDraft(value))).toEqual(value);
      expect(stringGroupYaml(value)).toContain(`threshold: ${value.threshold}`);
    }
    for (const threshold of [-.05, 0, .025, .049, .051, .49, .96, 1, NaN, Infinity, -Infinity]) {
      expect(stringDraftToGroup(stringGroupToDraft({ ...group(), threshold }))).toBeNull();
    }
  });
  it('never serializes incomplete cards, invalid thresholds, recursive models or oversized Unicode intents', () => {
    expect(stringDraftToGroup(newStringRoutingDraft())).toBeNull();
    for (const change of [
      (value: AutoRouteGroup) => { value.threshold = .86; },
      (value: AutoRouteGroup) => { value.models[0].intent = '🧵'.repeat(201); },
      (value: AutoRouteGroup) => { value.models[0].providerId = 'kota'; value.models[0].modelId = 'auto'; },
      (value: AutoRouteGroup) => { value.defaultModel = 'missing'; },
    ]) {
      const value = group(); change(value);
      expect(stringDraftToGroup(stringGroupToDraft(value))).toBeNull();
    }
    const value = group(); value.models[0].intent = '🧵'.repeat(200);
    expect(stringDraftToGroup(stringGroupToDraft(value))).toEqual(value);
  });
});
