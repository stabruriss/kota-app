import assert from 'node:assert/strict';
import test from 'node:test';
import { writeFile, mkdir, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createFakeProvider } from './fake-provider/server.mjs';
import { createTestSession, TestRpcClient } from './fake-provider/rpc-client.mjs';
import { envelope, extensionPath, injectCommand, isConsumed } from './fixtures.mjs';
const root = fileURLToPath(new URL(`./.tmp/queue-${process.pid}/`, import.meta.url));
await mkdir(root,{recursive:true});
const bridgePath = fileURLToPath(new URL('../bridge.ts', import.meta.url));
const settled = r => r.type === 'agent_settled';
const delta = r => r.type === 'message_update' && r.assistantMessageEvent.type === 'text_delta';
async function fixture(t,steps,options={}) {
 const provider = await createFakeProvider({steps});
 const session = await createTestSession({baseUrl:provider.baseUrl,extensionPath,persistent:true,...options});
 t.after(async()=>{await session.close();await provider.close();});
 return {provider,client:session.client,session};
}
const send = (c,i) => c.command('prompt',injectCommand(i));
const cancel = (c,i) => c.command('prompt',{message:`/kota-cancel ${i.kotaId}`});
const error = c => r => r.type==='extension_error' && r.extensionPath===`command:${c}`;
function assertNoDescriptions(provider) {
 for(const r of provider.requests) {
   const body=JSON.stringify(r.body);
   for(const text of ['Inject an existing Kota input.','Remove one waiting Kota input.','Discard waiting Kota inputs.']) assert(!body.includes(text));
 }
}

test('(a) synchronous turn_end flush batches two inputs into the very next request', {timeout:20000}, async t=>{
 const {provider,client}=await fixture(t,[{type:'tool',name:'read',arguments:{path:'fixture.txt'},holdAfterChunks:1},{type:'text',text:'batch reply'}]);
 const a=envelope('a-seed'),b=envelope('a-b','kota.user','B_BATCH_MARKER'),c=envelope('a-c','kota.user','C_BATCH_MARKER');
 await send(client,a);await client.waitFor(r=>r.type==='message_update');await send(client,b);await send(client,c);
 assert(!client.records.some(isConsumed(b.kotaId)));provider.release(1);await client.waitFor(settled);
 assert.equal(provider.requests.length,2);assert.equal(client.records.filter(isConsumed(b.kotaId)).length,1);assert.equal(client.records.filter(isConsumed(c.kotaId)).length,1);
 const request=JSON.stringify(provider.requests[1].body);assert(request.includes('B_BATCH_MARKER'));assert(request.includes('C_BATCH_MARKER'));assertNoDescriptions(provider);
 const commands=await client.command('get_commands');
 assert(commands.data.commands.some(command=>command.name==='kota-cancel'&&command.description==='Remove one waiting Kota input.'));
});

test('(a2) before_settle flush continues after a final answer', {timeout:20000}, async t=>{
 const late=envelope('before-settle');
 const wrapper=join(root,'before-settle.ts');
 t.after(()=>rm(root,{recursive:true,force:true}));
 await writeFile(wrapper,`import { registerInjection } from ${JSON.stringify(bridgePath)};
export default function(pi) {
 const commands=new Map();
 registerInjection({...pi,registerCommand(name,def){commands.set(name,def);pi.registerCommand(name,def);}});
 let once=false;pi.on('turn_end',()=>{if(!once){once=true;commands.get('kota-inject').handler(${JSON.stringify(JSON.stringify(late))},{isIdle:()=>false});}});
}`);
 const {provider,client}=await fixture(t,[{type:'text',text:'first final'},{type:'text',text:'next final'}],{extensionPath:wrapper});
 await send(client,envelope('before-seed'));await client.waitFor(settled);
 assert.equal(provider.requests.length,2);assert.equal(client.records.filter(isConsumed(late.kotaId)).length,1);
});

test('(b) compaction accepts into held and a later boundary consumes it without refusal', {timeout:25000}, async t=>{
 const {provider,client}=await fixture(t,[
 {type:'tool',name:'read',arguments:{path:'fixture.txt'},holdAfterChunks:1},
 {type:'text',text:'short compacted summary',holdBeforeHeaders:true},
 {type:'text',text:'first steering reply'},
 {type:'text',text:'second short summary'},
 {type:'text',text:'second steering reply'}],{settings:{compaction:{enabled:true,keepRecentTokens:0,reserveTokens:16300}}});
 await send(client,envelope('compact-seed'));await client.waitFor(r=>r.type==='message_update');await send(client,envelope('compact-a'));
 provider.release(1);await provider.waitForRequest(2);const b=envelope('compact-b');await send(client,b);
 assert(!client.records.some(isConsumed(b.kotaId)));assert(!client.records.some(r=>r.type==='extension_error'));
 provider.release(2);await client.waitFor(isConsumed(b.kotaId));await client.waitFor(settled);
 assertNoDescriptions(provider);
});

test('(c) cancel executes during a long tool and precisely beats the hook handoff', {timeout:20000}, async t=>{
 const {client,provider}=await fixture(t,[{type:'tool',name:'bash',arguments:{command:'sleep 0.5; printf fixture'},chunkSize:128},{type:'text',text:'tool final'}]);
 await send(client,envelope('cancel-seed'));await client.waitFor(r=>r.type==='tool_execution_start');
 const b=envelope('cancel-b');await send(client,b);const start=performance.now();await cancel(client,b);assert(performance.now()-start<1000);
 await client.waitFor(settled);assert(!client.records.some(isConsumed(b.kotaId)));
 const after=client.records.length;await cancel(client,b);const e=await client.waitFor(error('kota-cancel'),{after});
 assert.equal(e.error,`kota-cancel-too-late kotaId=${encodeURIComponent(b.kotaId)}`);assert.equal(provider.requests.length,2);
});

test('(d) abort/inject/cancel in one stdin write respond within one second', {timeout:20000}, async t=>{
 const {client}=await fixture(t,[{type:'long',text:'x',repeat:300,chunkSize:1,holdAfterChunks:1},{type:'text',text:'next',holdBeforeHeaders:true}]);
 await send(client,envelope('window-seed'));await client.waitFor(delta);
 const b=envelope('window-b');await send(client,b);
 const ids=['window-abort','window-inject','window-cancel'];
 const waits=ids.map(id=>client.waitFor(r=>r.type==='response'&&r.id===id,{timeoutMs:1000}));
 const start=performance.now();client.child.stdin.write([
 {id:ids[0],type:'abort'}, {id:ids[1],type:'prompt',...injectCommand(envelope('window-c'))}, {id:ids[2],type:'prompt',message:`/kota-cancel ${b.kotaId}`}
 ].map(r=>JSON.stringify(r)).join('\n')+'\n');
 const responses=await Promise.all(waits);assert(responses.every(r=>r.success));assert(performance.now()-start<1000);
});

test('(e) same-write idle injections start once and preserve one batch/order', {timeout:20000}, async t=>{
 const {client,provider}=await fixture(t,[{type:'text',text:'first',holdBeforeHeaders:true},{type:'text',text:'batch final'}]);
 const inputs=[envelope('same-a'),envelope('same-b'),envelope('same-c')];
 const ids=inputs.map((_,i)=>`same-${i}`);const waits=ids.map(id=>client.waitFor(r=>r.type==='response'&&r.id===id));
 client.child.stdin.write(inputs.map((i,n)=>JSON.stringify({id:ids[n],type:'prompt',...injectCommand(i)})).join('\n')+'\n');await Promise.all(waits);
 await provider.waitForRequest(1);assert.equal(provider.requests.length,1);assert(!client.records.some(isConsumed(inputs[1].kotaId)));
 provider.release(1);await client.waitFor(settled);assert.equal(provider.requests.length,2);
 for(const input of inputs)assert.equal(client.records.filter(isConsumed(input.kotaId)).length,1);
});

test('(f) clear-held then abort and clear_queue never sends the stopped batch', {timeout:20000}, async t=>{
 const {client,provider}=await fixture(t,[{type:'long',text:'x',repeat:300,chunkSize:1,holdAfterChunks:1}]);
 await send(client,envelope('stop-seed'));await client.waitFor(delta);const b=envelope('stop-b');await send(client,b);
 await client.command('prompt',{message:'/kota-clear-held'});await client.command('abort');await client.command('clear_queue');await client.waitFor(settled);
 assert(!client.records.some(isConsumed(b.kotaId)));assert.equal(provider.requests.length,1);
});

test('(g) nonretryable model error still flushes held and completes a next request', {timeout:20000}, async t=>{
 const {client,provider}=await fixture(t,[{type:'error',status:400,holdBeforeHeaders:true},{type:'text',text:'recovered queued task'}]);
 await send(client,envelope('error-seed'));await provider.waitForRequest(1);const b=envelope('error-b');await send(client,b);provider.release(1);
 await client.waitFor(isConsumed(b.kotaId));await client.waitFor(r=>r.type==='message_end'&&r.message.role==='assistant'&&r.message.stopReason==='stop');await client.waitFor(settled);
 assert.equal(provider.requests.length,2);assert(!client.records.some(r=>r.type==='extension_error'));assertNoDescriptions(provider);
});


test('(b2) soft clearing a drained input does not suppress its later native consumption', {timeout:25000}, async t=>{
 const {provider,client}=await fixture(t,[
 {type:'tool',name:'read',arguments:{path:'fixture.txt'},holdAfterChunks:1},
 {type:'text',text:'summary',holdBeforeHeaders:true},
 {type:'text',text:'queued reply'}],{settings:{compaction:{enabled:true,keepRecentTokens:0,reserveTokens:16300}}});
 await send(client,envelope('late-seed'));await client.waitFor(r=>r.type==='message_update');const b=envelope('late-b','kota.bus');await send(client,b);
 provider.release(1);await provider.waitForRequest(2);assert(!client.records.some(isConsumed(b.kotaId)));
 await client.command('prompt',{message:'/kota-clear-held'});provider.release(2);
 await client.waitFor(isConsumed(b.kotaId));await client.waitFor(settled);assert.equal(client.records.filter(isConsumed(b.kotaId)).length,1);
});

test('(f2) restarting the same session drops held, retains completed native history, and accepts new input', {timeout:20000}, async t=>{
 const {provider,client,session}=await fixture(t,[{type:'text',text:'stopped run',holdBeforeHeaders:true},{type:'text',text:'new input'}]);
 await send(client,envelope('restart-seed'));await provider.waitForRequest(1);const old=envelope('restart-old');await send(client,old);
 await client.close();const fresh=new TestRpcClient('pi',session.args,{cwd:session.cwd,env:session.env});t.after(()=>fresh.close());
 await fresh.command('get_state');const entries=await fresh.command('get_entries');
 assert(!JSON.stringify(entries.data.entries).includes(old.kotaId));
 const current=envelope('restart-new');await send(fresh,current);await fresh.waitFor(isConsumed(current.kotaId));await fresh.waitFor(settled);
 assert(!fresh.records.some(isConsumed(old.kotaId)));assert.equal(provider.requests.length,2);
});
