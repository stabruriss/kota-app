import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import approved from './fixtures/jev-approved-requests.json' with { type: 'json' };
import prompts from '../puppeteer-prompts.json' with { type: 'json' };
import { jevContext } from '../auto-route-core.ts';

// Independent, test-only snapshots copied from the four JSON blocks in the approved
// JEV-PROMPTS.md (sha256 10a9932cef79...). Never imported from the product constants.
// Dance Context keeps the user's exact 2026-10-10 continuation sentence. Latest-reply
// objects and Rule wording follow dance-latest-reply-content-plan-20261010/PLAN.md A/B,
// approved at 2026-10-10 20:27 UTC; the original JEV-PROMPTS.md remains unchanged.
for (const [index, key, question] of [[0,'stringFirst','model'],[1,'stringLater','model'],[2,'danceContext','target'],[3,'danceRule','target']]) {
  test(`${key}: product instructions exactly match the approved independent request`, () => {
    assert.equal(prompts[key].questions[question].type, approved[index].questions[question].type);
    assert.equal(prompts[key].questions[question].instructions, approved[index].questions[question].instructions);
  });
}
test('actual first/later String assemblies exactly match approved states/questions, not just shared constants', () => {
  const models = approved[0].state.models.map(model => ({...model,intent:approved[0].questions.model.criteria[model.id]}));
  const group = {models,defaultModel:'model-1',threshold:0.85};
  for (const index of [0,1]) {
    const {model:classifier,...expected} = approved[index];
    const s = expected.state;
    assert.deepEqual(jevContext(s.new_message,s.previous_reply,s.current_model,group),expected);
  }
  assert.deepEqual(prompts.defaultIntents,Object.values(approved[0].questions.model.criteria).slice(0,2));
});
test('the bundled fallback and readonly account loader are explicit Tauri resources', async () => {
  const config = JSON.parse(await readFile(new URL('../../src-tauri/tauri.conf.json',import.meta.url),'utf8'));
  assert.equal(config.bundle.resources['../kota-ext/puppeteer-prompts.json'],'kota-ext/puppeteer-prompts.json');
  for(const file of ['puppeteer-prompts-core.ts','puppeteer-prompts.ts']) assert.equal(config.bundle.resources[`../kota-ext/${file}`],`kota-ext/${file}`);
});
