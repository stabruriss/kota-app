import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import user from './fixtures/puppeteer-user-prompts.json' with { type: 'json' };
import questions from './fixtures/puppeteer-user-questions.json' with { type: 'json' };
import approved from './fixtures/jev-approved-requests.json' with { type: 'json' };
import { BUNDLED_PROMPTS, PROMPT_INVALID, PROMPT_UNREADABLE, validPuppeteerPrompts } from '../puppeteer-prompts-core.ts';
import { loadPuppeteerPrompts, puppeteerPromptPath } from '../puppeteer-prompts.ts';
import { jevContext } from '../auto-route-core.ts';
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'kota-puppeteer-prompts-'));
  t.after(() => rm(root, {recursive:true,force:true}));
  const owned = join(root,'Kota/providers/kota-local');
  const path = puppeteerPromptPath(owned);
  await mkdir(join(root,'Kota/heroes/system-puppeteer'),{recursive:true});
  return {root,owned,path};
}
test('only an owned kota-local root determines the account path; missing file falls back without a write', async t => {
  const {owned,path}=await fixture(t);
  assert.equal(puppeteerPromptPath('/unrelated/root'),undefined);
  assert.equal(puppeteerPromptPath('relative/providers/kota-local'),undefined);
  assert.deepEqual(loadPuppeteerPrompts(owned),{prompts:BUNDLED_PROMPTS,path,warning:PROMPT_UNREADABLE});
  await assert.rejects(readFile(path),/ENOENT/);
});
test('valid editable file changes actual first/later assemblies, sharing Rust independent expected questions', async t => {
  const {owned,path}=await fixture(t);await writeFile(path,JSON.stringify(user));
  const loaded=loadPuppeteerPrompts(owned);
  assert.equal(loaded.warning,undefined);assert.equal(loaded.path,path);
  const models=approved[0].state.models.map(m=>({...m,intent:approved[0].questions.model.criteria[m.id]}));
  const group={models,defaultModel:'model-1',threshold:0.85};
  for(const index of [0,1]) {
    const state=approved[index].state;
    assert.deepEqual(jevContext(state.new_message,state.previous_reply,state.current_model,group,loaded.prompts),{state,questions:questions[index]});
  }
  await writeFile(path,'not JSON');
  assert.equal(loaded.prompts.stringFirst.questions.model.instructions,user.stringFirst.questions.model.instructions,'one input keeps its snapshot');
  assert.equal(loadPuppeteerPrompts(owned).warning,PROMPT_INVALID,'the next input rereads');
  assert.equal(await readFile(path,'utf8'),'not JSON');
});
test('whole-file fallback and exact warning on bad JSON, missing section/key, 513 characters or NUL', async t => {
  const {owned,path}=await fixture(t);assert.equal(validPuppeteerPrompts(BUNDLED_PROMPTS),true);
  const broken=[];
  for(const [part,q] of [['stringFirst','model'],['stringLater','model'],['danceContext','target'],['danceRule','target']]) {
    for(const instructions of ['😀'.repeat(513),'bad\0instruction','']) {
      const value=structuredClone(user);value[part].questions[q].instructions=instructions;broken.push(value);
    }
    const missing=structuredClone(user);delete missing[part];broken.push(missing);
    const key=structuredClone(user);key[part].questions[q].criteria={};broken.push(key);
  }
  for(const value of broken) {
    const raw=JSON.stringify(value);await writeFile(path,raw);
    assert.deepEqual(loadPuppeteerPrompts(owned),{prompts:BUNDLED_PROMPTS,path,warning:PROMPT_INVALID});
    assert.equal(await readFile(path,'utf8'),raw);
  }
  await writeFile(path,'not JSON');assert.equal(loadPuppeteerPrompts(owned).warning,PROMPT_INVALID);
  await writeFile(path,Buffer.from([255]));assert.equal(loadPuppeteerPrompts(owned).warning,PROMPT_UNREADABLE);
  const boundary=structuredClone(user);boundary.stringFirst.questions.model.instructions='😀'.repeat(512);
  assert.equal(validPuppeteerPrompts(boundary),true);
  boundary.stringFirst.questions.model.instructions='bad\ud800';assert.equal(validPuppeteerPrompts(boundary),false);
});
test('non-files, symlinks and oversized data fall back, without changing their targets', async t => {
  const {root,owned,path}=await fixture(t);
  const target=join(root,'other.json');await writeFile(target,JSON.stringify(user));await symlink(target,path);
  assert.equal(loadPuppeteerPrompts(owned).warning,PROMPT_INVALID);assert.deepEqual(JSON.parse(await readFile(target,'utf8')),user);
  await rm(path);await mkdir(path);assert.equal(loadPuppeteerPrompts(owned).warning,PROMPT_INVALID);
  await rm(path,{recursive:true});await writeFile(path,' '.repeat(1024*1024+1));assert.equal(loadPuppeteerPrompts(owned).warning,PROMPT_INVALID);
});
