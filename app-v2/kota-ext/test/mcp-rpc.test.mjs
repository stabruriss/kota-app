/** Real pinned Bun/Pi, fake MCP and model servers, isolated HOME and no outbound network. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { mkdtemp,mkdir,writeFile,readFile,rm,stat,chmod } from 'node:fs/promises';
import { realpathSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname,join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { resolveTestPi,TestRpcClient } from './fake-provider/rpc-client.mjs';
import { createFakeProvider } from './fake-provider/server.mjs';
import { extensionPath } from './fixtures.mjs';
const stdioServer=fileURLToPath(new URL('./fixtures/mcp-server.mjs',import.meta.url));
const contractExtension=fileURLToPath(new URL('./fixtures/mcp-contract.ts',import.meta.url));
const upgradeExtension=fileURLToPath(new URL('./fixtures/pi-upgrade.ts',import.meta.url));
const browser=fileURLToPath(new URL('./fixtures/mcp-open.mjs',import.meta.url));
const cost={input:0,output:0,cacheRead:0,cacheWrite:0};
const content=(record,prefix)=>record.type==='extension_ui_request'&&record.method==='notify'&&record.message?.startsWith(prefix)
  ? JSON.parse(record.message.slice(prefix.length)):undefined;
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));

async function fakeMcp(t){
  let base;const requests=[];
  const json=(res,status,value,headers={})=>{res.writeHead(status,{'content-type':'application/json',...headers});res.end(JSON.stringify(value));};
  const server=createServer(async(req,res)=>{
    const url=new URL(req.url,base);requests.push({path:url.pathname,method:req.method});
    if(url.pathname==='/hang')return;
    if(url.pathname.includes('/.well-known/oauth-protected-resource'))return json(res,200,{resource:`${base}/mcp`,authorization_servers:[base],scopes_supported:['fixture']});
    if(url.pathname.includes('/.well-known/oauth-authorization-server')||url.pathname.includes('/.well-known/openid-configuration'))
      return json(res,200,{issuer:base,authorization_endpoint:`${base}/authorize`,token_endpoint:`${base}/token`,registration_endpoint:`${base}/register`,response_types_supported:['code'],grant_types_supported:['authorization_code','refresh_token'],token_endpoint_auth_methods_supported:['none'],code_challenge_methods_supported:['S256']});
    if(url.pathname==='/authorize'){
      const redirect=new URL(url.searchParams.get('redirect_uri'));assert.equal(redirect.hostname,'127.0.0.1');
      redirect.searchParams.set('code','fake-mcp-code');redirect.searchParams.set('state',url.searchParams.get('state'));
      res.writeHead(302,{location:redirect.href});res.end();return;
    }
    if(url.pathname==='/token')return json(res,200,{access_token:'fake-mcp-only-token',token_type:'Bearer',expires_in:3600,refresh_token:'fake-mcp-refresh',scope:'fixture'});
    const bytes=[];for await(const chunk of req)bytes.push(chunk);const text=Buffer.concat(bytes).toString();
    let input;try{input=text?JSON.parse(text):{};}catch{input={};}
    if(url.pathname==='/register')return json(res,201,{client_id:'fake-mcp-client',redirect_uris:input.redirect_uris,token_endpoint_auth_method:'none'});
    if(url.pathname!=='/mcp')return json(res,404,{});
    if(req.headers.authorization!=='Bearer fake-mcp-only-token')return json(res,401,{error:'authentication required'},{'www-authenticate':`Bearer resource_metadata="${base}/.well-known/oauth-protected-resource"`});
    if(req.method==='GET'){res.writeHead(405);res.end();return;}
    if(req.method==='DELETE'){res.writeHead(200);res.end();return;}
    if(input.id===undefined){res.writeHead(202);res.end();return;}
    let result={};
    if(input.method==='initialize')result={protocolVersion:input.params.protocolVersion,capabilities:{tools:{}},serverInfo:{name:'oauth-fixture',version:'1'}};
    if(input.method==='tools/list')result={tools:[{name:'echo',description:'Synthetic echo',inputSchema:{type:'object',properties:{}}}]};
    json(res,200,{jsonrpc:'2.0',id:input.id,result});
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));base=`http://127.0.0.1:${server.address().port}`;
  t.after(async()=>{server.closeAllConnections();await new Promise(resolve=>server.close(resolve));});
  return {base,requests};
}

async function fixture(t,{config,helper=false,extension=extensionPath,dotenv=false,steps=[{type:'text',text:'Unexpected synthetic model request'}]}={}){
  const directory=realpathSync(await mkdtemp('/tmp/kota-mcp-rpc-'));
  const home=join(directory,'home'),agent=join(directory,'agent'),cwd=join(directory,'cwd'),sessions=join(directory,'sessions');
  for(const path of [home,agent,cwd,sessions])await mkdir(path,{mode:0o700});
  const sentinel=join(home,'.pi/agent/auth.json');await mkdir(dirname(sentinel),{recursive:true});await writeFile(sentinel,'synthetic personal sentinel\n');
  const provider=await createFakeProvider({steps});
  await writeFile(join(cwd,'fixture.txt'),'synthetic read result\n');
  if(dotenv)for(const name of ['.env','.env.local','.env.development'])
    await writeFile(join(cwd,name),'KOTA_PI_TEST_DOTENV=synthetic-must-not-be-loaded\n');
  await writeFile(join(agent,'models.json'),JSON.stringify({providers:{'kota-fake':{baseUrl:provider.baseUrl,api:'openai-completions',apiKey:'fake-model-key',models:[{id:'fixture',name:'Synthetic model',reasoning:false,input:['text'],contextWindow:16000,maxTokens:1000,cost}]}}}));
  await writeFile(join(agent,'settings.json'),JSON.stringify({defaultTools:['read','bash'],steeringMode:'all',followUpMode:'all',cacheWarming:'off',enableInstallTelemetry:false,enableAnalytics:false,compaction:{enabled:false},retry:{enabled:false}}));
  await writeFile(join(agent,'auth.json'),'{}\n');
  if(config)await writeFile(join(agent,'mcp.json'),JSON.stringify(config));
  const env={PATH:`${dirname(resolveTestPi())}:/opt/homebrew/bin:/usr/bin:/bin`,HOME:home,USERPROFILE:home,TMPDIR:directory,
    PI_CODING_AGENT_DIR:agent,PI_SKIP_VERSION_CHECK:'1',PI_OFFLINE:'1',NO_COLOR:'1'};
  if(helper)Object.assign(env,{KOTA_LOCAL_AUTH_HELPER:'1',KOTA_LOCAL_INSTALLATION_UUID:'11111111-2222-4333-8444-555555555555'});
  const product=fileURLToPath(new URL('../../../',import.meta.url));
  const policy=`(version 1)(allow default)(deny network*)(allow network-outbound (remote ip "localhost:*"))(allow network-inbound (local ip "localhost:*"))
(deny file-read* file-write* (subpath ${JSON.stringify(join(homedir(),'.pi'))}))
(deny file-read* file-write* (subpath ${JSON.stringify(join(home,'.pi'))}))
(deny file-write* (subpath ${JSON.stringify(product)}))`;
  const clients=[];
  const openSession=(persistent=false)=>{
    const client=new TestRpcClient('/usr/bin/sandbox-exec',['-p',policy,resolveTestPi(),'--mode','rpc','--no-approve','-ne','-ns','-np','--no-themes','--no-context-files','--offline','-e',extension,
      '--provider','kota-fake','--model','fixture','--thinking','off',...(persistent?['--session-id','11111111-2222-4333-8444-555555555555','--session-dir',sessions]:['--no-session'])],{cwd,env});
    clients.push(client);return client;
  };
  const client=extension?openSession(true):undefined;
  t.after(async()=>{await Promise.all(clients.map(client=>client.close()));assert.equal(await readFile(sentinel,'utf8'),'synthetic personal sentinel\n');await provider.close();await rm(directory,{recursive:true,force:true});});
  if(client)assert.equal((await client.command('get_state')).success,true);
  let serial=0;
  async function status(){
    const requestId=`test-${++serial}`;
    const pending=client.send('prompt',{message:`/kota-mcp ${JSON.stringify({schema:1,requestId})}`});
    const response=content(await client.waitFor(r=>content(r,'kota.mcp.v1:')?.requestId===requestId),'kota.mcp.v1:');
    assert.equal((await pending.response).success,true);return response;
  }
  return {directory,home,agent,cwd,sessions,env,policy,provider,client,status,openSession};
}

test('pinned native MCP completion contract is synchronous and reflects connected/connecting/auth/failed formats',{timeout:30000},async t=>{
  const mcp=await fakeMcp(t);
  const f=await fixture(t,{extension:contractExtension,config:{mcpServers:{
    stdio:{command:process.execPath,args:[stdioServer]},
    hanging:{url:`${mcp.base}/hang`},oauth:{url:`${mcp.base}/mcp`},
    missing:{command:'/nonexistent-kota-mcp-fixture'},disabled:{command:'/nonexistent-kota-mcp-fixture',enabled:false},
  }}});
  let rows;
  for(let n=0;n<100;n++){
    const after=f.client.records.length,start=performance.now();
    assert.equal((await f.client.command('prompt',{message:'/test-mcp-format'})).success,true);
    rows=content(await f.client.waitFor(r=>content(r,'test.mcp-format:')!==undefined,{after}),'test.mcp-format:');
    assert(performance.now()-start<2000,'status must not wait for a hanging connection');
    if(rows?.some(r=>r.label==='stdio'&&r.description==='connected · 1 tool · resources')&&rows.some(r=>r.label==='oauth'&&r.description==='needs sign-in')&&rows.some(r=>r.label==='missing'&&r.description.startsWith('failed: ')))break;
    await delay(50);
  }
  const byName=new Map(rows.map(r=>[r.label,r]));
  assert.equal(byName.get('stdio').value,'reconnect stdio');
  assert.equal(byName.get('stdio').description,'connected · 1 tool · resources');
  assert.equal(byName.get('hanging').description,'connecting…');
  assert.equal(byName.get('oauth').description,'needs sign-in');
  assert.match(byName.get('missing').description,/^failed: .+/);
  assert(!byName.has('disabled'));
  assert.equal(f.provider.requests.length,0);
});

test('product Local exposes native codemode with ordinary tools and executes a script without any MCP config',{timeout:30000},async t=>{
  const f=await fixture(t,{steps:[{type:'tool',name:'codemode',arguments:{code:'const results = await Promise.allSettled([tools.read({path:"fixture.txt"}), tools.read({path:"fixture.txt"})]); text("Synthetic first item"); text(results.map(result => result.status)); console.log("Synthetic console line");'}},{type:'text',text:'Synthetic codemode finished'}]});
  const status=await f.status();assert.equal(status.available,true);assert.deepEqual(status.servers,[]);
  const after=f.client.records.length;
  assert.equal((await f.client.command('prompt',{message:'Synthetic codemode acceptance.'})).success,true);
  await f.client.waitFor(r=>r.type==='agent_settled',{after});
  const result=f.client.records.slice(after).find(r=>r.type==='tool_execution_end'&&r.toolName==='codemode');
  assert(result,JSON.stringify(f.client.records.slice(after).filter(r=>r.type==='extension_error'||(r.type==='message_end'&&r.message?.role==='assistant'))));assert.equal(result.isError,false);assert(JSON.stringify(result).includes('fulfilled'));
  const output=JSON.stringify(result.result.content);
  for(const marker of ['==> text 1/2 <==','==> text 2/2 <==','<console_output>','Synthetic console line'])assert(output.includes(marker));
  const names=f.provider.requests[0].body.tools.map(tool=>tool.function?.name??tool.name);
  for(const name of ['read','bash','codemode'])assert(names.includes(name));
  assert(!JSON.stringify(f.provider.requests).includes('kota.mcp.v1:'));
  assert(!JSON.stringify(f.provider.requests).includes('Read Local MCP connection state'));
  const entries=await f.client.command('get_entries');assert(!JSON.stringify(entries.data).includes('/kota-mcp '));
  assert(!JSON.stringify(entries.data).includes('kota.mcp.v1:'));
});

test('pinned standalone ignores launch dotenv files and persists a rotated fake token after cancellation',{timeout:30000},async t=>{
  const f=await fixture(t,{extension:upgradeExtension,dotenv:true});
  const after=f.client.records.length;
  const sent=f.client.command('prompt',{message:'/test-pi-upgrade'});
  const response=await sent;
  assert.equal(response.success,true,JSON.stringify(response));
  const witness=await f.client.waitFor(r=>r.type==='extension_error'||(r.type==='extension_ui_request'&&r.method==='notify'&&r.message?.startsWith('test.pi-upgrade:')),{after});
  assert.equal(witness.type,'extension_ui_request',JSON.stringify(witness));
  assert.deepEqual(JSON.parse(witness.message.slice('test.pi-upgrade:'.length)),{
    dotenvIgnored:true,rotatedTokenSaved:true,nextRequestUsesSavedToken:true,refreshes:1,networkRequests:0,
  });
  assert.equal(f.provider.requests.length,0);
  assert.equal(await readFile(join(f.agent,'auth.json'),'utf8'),'{}\n');
});

test('product MCP health is read-only, retained after saved removal; new config is loaded only by a new session',{timeout:30000},async t=>{
  const f=await fixture(t,{config:{mcpServers:{stdio:{command:process.execPath,args:[stdioServer]}}},steps:[
    {type:'tool',name:'codemode',arguments:{code:'const result = await tools.mcp__stdio__echo({value:"synthetic-mcp-result"}); text(result.content);'}},
    {type:'text',text:'Synthetic MCP tool finished',holdBeforeHeaders:true},
  ]});
  let status;
  for(let n=0;n<100;n++){status=await f.status();if(status.servers.some(s=>s.status==='connected'))break;await delay(20);}
  assert.deepEqual(status.servers,[{name:'stdio',status:'connected'}]);
  const configPath=join(f.agent,'mcp.json'),before=await readFile(configPath,'utf8');
  await f.status();assert.equal(await readFile(configPath,'utf8'),before);
  const after=f.client.records.length;
  assert.equal((await f.client.command('prompt',{message:'Synthetic MCP tool acceptance.'})).success,true);
  await f.provider.waitForRequest(2);
  assert.equal((await f.client.command('get_state')).data.isStreaming,true);
  assert.deepEqual((await f.status()).servers,[{name:'stdio',status:'connected'}]);
  assert.equal(f.provider.requests.length,2); // Read state without injecting model input or restarting a connection.
  f.provider.release(2);
  await f.client.waitFor(r=>r.type==='agent_settled',{after});
  const tool=f.client.records.slice(after).find(r=>r.type==='tool_execution_end'&&r.toolName==='codemode');
  assert(tool);assert.equal(tool.isError,false);assert(JSON.stringify(tool).includes('synthetic-mcp-result'));
  assert(!f.provider.requests[0].body.tools.some(tool=>(tool.function?.name??tool.name)==='mcp__stdio__echo'));
  await writeFile(configPath,JSON.stringify({mcpServers:{added:{command:process.execPath,args:[stdioServer]}}}));
  assert.deepEqual((await f.status()).servers,[{name:'stdio',status:'connected'}]);
  const next=f.openSession();assert.equal((await next.command('get_state')).success,true);
  let loaded;
  for(let n=0;n<100;n++){
    const requestId=`next-${n}`,after=next.records.length;
    await next.command('prompt',{message:`/kota-mcp ${JSON.stringify({schema:1,requestId})}`});
    loaded=content(await next.waitFor(r=>content(r,'kota.mcp.v1:')?.requestId===requestId,{after}),'kota.mcp.v1:');
    if(loaded.servers.some(row=>row.status==='connected'))break;await delay(20);
  }
  assert.deepEqual(loaded.servers,[{name:'added',status:'connected'}]);
  assert.equal(f.provider.requests.length,2);
  assert(!JSON.stringify(f.provider.requests).includes('kota.mcp.v1:'));
  const entries=await f.client.command('get_entries');assert(!JSON.stringify(entries.data).includes('kota.mcp.v1:'));
});

test('auth/query helpers do not install native capabilities or connect configured MCP servers',{timeout:20000},async t=>{
  const mcp=await fakeMcp(t),f=await fixture(t,{helper:true,config:{mcpServers:{oauth:{url:`${mcp.base}/mcp`}}}});
  const commands=await f.client.command('get_commands');
  assert(!commands.data.commands.some(c=>['mcp','kota-mcp'].includes(c.name)));
  assert(commands.data.commands.some(c=>c.name==='kota-auth'));
  await delay(200);assert.equal(mcp.requests.length,0);assert.equal(f.provider.requests.length,0);
});

test('pinned CLI MCP add/list/login/logout/remove works under a model-auth lock without OAuth refresh',{timeout:45000},async t=>{
  const mcp=await fakeMcp(t),f=await fixture(t,{extension:null});
  const auth=JSON.stringify({'openai-codex':{type:'oauth',access:'fake-expired-access',refresh:'fake-expired-refresh',expires:1}})+'\n';
  await writeFile(join(f.agent,'auth.json'),auth);
  const lock=join(f.agent,'auth.json.lock');await mkdir(lock);const before=await stat(lock);
  const bin=join(f.directory,'bin');await mkdir(bin);
  const quote=text=>`'${text.replaceAll("'","'\\''")}'`;
  await writeFile(join(bin,'open'),`#!/bin/sh\nexec ${quote(process.execPath)} ${quote(browser)} "$1"\n`);await chmod(join(bin,'open'),0o700);
  const env={...f.env,PATH:`${dirname(resolveTestPi())}:${bin}:/opt/homebrew/bin:/usr/bin:/bin`};
  const timings=[];
  async function cli(args){
    const start=performance.now();
    // Resolve by PATH, as agent bash does. The pinned directory is first.
    const child=spawn('/usr/bin/sandbox-exec',['-p',f.policy,'/bin/bash','-c','exec pi "$@"','mcp-fixture',...args],{cwd:f.cwd,env,stdio:['ignore','pipe','pipe']});
    let output='',error='';child.stdout.on('data',b=>output+=b);child.stderr.on('data',b=>error+=b);
    const timer=setTimeout(()=>child.kill('SIGTERM'),12000);
    const code=await new Promise((resolve,reject)=>{child.once('error',reject);child.once('exit',resolve);});clearTimeout(timer);
    timings.push({operation:args[1],elapsedMs:Math.round(performance.now()-start)});
    assert.equal(code,0,`MCP ${args[1]} failed: ${error}`);assert.equal(await readFile(join(f.agent,'auth.json'),'utf8'),auth);
    const after=await stat(lock);assert.equal(after.ino,before.ino);assert.equal(after.mtimeMs,before.mtimeMs);
    return output;
  }
  await cli(['mcp','add','stdio','--',process.execPath,stdioServer]);
  const list=JSON.parse(await cli(['mcp','list','--json']));assert.equal(list.servers[0].state,'connected');
  await cli(['mcp','remove','stdio']);
  await cli(['mcp','add','oauth','--url',`${mcp.base}/mcp`,'--oauth-client-id','fake-mcp-client']);
  await cli(['mcp','login','oauth','--timeout','8']);
  assert((await readFile(join(f.agent,'mcp-auth.json'),'utf8')).includes('fake-mcp-only-token'));
  await cli(['mcp','logout','oauth']);
  await cli(['mcp','remove','oauth']);
  assert.equal(f.provider.requests.length,0);
  assert(mcp.requests.some(r=>r.path==='/token'));
  assert(mcp.requests.every(r=>['/mcp','/token','/authorize','/register'].includes(r.path)||r.path.includes('/.well-known/')));
  console.log('MCP management timing (synthetic only):',JSON.stringify(timings));
});
