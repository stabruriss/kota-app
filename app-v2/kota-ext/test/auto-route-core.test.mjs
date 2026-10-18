import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import {
  decideModel, routingHistory, FAILED_SELECTION_ENTRY, jevContext, markJevRejected, newMessageText, parseGroup, previousReply, readJevCredential,
  JEV_INSTRUCTIONS, DEFAULT_INTENTS, DEFAULT_THRESHOLD, NEW_MESSAGE_CHARS,
} from '../auto-route-core.ts';

const user = text => ({ role: 'user', content: [{ type: 'text', text }], timestamp: 1 });
const reply = (text, stopReason = 'stop') => ({ role: 'assistant', content: [{ type: 'text', text }], stopReason });
const toolCall = () => ({ role: 'assistant', content: [{ type: 'toolCall', id: 't1', name: 'read', arguments: {} }], stopReason: 'toolUse' });
const toolResult = () => ({ role: 'toolResult', toolCallId: 't1', content: [{ type: 'text', text: 'file text' }] });
// Pi 1.0.0 convertToLlm turns these into user-role messages.
const compaction = user('The conversation history before this point was compacted into the following summary:\n\n<summary>\nold work\n</summary>');
const branch = user('The following is a summary of a branch that this conversation came back from:\n\n<summary>\nother branch</summary>');

test('new input is the user-role text after the latest response', () => {
  assert.equal(newMessageText([user('first')]), 'first');
  assert.equal(newMessageText([user('first'), reply('done'), user('<KOTA_MESSAGE …>bus</KOTA_MESSAGE>')]), '<KOTA_MESSAGE …>bus</KOTA_MESSAGE>');
  // Tool follow-ups are not new input; a message steered in after tool results is.
  assert.equal(newMessageText([user('first'), toolCall(), toolResult()]), undefined);
  assert.equal(newMessageText([user('first'), toolCall(), toolResult(), user('inserted')]), 'inserted');
  assert.equal(newMessageText([user('first'), reply('done')]), undefined);
});

test('compaction and branch summaries are never new input', () => {
  assert.equal(newMessageText([compaction]), undefined);
  assert.equal(newMessageText([compaction, reply('kept'), branch]), undefined);
  assert.equal(newMessageText([compaction, user('after summary')]), 'after summary');
  assert.equal(newMessageText([branch, user('after branch')]), 'after branch');
});

test('several new messages are joined in order and the whole is cut to 4,000 characters', () => {
  const joined = newMessageText([reply('done'), user('a'.repeat(3_000)), user('b'.repeat(3_000))]);
  assert.equal(joined, `${'a'.repeat(3_000)}\n\n${'b'.repeat(NEW_MESSAGE_CHARS - 3_002)}`);
  // Characters, not UTF-16 units: an emoji at the boundary is never split.
  const emoji = newMessageText([user('😀'.repeat(NEW_MESSAGE_CHARS + 5))]);
  assert.equal(Array.from(emoji).length, NEW_MESSAGE_CHARS);
  assert.ok(emoji.endsWith('😀'));
  // Images never reach Jev; their text siblings do.
  assert.equal(newMessageText([{ role: 'user', content: [{ type: 'image', data: 'AAAA', mimeType: 'image/png' }, { type: 'text', text: 'caption' }] }]), 'caption');
});

test('the previous reply is the latest final reply, last 1,500 characters', () => {
  assert.equal(previousReply([user('first')]), undefined);
  assert.equal(previousReply([user('a'), reply('final'), user('b'), toolCall(), toolResult(), user('c')]), 'final');
  assert.equal(previousReply([user('a'), reply('ok'), user('b'), reply('', 'error'), user('c')]), 'ok');
  const long = `${'x'.repeat(2_000)}TAIL`;
  assert.equal(previousReply([reply(long), user('next')]), long.slice(-1_500));
});

const group = () => ({ models: DEFAULT_INTENTS.map((intent, i) => ({ id: `model-${i+1}`, providerId:'fable', modelId:`model${i}`, thinkingLevel:'off', intent })), defaultModel:'model-1', threshold:0.85 });
test('Jev state and questions match the final model schema; intents only appear in criteria', () => {
  const g = group();
  const first = jevContext('hi', 'old concrete-model reply', undefined, g);
  assert.deepEqual(first.state, {new_message:'hi',models:g.models.map(({intent,...m})=>m)});
  assert.equal(first.questions.model.instructions, JEV_INSTRUCTIONS.first);
  assert.deepEqual(first.questions.model.criteria, {'model-1':DEFAULT_INTENTS[0],'model-2':DEFAULT_INTENTS[1]});
  const later = jevContext('hi', 'before', 'model-2', g);
  assert.equal(later.state.current_model,'model-2'); assert.equal(later.state.previous_reply,'before');
  assert.equal(later.questions.model.instructions,JEV_INSTRUCTIONS.later);
  assert.equal(jevContext('hi',undefined,'model-1',g).state.previous_reply,undefined);
});
test('strict per-group threshold: first defaults, later keeps current; confidence/choice are ignored', () => {
  const g = {...group(),defaultModel:'model-2'};
  const answer = p => ({choice:'model-2',confidence:1,probabilities:{'model-1':p,'model-2':1-p}});
  assert.equal(decideModel(answer(0.85),undefined,g),'model-2');
  assert.equal(decideModel(answer(0.851),undefined,g),'model-1');
  assert.equal(decideModel(answer(0.85),'model-2',g),'model-2');
  assert.equal(decideModel(answer(0.9),'model-2',g),'model-1');
  assert.equal(decideModel(answer(0.7),'model-2',{...g,threshold:0.65}),'model-1');
  assert.equal(decideModel(answer(0.6),'removed',g),'model-2');
});

test('a rejected marker applies only to the key it names, and owned files must be regular', async t => {
  const root = await mkdtemp(join(tmpdir(), 'kota-route-core-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'jev'));
  assert.equal(readJevCredential(root), undefined);
  const save = (keyId, provider = 'openrouter') => writeFile(join(root, 'jev', 'keys.json'), JSON.stringify({ schema: 1, provider, keys: { [provider]: { key: 'fixture-key', keyId } } }));
  await save('k1');
  const first = readJevCredential(root);
  assert.deepEqual(first, { provider: 'openrouter', key: 'fixture-key', keyId: 'k1' });
  markJevRejected(root, first);
  assert.deepEqual(JSON.parse(await readFile(join(root, 'jev', 'rejected-openrouter.json'), 'utf8')), { schema: 1, keyId: 'k1' });
  assert.equal(readJevCredential(root), undefined);
  await save('k2');
  assert.equal(readJevCredential(root).keyId, 'k2');
  await save('k3', 'unknown-channel');
  assert.equal(readJevCredential(root), undefined);
  await rm(join(root, 'jev', 'keys.json'));
  await writeFile(join(root, 'elsewhere.json'), '{}');
  await symlink(join(root, 'elsewhere.json'), join(root, 'jev', 'keys.json'));
  assert.throws(() => readJevCredential(root));
});

test('six complete cards and model-6 default are accepted, seven or missing model-6 are not', () => {
  const g = {...group(), defaultModel:'model-6'};
  g.models = Array.from({length:6}, (_,i) => ({...g.models[0], id:`model-${i+1}`, modelId:`family/model:${i+1}`}));
  assert.deepEqual(parseGroup(JSON.stringify(g)), g);
  assert.equal(parseGroup({...g,models:g.models.slice(0,5)}), undefined);
  assert.equal(parseGroup({...g,models:[...g.models,{...g.models[0],id:'model-7'}]}), undefined);
});

test('group thresholds accept every 0.05 step from 0.05 to 0.95, not out-of-range or off-grid values', () => {
  assert.equal(DEFAULT_THRESHOLD, 0.5);
  assert.equal(group().threshold, 0.85);
  for (let step = 1; step <= 19; step++) {
    const value = { ...group(), threshold: step / 20 };
    assert.deepEqual(parseGroup(value), value);
    assert.deepEqual(parseGroup(JSON.stringify(value)), value);
  }
  for (const threshold of [-.05, 0, .025, .049, .051, .49, .96, 1, NaN, Infinity, -Infinity]) {
    assert.equal(parseGroup({ ...group(), threshold }), undefined, String(threshold));
  }
});

test('the group from Rust is accepted only whole and well formed, without pair migration', () => {
  const g = group(); g.models[0].modelId='anthropic/family/model:beta'; g.models[0].intent='😀'.repeat(200);
  assert.deepEqual(parseGroup(JSON.stringify(g)),g);
  for (const bad of ['not json','[]','{}',{serious:null,casual:null},{...g,extra:1},{...g,defaultModel:'missing'},
    {...g,threshold:0.86},{...g,models:g.models.slice(0,1)},
    ...[['id','model-3'],['thinkingLevel','huge'],['providerId','kota'],['providerId','a/b'],['modelId',''],['intent','😀'.repeat(201)]]
      .map(([key,value])=>({...g,models:[{...g.models[0],[key]:value},g.models[1]]}))]) {
    assert.equal(parseGroup(bad),undefined,JSON.stringify(bad));
  }
});

const selection = (inputIds = ['kota:first#agent'], switchId = 'auto-boundary') => ({
  v: 3, switchId, selectedModel: 'model-1', routeModel: 'model-1', fallback: false, snapshot: group(), inputIds, thinkingLevel: 'off',
});
const switchEntry = { type: 'model_change', id: 'auto-boundary', provider: 'kota', modelId: 'auto' };
const delivery = (entryId, kotaId, text = 'identical text') => ({ type: 'custom_message', id: entryId, customType: 'kota.bus', content: text, details: { kotaId } });
const saved = state => ({ type: 'custom', customType: 'pi.virtual-model-state', data: { provider: 'kota', modelId: 'auto', state } });
test('delivery identity survives requeue and differs for identical text with a different kotaId', () => {
  const state = selection();
  const history = routingHistory([switchEntry, delivery('entry-1', 'first#agent'), saved(state),
    { type: 'message', message: reply('failed', 'error') }, delivery('new-entry', 'first#agent'), delivery('entry-3', 'second#agent')]);
  assert.equal(history.choiceFor(history.inputs[0].id), state);
  assert.equal(history.choiceFor(history.inputs[1].id), undefined);
  assert.deepEqual(history.inputs.map(i => i.text), ['identical text', 'identical text']);
});
test('a restarted direct call finds the frozen branch choice despite older replies, summaries or later group edits', () => {
  const state = selection();
  const h = routingHistory([switchEntry, delivery('entry-1', 'first#agent'), { type: 'message', message: reply('previous A') },
    saved(state), { type: 'compaction', summary: 'old context' }, { type: 'message', message: toolResult() }]);
  assert.equal(h.current, state);
  assert.deepEqual(h.inputs, []);
  assert.equal(routingHistory([switchEntry, saved(state), { ...switchEntry, id: 'reenter' }]).current, undefined);
});
test('failed resolution remembers the selection privately, without requiring a successful model request', () => {
  const state = selection();
  const h = routingHistory([switchEntry, delivery('entry-1', 'first#agent'),
    { type: 'custom', customType: FAILED_SELECTION_ENTRY, data: state }, delivery('entry-2', 'first#agent')]);
  assert.equal(h.current, state);
  assert.equal(h.choiceFor(h.inputs[0].id), state);
});
test('older fallback snapshots remain readable; neither summaries nor tools become new inputs', () => {
  const legacy = { ...selection(), v: 2, routeModel: 'model-2', fallback: true, inputIds: undefined };
  const h = routingHistory([switchEntry, saved(legacy), { type: 'compaction', summary: 'summary' },
    { type: 'message', message: toolResult() }, { type: 'branch_summary', summary: 'summary' }]);
  assert.equal(h.current, legacy);
  assert.deepEqual(h.inputs, []);
});
