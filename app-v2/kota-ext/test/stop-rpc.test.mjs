import assert from 'node:assert/strict';
import test from 'node:test';
import { writeFile, readFile, readdir, mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir, homedir } from 'node:os';
import { createFakeProvider } from './fake-provider/server.mjs';
import { createTestSession, resolveTestPi } from './fake-provider/rpc-client.mjs';
import { envelope, extensionPath, injectCommand, isConsumed } from './fixtures.mjs';
const quote=s=>"'"+s.replaceAll("'", "'\\''")+"'";
const ids=(...values)=>values.map(x=>`${x}#agent-fixture`);
const consumed=c=>c.records.filter(r=>r.type==='message_end'&&r.message.role==='custom').map(r=>r.message.details.kotaId);
const settled=r=>r.type==='agent_settled';
const delta=r=>r.type==='message_update'&&r.assistantMessageEvent.type==='text_delta';
const send=async(c,id)=>{const r=await c.command('prompt',injectCommand(envelope(id,'kota.user',`MARKER_${id}`)));assert.equal(r.success,true);return r;};
async function control(c,action,requestId) {
 const after=c.records.length;
 const response=await c.command('prompt',{message:`/kota-hold-${action} ${JSON.stringify({requestId})}`});
 assert.equal(response.success,true);assert.equal(response.data.disposition,'handled');
 const event=await c.waitFor(r=>r.type==='extension_ui_request'&&r.method==='notify'&&r.message?.startsWith('kota.queue.v1:')&&JSON.parse(r.message.slice(14)).requestId===requestId,{after});
 const receipt=JSON.parse(event.message.slice(14));assert.equal(receipt.action,action);return receipt;
}
async function abortAndClear(c) {
 const after=c.records.length;
 assert.equal((await c.command('abort')).success,true);await c.waitFor(settled,{after});
 assert.equal((await c.command('clear_queue')).success,true);
 const state=await c.command('get_state');assert.equal(state.data.isStreaming,false);return state.data;
}
function run(name,steps,task,settings={}) {
 test(name,{timeout:30000},async t=>{
  const root=await mkdtemp(join(tmpdir(),'kota-stop-pi-'));const p=await createFakeProvider({steps});
  t.after(async()=>{await p.close();await rm(root,{recursive:true,force:true});});
  const port=new URL(p.baseUrl).port;
  const policy=`(version 1)(allow default)(deny network*)(allow network-outbound (remote ip "localhost:${port}"))(deny file-read* file-write* (subpath "${join(homedir(),'.pi')}"))`;
  const wrapper=join(root,'sandbox.sh');
  await writeFile(wrapper,`#!/bin/sh\nexec /usr/bin/sandbox-exec -p ${quote(policy)} ${quote(resolveTestPi())} "$@"\n`,{mode:0o700});
  const session=await createTestSession({baseUrl:p.baseUrl,extensionPath,executable:wrapper,persistent:true,settings});
  try {
   await task(session.client,p);
   const seen=consumed(session.client);assert.equal(new Set(seen).size,seen.length,'Each input is consumed at most once');
   for(const request of p.requests) assert.doesNotMatch(JSON.stringify(request.body),/kota-hold-|kota\.queue\.v1:|Pause waiting Kota inputs|Resume waiting Kota inputs/,'Control metadata never reaches the model');
   const files=await readdir(join(session.directory,'sessions'),{recursive:true});
   for(const file of files.filter(f=>f.endsWith('.jsonl'))) assert.doesNotMatch(await readFile(join(session.directory,'sessions',file),'utf8'),/kota-hold-|kota\.queue\.v1:/,'Control metadata never enters persisted sessions');
  } finally { await session.close(); }
 });
}
const streaming={type:'long',text:'held-stream',repeat:100,chunkSize:1,holdAfterChunks:1};
run('held-a-b-new-c-tail',[streaming,{type:'text',text:'A reply',holdBeforeHeaders:true},{type:'text',text:'B C reply'}],async(c,p)=>{
 await send(c,'seed');await c.waitFor(delta);await send(c,'A');await send(c,'B');
 const pause=await control(c,'pause','stop-1');assert.deepEqual(pause.kotaIds,ids('A','B'));
 // C is accepted by the (simulated) Rust owner but its pump stays closed until resume ACK.
 const state=await abortAndClear(c);assert.deepEqual(consumed(c),ids('seed'));assert.equal(p.requests.length,1);
 const resumedAfter=c.records.length;const resume=await control(c,'resume','stop-1');assert.deepEqual(resume.kotaIds,ids('A','B'));
 await p.waitForRequest(2);await send(c,'C');p.release(2);
 await c.waitFor(isConsumed('C#agent-fixture'));await c.waitFor(settled,{after:resumedAfter});
 await c.command('get_state');assert.deepEqual(consumed(c),ids('seed','A','B','C'));
 return {pause,resume,idleAfterAbort:!state.isStreaming};
});
run('second-stop-ends-resumed-batch',[streaming,streaming],async(c,p)=>{
 await send(c,'seed');await c.waitFor(delta);await send(c,'A');await send(c,'B');
 const first=await control(c,'pause','stop-1');assert.deepEqual(first.kotaIds,ids('A','B'));
 await abortAndClear(c);const after=c.records.length;await control(c,'resume','stop-1');
 await c.waitFor(delta,{after});assert.deepEqual(consumed(c),ids('seed','A','B'));
 const second=await control(c,'pause','stop-2');assert.deepEqual(second.kotaIds,[]);
 await abortAndClear(c);const starts=c.records.filter(r=>r.type==='agent_start').length;
 const resume=await control(c,'resume','stop-2');assert.deepEqual(resume.kotaIds,[]);
 const state=await c.command('get_state');assert.equal(state.data.isStreaming,false);assert.equal(p.requests.length,2);
 assert.deepEqual(consumed(c),ids('seed','A','B'));assert.equal(c.records.filter(r=>r.type==='agent_start').length,starts);
 return {first,second,resume,requestCount:2,batch:'steeringMode=all consumes A/B before the resumed model call; second Stop ends that run without a third request'};
});
run('empty-held-does-not-start',[streaming],async(c,p)=>{
 await send(c,'seed');await c.waitFor(delta);const pause=await control(c,'pause','empty');assert.deepEqual(pause.kotaIds,[]);
 await abortAndClear(c);const starts=c.records.filter(r=>r.type==='agent_start').length;const resume=await control(c,'resume','empty');
 assert.deepEqual(resume.kotaIds,[]);const state=await c.command('get_state');assert.equal(state.data.isStreaming,false);
 assert.equal(p.requests.length,1);assert.equal(c.records.filter(r=>r.type==='agent_start').length,starts);return {pause,resume};
});
run('pause-and-inject-same-write',[streaming,{type:'text',text:'A reply'},{type:'text',text:'B reply'}],async(c,p)=>{
 await send(c,'seed');await c.waitFor(delta);
 const pauseCommand={type:'prompt',id:'same-pause',message:'/kota-hold-pause '+JSON.stringify({requestId:'same-write'})};
 const first={type:'prompt',id:'same-A',...injectCommand(envelope('A','kota.user','MARKER_A'))};
 const second={type:'prompt',id:'same-B',...injectCommand(envelope('B','kota.user','MARKER_B'))};
 c.child.stdin.write([first,pauseCommand,second].map(x=>JSON.stringify(x)).join('\n')+'\n');
 for(const id of ['same-A','same-pause','same-B'])assert.equal((await c.waitFor(r=>r.type==='response'&&r.id===id)).success,true);
 const receipt=await c.waitFor(r=>r.type==='extension_ui_request'&&r.message?.startsWith('kota.queue.v1:'));
 const pause=JSON.parse(receipt.message.slice(14));assert.deepEqual(pause.kotaIds,ids('A'));
 // Already in-flight injection B arrives after pause; it must stay held through abort.
 await abortAndClear(c);assert.deepEqual(consumed(c),ids('seed'));
 const after=c.records.length;const resume=await control(c,'resume','same-write');assert.deepEqual(resume.kotaIds,ids('A','B'));
 await c.waitFor(settled,{after});assert.deepEqual(consumed(c),ids('seed','A','B'));return {pause,resume};
});
run('compaction-abort-consumed-fact-wins',[{type:'tool',name:'read',arguments:{path:'fixture.txt'},holdAfterChunks:1},{type:'text',text:'compaction held',holdBeforeHeaders:true},{type:'text',text:'B reply'},{type:'text',text:'later reply'}],async(c,p)=>{
 await send(c,'seed');await c.waitFor(r=>r.type==='message_update');await send(c,'A');p.release(1);await p.waitForRequest(2);
 await send(c,'B');const pause=await control(c,'pause','compacting');assert.deepEqual(pause.kotaIds,ids('B'));
 await abortAndClear(c);assert.deepEqual(consumed(c),ids('seed','A'));
 const after=c.records.length;const resume=await control(c,'resume','compacting');assert.deepEqual(resume.kotaIds,ids('B'));
 await c.waitFor(isConsumed('B#agent-fixture'),{after});await c.waitFor(settled,{after});
 assert.deepEqual(consumed(c),ids('seed','A','B'));return {pause,resume,A:'already handed before pause; Pi consumes it during compaction abort before clear_queue; consumed fact must win'};
},{compaction:{enabled:true,keepRecentTokens:0,reserveTokens:16300}});
run('prior-handed-unconsumed-cleared',[streaming,{type:'text',text:'B reply'}],async(c,p)=>{
 await send(c,'seed');await c.waitFor(delta);await send(c,'A');
 const beforeAbort=c.records.length;assert.equal((await c.command('abort')).success,true);await c.waitFor(settled,{after:beforeAbort});
 assert.deepEqual(consumed(c),ids('seed'));assert.equal(p.requests.length,1);
 // A is now inside Pi's custom steer queue: it was handed by the abort turn_end.
 const pause=await control(c,'pause','residual');assert.deepEqual(pause.kotaIds,[]);
 await send(c,'B');assert.equal((await c.command('abort')).success,true);assert.equal((await c.command('clear_queue')).success,true);
 const after=c.records.length;const resume=await control(c,'resume','residual');assert.deepEqual(resume.kotaIds,ids('B'));
 await c.waitFor(settled,{after});assert.deepEqual(consumed(c),ids('seed','B'));assert.equal(p.requests.length,2);
 return {pause,resume,A:'previously handed but still unconsumed; native clear_queue removes it, no replay'};
});
