import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createFakeProvider, MODEL, PROVIDER } from './fake-provider/server.mjs';
import { createFakeJev } from './fake-provider/jev.mjs';
import { createTestSession, TestRpcClient } from './fake-provider/rpc-client.mjs';
import { envelope, extensionPath, injectCommand } from './fixtures.mjs';
import { DEFAULT_INTENTS, JEV_INSTRUCTIONS } from '../auto-route-core.ts';
import bundledPrompts from '../puppeteer-prompts.json' with { type: 'json' };
import userPrompts from './fixtures/puppeteer-user-prompts.json' with { type: 'json' };
import { puppeteerPromptPath } from '../puppeteer-prompts.ts';
import { PROMPT_INVALID } from '../puppeteer-prompts-core.ts';

// Pinned real Pi, only loopback fakes and throwaway HOME/config/cwd. No actual model or key.
const ROUTE_CODEC = 'kota.route.v1:';
const ALTERNATE = 'fixture-alternate';
const MODEL_IDS = [MODEL, ALTERNATE, 'fixture-third', 'fixture-fourth', 'fixture-fifth', 'fixture-sixth'];
const KEY = 'fixture-jev-key-not-a-secret';
const copyProbe = fileURLToPath(new URL('./fixtures/route-copy-probe.ts', import.meta.url));
const groupOf = (models = MODEL_IDS.slice(0,2), defaultModel = 'model-1', threshold = 0.85) => ({
  models: models.map((modelId,i) => ({id:`model-${i+1}`,providerId:PROVIDER,modelId,thinkingLevel:'off',intent:DEFAULT_INTENTS[i] ?? `Fixture intent ${i+1}`})), defaultModel, threshold,
});
const setGroup = async (client, group) => assert.equal((await client.command('prompt', {message:`/kota-auto-route ${JSON.stringify(group)}`})).success,true);
const TEXT = { auth:'Jev authentication failed.', timeout:'Jev did not respond within 3 seconds.', unreachable:'Could not reach Jev.',
  empty:'No models are configured for this Auto String.', exhausted:'The selected model failed for this turn.' };
async function writeJev(directory,{keyId='key-1'}={}) {
  await mkdir(join(directory,'jev'),{recursive:true,mode:0o700});
  await writeFile(join(directory,'jev','keys.json'),JSON.stringify({schema:1,provider:'typesafe',keys:{typesafe:{key:KEY,keyId}}}),{mode:0o600});
}
async function routed(t,{steps,jev=[],group=groupOf(),settings={retry:{enabled:true,maxRetries:3,baseDelayMs:10}},persistent=false,extraExtensions=[],prompts=bundledPrompts,extraProviders={},reasoning=false}) {
  let provider,fakeJev,session,client;
  t.after(async()=>{try{await client?.close();await session?.close();}finally{await provider?.close();await fakeJev?.close();}});
  provider=await createFakeProvider({steps});fakeJev=await createFakeJev({steps:jev});
  session=await createTestSession({baseUrl:provider.baseUrl,extensionPath,extraExtensions,persistent,model:'kota/auto',modelIds:MODEL_IDS,ownedLayout:true,
    extraArgs:[`--kota-auto-route=${JSON.stringify(group)}`],extraProviders:{typesafe:{baseUrl:fakeJev.baseUrl},...extraProviders},settings,reasoning});
  await writeJev(session.directory);
  const promptPath=puppeteerPromptPath(session.directory);
  await mkdir(join(session.env.HOME,'Kota/heroes/system-puppeteer'),{recursive:true});
  await writeFile(promptPath,typeof prompts==='string'?prompts:JSON.stringify(prompts));
  client=session.client;
  const facts=(after=0)=>client.records.slice(after).filter(r=>r.type==='extension_ui_request'&&r.method==='notify'&&r.message?.startsWith(ROUTE_CODEC)).map(r=>JSON.parse(r.message.slice(ROUTE_CODEC.length)));
  const inject=async(id,content=`MARKER_${id}`)=>{
    const {id:request}=client.send('prompt',injectCommand(envelope(id,'kota.user',content)));
    assert.equal((await client.waitFor(r=>r.type==='response'&&r.id===request,{timeoutMs:20_000})).success,true);
  };
  const settle=async after=>{
    await client.waitFor(r=>r.type==='agent_settled',{after,timeoutMs:30_000});
    const assistants=client.records.slice(after).filter(r=>r.type==='message_end'&&r.message.role==='assistant').map(r=>r.message);
    return{assistants,last:assistants.at(-1),facts:facts(after),events:client.records.slice(after)};
  };
  const send=async(id,content)=>{const after=client.records.length;await inject(id,content);return settle(after);};
  const restart=async()=>{
    await client.close();client=new TestRpcClient('pi',session.args,{cwd:session.cwd,env:session.env});
    assert.equal((await client.command('get_state')).success,true);
  };
  return{session,get client(){return client;},provider,fakeJev,send,inject,settle,facts,promptPath,restart};
}
const models=provider=>provider.requests.map(r=>r.body.model);
const byEvent=(list,event)=>list.filter(f=>f.event===event);
test('first String message uses strict threshold/default and the exact final Jev schema',{timeout:40_000},async t=>{
  const g=groupOf(undefined,'model-2');
  const r=await routed(t,{group:g,steps:[{type:'text',text:'default answer'}],jev:[{serious:0.85,casual:0.15}]});
  const result=await r.send('one','Plain Kota input.');assert.equal(result.last.stopReason,'stop');assert.deepEqual(models(r.provider),[ALTERNATE]);
  const {body,authorization}=r.fakeJev.requests[0];assert.equal(authorization,`Bearer ${KEY}`);assert.equal(body.model,'jev-latest');
  assert.deepEqual(body.state,{new_message:'Plain Kota input.',models:g.models.map(({intent,...m})=>m)});
  assert.deepEqual(body.questions,{model:{type:'choice',instructions:JEV_INSTRUCTIONS.first,criteria:{'model-1':DEFAULT_INTENTS[0],'model-2':DEFAULT_INTENTS[1]}}});
  assert.deepEqual(byEvent(result.facts,'selection'),[{schema:1,event:'selection',outcome:'default',selected:{provider:PROVIDER,model:ALTERNATE,thinkingLevel:'off'},candidate:{routeModel:'model-1',provider:PROVIDER,model:MODEL,probability:.85},threshold:.85}]);
  assert.ok(result.facts.findIndex(f=>f.event==='selection')<result.facts.findIndex(f=>f.event==='decision'));
  assert.equal(byEvent(result.facts,'decision')[0].selectedModel,'model-2');assert.equal(byEvent(result.facts,'decision')[0].fallback,false);
});
test('later messages carry current_model/latest final reply and switch only strictly above the configured threshold',{timeout:40_000},async t=>{
  const long='x'.repeat(1700)+'TAIL';const r=await routed(t,{steps:[{type:'text',text:long},{type:'text',text:'second'},{type:'text',text:'third'}],
    jev:[{serious:0.9,casual:0.1},{serious:0.15,casual:0.85},{serious:0.1,casual:0.9}]});
  await r.send('one');await r.send('two');await r.send('three');assert.deepEqual(models(r.provider),[MODEL,MODEL,ALTERNATE]);
  const second=r.fakeJev.requests[1].body;assert.equal(second.questions.model.instructions,JEV_INSTRUCTIONS.later);
  assert.equal(second.state.current_model,'model-1');assert.equal(second.state.previous_reply,long.slice(-1500));
  assert.equal(r.fakeJev.requests[2].body.state.previous_reply,'second');
  const selected=byEvent(r.facts(),'selection');assert.deepEqual(selected.map(e=>e.outcome),['selected','retained','selected']);
  assert.equal(selected[1].selected.model,MODEL);assert.equal(selected[1].candidate.model,ALTERNATE);assert.equal(selected[1].candidate.probability,.85);
});
test('tool follow-ups stay on the turn model; new inserted input asks Jev again',{timeout:40_000},async t=>{
  const r=await routed(t,{steps:[{type:'tool',name:'read',arguments:{path:'fixture.txt'}},{type:'text',text:'tool done'}],jev:[{serious:0.1,casual:0.9}]});
  await r.send('one');assert.equal(byEvent(r.facts(),'selection').length,1);assert.equal(r.fakeJev.requests.length,1);assert.deepEqual(models(r.provider),[ALTERNATE,ALTERNATE]);
  const s=await routed(t,{steps:[{type:'tool',name:'read',arguments:{path:'fixture.txt'},holdBeforeHeaders:true},{type:'text',text:'inserted'}],
    jev:[{serious:0.9,casual:0.1},{serious:0.05,casual:0.95}]});
  const after=s.client.records.length;await s.inject('one');await s.provider.waitForRequest(1);await s.inject('two','Inserted while busy.');s.provider.release(1);await s.settle(after);
  assert.equal(s.fakeJev.requests[1].body.state.new_message,'Inserted while busy.');assert.deepEqual(models(s.provider),[MODEL,ALTERNATE]);
});
test('several fresh messages merge in order before the Unicode 4000-character cap',{timeout:40_000},async t=>{
  const r=await routed(t,{steps:[{type:'text',text:'first',holdBeforeHeaders:true},{type:'text',text:'batched'}],jev:[{serious:0.9,casual:0.1},{serious:0.9,casual:0.1}]});
  const after=r.client.records.length;await r.inject('one');await r.provider.waitForRequest(1);await r.inject('two','😀'.repeat(3000));await r.inject('three','b'.repeat(3000));r.provider.release(1);await r.settle(after);
  assert.equal(r.fakeJev.requests[1].body.state.new_message,`${'😀'.repeat(3000)}\n\n${'b'.repeat(998)}`);
});
test('compaction uses the latest physical reply and its summary is not classified as new input',{timeout:40_000},async t=>{
  const r=await routed(t,{steps:[{type:'text',text:'first '.repeat(400)},{type:'text',text:'second '.repeat(400)},
    {type:'text',text:'Summary of fixture.'},{type:'text',text:'Summary of fixture.'},{type:'text',text:'after'}],
    jev:[{serious:0.1,casual:0.9},{serious:0.1,casual:0.9},{serious:0.1,casual:0.9}]});
  await r.send('one','one '.repeat(300));await r.send('two','two '.repeat(300));const decisions=byEvent(r.facts(),'decision').length;assert.equal((await r.client.command('compact')).success,true);
  assert.equal(byEvent(r.facts(),'decision').length,decisions,'auxiliary compaction does not reset the displayed turn');
  assert.equal(byEvent(r.facts(),'selection').length,2);assert.equal(r.fakeJev.requests.length,2);assert.deepEqual(new Set(models(r.provider)),new Set([ALTERNATE]));
  await r.send('three','After summary.');assert.equal(r.fakeJev.requests[2].body.state.new_message,'After summary.');
});
test('a new B selection survives threshold compaction before its first response, despite previous A',{timeout:60_000},async t=>{
  const r=await routed(t,{settings:{retry:{enabled:false},compaction:{enabled:true,keepRecentTokens:0,reserveTokens:1024}},
    steps:[{type:'text',text:'A reply '.repeat(400)},...Array.from({length:8},()=>({type:'text',text:'Short B summary or reply.'}))],
    jev:[{serious:0.9,casual:0.1},{serious:0.1,casual:0.9}]});
  await r.send('one','First input.');const result=await r.send('two','Long new input. '.repeat(6000));
  assert.equal(result.last.stopReason,'stop');assert.equal(result.last.model,ALTERNATE);
  assert.ok(result.events.some(e=>e.type==='compaction_start'));
  assert.ok(models(r.provider).length>=3);assert.deepEqual(new Set(models(r.provider).slice(1)),new Set([ALTERNATE]));
  assert.equal(r.fakeJev.requests.length,2);
});
test('B overflow compaction and continuation stay on B rather than previous A',{timeout:60_000},async t=>{
  const r=await routed(t,{settings:{retry:{enabled:false},compaction:{enabled:true,keepRecentTokens:0,reserveTokens:1024}},
    steps:[{type:'text',text:'A reply '.repeat(400)},{type:'error',status:400,message:'maximum context length exceeded'},
      ...Array.from({length:8},()=>({type:'text',text:'Short B summary or reply.'}))],
    jev:[{serious:0.9,casual:0.1},{serious:0.1,casual:0.9}]});
  await r.send('one','First input.');const result=await r.send('two','New B input.');
  assert.equal(result.last.stopReason,'stop');assert.equal(result.last.model,ALTERNATE);
  assert.ok(result.events.some(e=>e.type==='compaction_start'));assert.deepEqual(new Set(models(r.provider).slice(1)),new Set([ALTERNATE]));
  assert.equal(r.fakeJev.requests.length,2);
});
test('direct compaction after restart prefers persisted B over previous successful A',{timeout:60_000},async t=>{
  const r=await routed(t,{persistent:true,settings:{retry:{enabled:false}},
    steps:[{type:'text',text:'A reply '.repeat(400)},{type:'error',status:400,message:'Quota exceeded.'},
      ...Array.from({length:8},()=>({type:'text',text:'Short B summary.'}))],
    jev:[{serious:0.9,casual:0.1},{serious:0.1,casual:0.9}]});
  await r.send('one','A input. '.repeat(300));await r.send('two','B input. '.repeat(300));await r.restart();
  assert.equal((await r.client.command('compact')).success,true);
  assert.ok(r.provider.requests.length>=3);assert.deepEqual(new Set(models(r.provider).slice(1)),new Set([ALTERNATE]));
  assert.equal(r.fakeJev.requests.length,2);
});
test('a native retry of a routing failure has no failed physical model and does not reclassify',{timeout:40_000},async t=>{
  // The missing ID deliberately contains a native retry keyword. Pi retries the virtual error,
  // so failed is absent: the callback must fail closed instead of re-entering Jev.
  const group=groupOf();group.models[0].modelId='500';
  const r=await routed(t,{group,steps:[{type:'text',text:'unused'}],jev:[{serious:0.9,casual:0.1}]});
  const result=await r.send('one');assert.equal(result.events.filter(e=>e.type==='auto_retry_start').length,1);
  assert.equal(result.last.errorMessage,TEXT.exhausted);assert.equal(r.fakeJev.requests.length,1);assert.deepEqual(models(r.provider),[]);
});
test('transient failure retries the selected model and actual clamped thinking, without a backup request',{timeout:40_000},async t=>{
  const g=groupOf();g.models[0].thinkingLevel='max';
  const r=await routed(t,{group:g,steps:[{type:'error',status:503,message:'down'},{type:'text',text:'recovered'}],jev:[{serious:0.9,casual:0.1}]});
  const result=await r.send('one');assert.deepEqual(models(r.provider),[MODEL,MODEL]);assert.equal(result.last.stopReason,'stop');
  assert.equal(result.last.thinkingLevel,'off');assert.equal(result.assistants[0].thinkingLevel,'off');
  assert.equal(r.fakeJev.requests.length,1);assert.equal(byEvent(result.facts,'model_error').length,1);
  assert.equal(byEvent(result.facts,'error').length,0);
  assert.equal(byEvent(result.facts,'selection').length,1);assert.equal(byEvent(result.facts,'selection')[0].selected.thinkingLevel,'off');
});
test('persistent Connection error exhausts native default budget on one physical model and preserves the error',{timeout:60_000},async t=>{
  const r=await routed(t,{group:groupOf(MODEL_IDS),settings:{retry:{baseDelayMs:10}},
    steps:Array.from({length:4},()=>({type:'error',status:503,drop:true})),
    jev:[{probabilities:{'model-1':0.95,'model-2':0.01,'model-3':0.01,'model-4':0.01,'model-5':0.01,'model-6':0.01}}]});
  const result=await r.send('one');assert.deepEqual(models(r.provider),[MODEL,MODEL,MODEL,MODEL]);
  assert.match(result.last.errorMessage,/Connection error/);assert.equal(byEvent(result.facts,'model_error').length,4);
  const retries=result.events.filter(e=>e.type==='auto_retry_start');assert.equal(retries.length,3);
  assert.deepEqual(retries.map(e=>e.delayMs),[10,20,40]);assert.equal(r.fakeJev.requests.length,1);
  assert.deepEqual(byEvent(result.facts,'error').at(-1),{schema:1,event:'error',code:'route_exhausted'});
  assert.equal(byEvent(result.facts,'selection').length,1);
  t.diagnostic(`Pinned Pi: ${r.provider.requests.length} requests, ${retries.length} native retries, one model; final=${result.last.errorMessage}`);
});
test('valid account instructions/criteria change classifier input, not Group metadata or physical input',{timeout:40_000},async t=>{
  const r=await routed(t,{prompts:userPrompts,persistent:true,steps:[{type:'text',text:'ok'}],jev:[{serious:0.9,casual:0.1}]});
  const result=await r.send('one');assert.equal(result.last.stopReason,'stop');
  const body=r.fakeJev.requests[0].body;
  assert.equal(body.questions.model.instructions,userPrompts.stringFirst.questions.model.instructions);
  assert.equal(body.questions.model.criteria['model-1'],`First model-1: ${DEFAULT_INTENTS[0]}`);
  assert.equal(byEvent(result.facts,'prompt_status')[0].warning,null);
  assert.equal(byEvent(result.facts,'prompt_status')[0].path,r.promptPath);
  for(const request of r.provider.requests)assert.doesNotMatch(JSON.stringify(request.body),/Fixture user file|Metadata only|puppeteer-prompts|kota\.route\.v1/);
  for(const file of (await readdir(join(r.session.directory,'sessions'),{recursive:true})).filter(f=>f.endsWith('.jsonl')))
    assert.doesNotMatch(await readFile(join(r.session.directory,'sessions',file),'utf8'),/puppeteer-prompts|kota\.route\.v1/);
});
test('a bad account file uses all bundled questions, warns privately, and is not overwritten',{timeout:40_000},async t=>{
  const r=await routed(t,{prompts:'not JSON',steps:[{type:'text',text:'ok'}],jev:[{serious:0.9,casual:0.1}]});
  const result=await r.send('one');assert.equal(result.last.stopReason,'stop');
  assert.equal(r.fakeJev.requests[0].body.questions.model.instructions,JEV_INSTRUCTIONS.first);
  assert.equal(byEvent(result.facts,'prompt_status')[0].warning,PROMPT_INVALID);
  assert.equal(await readFile(r.promptPath,'utf8'),'not JSON');
  for(const request of r.provider.requests)assert.doesNotMatch(JSON.stringify(request.body),/Using bundled defaults|puppeteer-prompts/);
});
test('editing a file during same-model retry does not reread it, but the next input does',{timeout:40_000},async t=>{
  const r=await routed(t,{prompts:userPrompts,steps:[{type:'error',status:503,message:'down',holdBeforeHeaders:true},{type:'text',text:'recovered'},{type:'text',text:'next'}],
    jev:[{serious:0.9,casual:0.1},{serious:0.1,casual:0.9}]});
  const after=r.client.records.length;await r.inject('one');await r.provider.waitForRequest(1);
  const next=structuredClone(userPrompts);next.stringLater.questions.model.instructions='Edited file for the next input.';
  await writeFile(r.promptPath,JSON.stringify(next));r.provider.release(1);await r.settle(after);
  assert.equal(r.fakeJev.requests.length,1);assert.equal(byEvent(r.facts(after),'prompt_status').length,1);
  await r.send('two');assert.equal(r.fakeJev.requests[1].body.questions.model.instructions,'Edited file for the next input.');
});
test('a non-retryable quota error is neither rewritten nor sent to a backup',{timeout:40_000},async t=>{
  const r=await routed(t,{persistent:true,steps:[{type:'error',status:400,message:'Quota exceeded for billing account.'}],jev:[{serious:0.9,casual:0.1}]});
  const result=await r.send('one');assert.equal(result.last.stopReason,'error');assert.match(result.last.errorMessage,/Quota exceeded/);
  assert.deepEqual(models(r.provider),[MODEL]);assert.equal(result.events.some(e=>e.type==='auto_retry_start'),false);
  assert.equal(byEvent(result.facts,'model_error')[0].message,result.last.errorMessage);
  assert.doesNotMatch(result.last.errorMessage,/Trying another|All configured/);
});
test('unavailable selected model fails without choosing another; the same delivery retry stays selected',{timeout:40_000},async t=>{
  const g=groupOf();g.models[0].modelId='missing';
  const r=await routed(t,{group:g,persistent:true,steps:[{type:'text',text:'unused'}],jev:[{serious:0.9,casual:0.1}]});
  const result=await r.send('one');assert.match(result.last.errorMessage,/kota-fake\/missing is unavailable/);
  assert.equal(result.events.some(e=>e.type==='auto_retry_start'),false);
  assert.equal(byEvent(result.facts,'error').filter(e=>e.kind==='string_error').length,1);
  assert.equal(byEvent(result.facts,'model_error').length,0);
  const replay=await r.send('one');assert.equal(byEvent(replay.facts,'selection').length,0);assert.equal(byEvent(replay.facts,'error').filter(e=>e.kind==='string_error').length,1);
  await setGroup(r.client,groupOf());await r.restart();await r.send('one');
  assert.equal(r.fakeJev.requests.length,1);assert.deepEqual(models(r.provider),[]);
});
test('a selected model without credentials fails, with zero requests to an available alternate',{timeout:40_000},async t=>{
  const g=groupOf();g.models[0].providerId='no-key';
  const r=await routed(t,{group:g,steps:[{type:'text',text:'unused'}],jev:[{serious:0.9,casual:0.1}],
    extraProviders:{'no-key':{api:'openai-completions',baseUrl:'http://127.0.0.1:1/v1',models:[{id:MODEL,name:'No key',reasoning:false,input:['text'],contextWindow:16384,maxTokens:2048,cost:{input:0,output:0,cacheRead:0,cacheWrite:0}}]}}});
  const result=await r.send('one');assert.match(result.last.errorMessage,/No credentials for no-key\/fixture/);
  assert.deepEqual(models(r.provider),[]);assert.equal(result.events.some(e=>e.type==='auto_retry_start'),false);
  assert.equal(byEvent(result.facts,'error')[0].kind,'string_error');assert.equal(byEvent(result.facts,'model_error').length,0);
});
test('same kotaId retry after restart reuses its snapshot; identical text with a new ID asks Jev',{timeout:60_000},async t=>{
  const r=await routed(t,{persistent:true,settings:{retry:{enabled:false}},
    steps:[{type:'error',status:400,message:'Quota exceeded.'},{type:'text',text:'replayed'},{type:'text',text:'new input'}],
    jev:[{serious:0.9,casual:0.1},{serious:0.1,casual:0.9}]});
  await r.send('one','Same text');await r.restart();await setGroup(r.client,groupOf([MODEL_IDS[2],ALTERNATE]));
  const replay=await r.send('one','Same text');assert.equal(byEvent(replay.facts,'selection').length,0);assert.deepEqual(models(r.provider),[MODEL,MODEL]);assert.equal(r.fakeJev.requests.length,1);
  await r.send('two','Same text');assert.deepEqual(models(r.provider),[MODEL,MODEL,ALTERNATE]);assert.equal(r.fakeJev.requests.length,2);
});
test('group edits keep current outlet but change next-message config; reentry starts first classification with no previous_reply',{timeout:60_000},async t=>{
  const r=await routed(t,{persistent:true,steps:[1,2,3,4].map(n=>({type:'text',text:`reply ${n}`})),jev:[1,2,3,4].map(()=>({serious:0.9,casual:0.1}))});
  await r.send('one');const next=groupOf([ALTERNATE,MODEL]);next.threshold=0.75;next.models[0].intent='Replacement intent';await setGroup(r.client,next);await r.send('two');
  assert.equal(r.fakeJev.requests[1].body.state.current_model,'model-1');assert.equal(r.fakeJev.requests[1].body.questions.model.criteria['model-1'],'Replacement intent');
  assert.deepEqual(models(r.provider),[MODEL,ALTERNATE]);
  assert.equal((await r.client.command('set_model',{provider:PROVIDER,modelId:MODEL})).success,true);
  assert.equal((await r.client.command('set_model',{provider:'kota',modelId:'auto'})).success,true);await r.send('three');
  assert.equal(r.fakeJev.requests[2].body.state.current_model,undefined);assert.equal(r.fakeJev.requests[2].body.state.previous_reply,undefined);
  assert.equal(r.fakeJev.requests[2].body.questions.model.instructions,JEV_INSTRUCTIONS.first);await r.send('four');
  assert.equal(r.fakeJev.requests[3].body.state.current_model,'model-1');
});
test('an edit during the turn does not change the selected snapshot',{timeout:40_000},async t=>{
  const r=await routed(t,{steps:[{type:'error',status:503,message:'down',holdBeforeHeaders:true},{type:'text',text:'same model'}],jev:[{serious:0.9,casual:0.1}]});
  const after=r.client.records.length;await r.inject('one');await r.provider.waitForRequest(1);await setGroup(r.client,groupOf([MODEL_IDS[2],MODEL_IDS[3]]));r.provider.release(1);await r.settle(after);
  assert.deepEqual(models(r.provider),[MODEL,MODEL]);
});
test('a malformed group command is rejected whole and leaves the saved runtime config',{timeout:30_000},async t=>{
  const r=await routed(t,{steps:[{type:'text',text:'ok'}],jev:[{serious:0.9,casual:0.1}]});const after=r.client.records.length;
  await setGroup(r.client,{...groupOf(),defaultModel:'missing'});await r.client.waitFor(e=>e.type==='extension_error'&&e.extensionPath==='command:kota-auto-route',{after});
  await r.send('one');assert.deepEqual(models(r.provider),[MODEL]);
});
test('a missing/incomplete group gives a fixed error without requests or retry',{timeout:30_000},async t=>{
  const r=await routed(t,{group:{models:[],defaultModel:'model-1',threshold:0.85},steps:[{type:'text',text:'unused'}]});
  const result=await r.send('one');assert.equal(result.last.errorMessage,TEXT.empty);assert.equal(byEvent(result.facts,'error')[0].kind,'string_error');assert.equal(byEvent(result.facts,'selection').length,0);assert.equal(r.provider.requests.length,0);assert.equal(r.fakeJev.requests.length,0);
  assert.equal(result.events.some(e=>e.type==='auto_retry_start'),false);
});
test('Jev 401 rejects only that key; replacing its keyId works',{timeout:40_000},async t=>{
  const r=await routed(t,{steps:[{type:'text',text:'new key works'}],jev:[{status:401},{serious:0.9,casual:0.1}]});
  const first=await r.send('one');assert.equal(first.last.errorMessage,TEXT.auth);assert.equal(byEvent(first.facts,'error')[0].kind,'jev_error');assert.equal(byEvent(first.facts,'selection').length,0);assert.deepEqual(JSON.parse(await readFile(join(r.session.directory,'jev','rejected-typesafe.json'),'utf8')),{schema:1,keyId:'key-1'});
  assert.equal((await r.send('two')).last.errorMessage,TEXT.auth);assert.equal(r.fakeJev.requests.length,1);assert.equal(r.provider.requests.length,0);
  await writeJev(r.session.directory,{keyId:'key-2'});assert.equal((await r.send('three')).last.stopReason,'stop');
});
test('Jev timeout/unreachable/malformed probabilities fail without provider requests or retry',{timeout:60_000},async t=>{
  for(const [step,text] of [[{hang:true},TEXT.timeout],[{drop:true},TEXT.unreachable],[{status:500},TEXT.unreachable],[{probabilities:{'model-1':2,'model-2':-1}},TEXT.unreachable]]){
    const r=await routed(t,{steps:[{type:'text',text:'unused'}],jev:[step]});const result=await r.send('one');assert.equal(result.last.errorMessage,text);
    assert.equal(r.provider.requests.length,0);assert.equal(result.events.some(e=>e.type==='auto_retry_start'),false);
    const errors=byEvent(result.facts,'error');assert.equal(errors.length,1);assert.equal(errors[0].kind,'jev_error');assert.equal(byEvent(result.facts,'selection').length,0);assert.equal('probability' in errors[0],false);
  }
});
test('route facts/Jev data/key never reach physical model input or persisted transcript',{timeout:40_000},async t=>{
  const r=await routed(t,{persistent:true,steps:[{type:'text',text:'ok'}],jev:[{serious:0.9,casual:0.1}]});await r.send('one');
  for(const request of r.provider.requests)assert.doesNotMatch(JSON.stringify(request.body),/kota\.route\.v1|new_message|current_model|kota-auto-route|fixture-jev-key/);
  for(const file of (await readdir(join(r.session.directory,'sessions'),{recursive:true})).filter(f=>f.endsWith('.jsonl')))
    assert.doesNotMatch(await readFile(join(r.session.directory,'sessions',file),'utf8'),/kota\.route\.v1|fixture-jev-key/);
});
test('bundled Pi retry classifier: five fixed errors remain non-retryable',{timeout:30_000},async t=>{
  const r=await routed(t,{steps:[{type:'text',text:'unused'}],extraExtensions:[copyProbe]});const after=r.client.records.length;
  assert.equal((await r.client.command('prompt',{message:'/route-copy-probe'})).success,true);
  const report=await r.client.waitFor(e=>e.type==='extension_ui_request'&&e.message?.startsWith('route-copy-probe:'),{after});
  const result=JSON.parse(report.message.slice('route-copy-probe:'.length));assert.equal(result.errors.length,5);
  for(const entry of result.errors)assert.equal(entry.retryable,false,entry.text);
});
async function verifier(t,jev,extraEnv={KOTA_LOCAL_AUTH_HELPER:'1'}) {
  let provider,fakeJev,session,client;t.after(async()=>{try{await client?.close();await session?.close();}finally{await provider?.close();await fakeJev?.close();}});
  provider=await createFakeProvider({steps:[{type:'text',text:'unused'}]});fakeJev=await createFakeJev({steps:jev});
  session=await createTestSession({baseUrl:provider.baseUrl,extensionPath,extraEnv,extraProviders:{typesafe:{baseUrl:fakeJev.baseUrl}}});
  client=session.client;
  const verify=async(key,operationId='verify-1')=>{
    const after=client.records.length;const {response}=client.send('prompt',{message:`/kota-jev-verify ${JSON.stringify({schema:1,operationId,providerId:'typesafe'})}`});
    const prompt=await client.waitFor(e=>e.type==='extension_ui_request'&&e.method==='input'&&e.title?.startsWith('kota.jev.v1:'),{after,timeoutMs:20_000});
    client.child.stdin.write(`${JSON.stringify({type:'extension_ui_response',id:prompt.id,value:key})}\n`);
    const result=await client.waitFor(e=>e.type==='extension_ui_request'&&e.method==='notify'&&e.message?.startsWith('kota.jev.v1:'),{after,timeoutMs:20_000});
    assert.equal((await response).success,true);return JSON.parse(result.message.slice('kota.jev.v1:'.length));
  };return{client,fakeJev,verify};
}
test('save-time verification still sends fixed ok with the key in the request only',{timeout:90_000},async t=>{
  const r=await verifier(t,[{serious:0.6,casual:0.4},{status:401},{status:500},{drop:true}]);
  assert.equal((await r.verify(KEY,'verify-ok')).outcome,'verified');assert.equal(r.fakeJev.requests[0].authorization,`Bearer ${KEY}`);
  assert.deepEqual(r.fakeJev.requests[0].body.state,{new_message:'ok'});assert.equal(r.fakeJev.requests[0].body.questions.model.instructions,JEV_INSTRUCTIONS.first);
  assert.equal((await r.verify(KEY,'verify-401')).outcome,'rejected');assert.equal((await r.verify(KEY,'verify-500')).outcome,'unavailable');assert.equal((await r.verify(KEY,'verify-drop')).outcome,'unavailable');
});
test('only the marked helper may verify Jev keys',{timeout:30_000},async t=>{
  const r=await verifier(t,[],{});const after=r.client.records.length;
  await r.client.command('prompt',{message:`/kota-jev-verify ${JSON.stringify({schema:1,operationId:'nope',providerId:'typesafe'})}`});
  const error=await r.client.waitFor(e=>e.type==='extension_error'&&e.extensionPath==='command:kota-jev-verify',{after});assert.match(error.error,/invalid-request/);assert.equal(r.fakeJev.requests.length,0);
});


test('cancelling a pending Jev request records no error or invented selection',{timeout:20_000},async t=>{
  const r=await routed(t,{steps:[{type:'text',text:'unused'}],jev:[{hang:true}]});
  const after=r.client.records.length;await r.inject('cancel-me');
  for(let n=0;n<100&&!r.fakeJev.requests.length;n++)await new Promise(resolve=>setTimeout(resolve,10));
  assert.equal(r.fakeJev.requests.length,1);
  await r.client.command('abort');await r.settle(after);
  assert.equal(byEvent(r.facts(after),'selection').length,0);assert.equal(byEvent(r.facts(after),'error').length,0);
  assert.equal(r.provider.requests.length,0);
});

test('each failed direct compaction logs a String Error without a new Jev selection',{timeout:40_000},async t=>{
  const r=await routed(t,{steps:[{type:'text',text:'First response '.repeat(400)},{type:'text',text:'Second response '.repeat(400)}],jev:[{serious:.9,casual:.1},{serious:.9,casual:.1}]});
  await r.send('one','one '.repeat(300));await r.send('two','two '.repeat(300));
  const group=groupOf();group.models[0].modelId='missing-for-compaction';await setGroup(r.client,group);
  assert.equal((await r.client.command('set_model',{provider:PROVIDER,modelId:MODEL})).success,true);
  assert.equal((await r.client.command('set_model',{provider:'kota',modelId:'auto'})).success,true);
  const after=r.client.records.length;
  for(let n=0;n<2;n++)assert.equal((await r.client.command('compact')).success,false);
  assert.equal(byEvent(r.facts(after),'error').filter(e=>e.kind==='string_error').length,2);
  assert.equal(byEvent(r.facts(after),'selection').length,0);assert.equal(byEvent(r.facts(after),'model_error').length,0);
  assert.equal(r.fakeJev.requests.length,2);assert.equal(r.provider.requests.length,2);
});
