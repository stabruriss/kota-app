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
function run(name,steps,task,settings={}) {
 test(name,{timeout:30000},async t=>{
  const root=await mkdtemp(join(tmpdir(),'kota-reorder-pi-'));const p=await createFakeProvider({steps});
  t.after(async()=>{await p.close();await rm(root,{recursive:true,force:true});});
  const port=new URL(p.baseUrl).port;
  const policy=`(version 1)(allow default)(deny network*)(allow network-outbound (remote ip "localhost:${port}"))(deny file-read* file-write* (subpath "${join(homedir(),'.pi')}"))`;
  const wrapper=join(root,'sandbox.sh');
  await writeFile(wrapper,`#!/bin/sh\nexec /usr/bin/sandbox-exec -p ${quote(policy)} ${quote(resolveTestPi())} "$@"\n`,{mode:0o700});
  const session=await createTestSession({baseUrl:p.baseUrl,extensionPath,executable:wrapper,persistent:true,settings});
  try {
   await task(session.client,p);
   const seen=consumed(session.client);assert.equal(new Set(seen).size,seen.length,'Each input is consumed at most once');
   for(const request of p.requests) assert.doesNotMatch(JSON.stringify(request.body),/kota-reorder|Reorder waiting Kota inputs|kota-hold-|kota\.queue\.v1:|Pause waiting Kota inputs|Resume waiting Kota inputs/,'Control metadata never reaches the model');
   const files=await readdir(join(session.directory,'sessions'),{recursive:true});
   for(const file of files.filter(f=>f.endsWith('.jsonl'))) assert.doesNotMatch(await readFile(join(session.directory,'sessions',file),'utf8'),/kota-reorder|Reorder waiting Kota inputs|kota-hold-|kota\.queue\.v1:/,'Control metadata never enters persisted sessions');
  } finally { await session.close(); }
 });
}
const streaming={type:'long',text:'held-stream',repeat:100,chunkSize:1,holdAfterChunks:1};
run('reorder-real-pi-C-A-B-new-D-tail-and-reject-after-flush', [streaming,{type:'text',text:'queued batch reply'}], async(c,p)=>{
 await send(c,'seed');await c.waitFor(delta);
 for(const id of ['A','B','C'])await send(c,id);
 const reorder=async ids=>(await c.command('prompt',{message:'/kota-reorder '+JSON.stringify({requestId:'reorder',ids})}));
 const after=c.records.length;
 assert.equal((await reorder(ids('C','A','B'))).data.disposition,'handled');
 assert.equal(c.records.slice(after).some(r=>r.type==='extension_error'),false);
 await send(c,'D');p.release(1);
 await c.waitFor(isConsumed('D#agent-fixture'));await c.waitFor(settled,{after});
 assert.deepEqual(consumed(c),ids('seed','C','A','B','D'));
 const late=c.records.length;await reorder(ids('C','A','B'));
 await c.waitFor(r=>r.type==='extension_error'&&r.extensionPath==='command:kota-reorder'&&r.error==='kota-reorder-changed requestId=reorder',{after:late});
 await c.command('get_state');assert.deepEqual(consumed(c),ids('seed','C','A','B','D'));
});
