/** Opt-in, real bundled Pi + product extension. All model traffic terminates on
 * two synthetic loopback servers; sandboxed Pi cannot read personal credentials
 * or contact any other host. No provider factory or custom stream implementation. */
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, rm, realpath } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
const directory = dirname(fileURLToPath(import.meta.url));
const runtime = process.env.KOTA_LOCAL_TEST_PI_RUNTIME_ROOT;
const productExtension = process.env.KOTA_LOCAL_TEST_EXTENSION;
assert(runtime && productExtension, 'explicit pinned bundle and product extension required');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const sse = value => `data: ${JSON.stringify(value)}\n\n`;
const event = value => `event: ${value.type}\ndata: ${JSON.stringify(value)}\n\n`;
function chunks(api, action) {
  const tool = action === 'tool', hold = action === 'hold';
  const text = hold ? ['held'] : tool ? ['alpha ', 'beta'] : ['done'];
  if (api === 'openai-completions') {
    const chunk = (delta, finish_reason = null) => sse({ id:'fixture-chat',object:'chat.completion.chunk',created:1,
      model:'fixture',choices:[{index:0,delta,finish_reason}] });
    return [chunk({role:'assistant'}),chunk({reasoning_content:'synthetic thinking'}),...text.map(content => chunk({content})),
      ...(hold ? [] : [...(tool ? [chunk({tool_calls:[{index:0,id:'fixture-call',type:'function',function:{name:'fixture_echo',arguments:'{"value":"ok"}'}}]})] : []),
        chunk({},tool ? 'tool_calls' : 'stop'),'data: [DONE]\n\n'])];
  }
  if (api === 'anthropic-messages') {
    const block = (index, content_block, delta) => [event({type:'content_block_start',index,content_block}),
      ...delta.map(delta => event({type:'content_block_delta',index,delta})),event({type:'content_block_stop',index})];
    return [event({type:'message_start',message:{id:'fixture-message',type:'message',role:'assistant',model:'fixture',content:[],stop_reason:null,stop_sequence:null,usage:{input_tokens:20,output_tokens:0}}}),
      ...block(0,{type:'thinking',thinking:'',signature:''},[{type:'thinking_delta',thinking:'synthetic thinking'},{type:'signature_delta',signature:'fixture-signature'}]),
      event({type:'content_block_start',index:1,content_block:{type:'text',text:''}}),
      ...text.map(text => event({type:'content_block_delta',index:1,delta:{type:'text_delta',text}})),
      ...(hold ? [] : [event({type:'content_block_stop',index:1}),
        ...(tool ? block(2,{type:'tool_use',id:'fixture-call',name:'fixture_echo',input:{}},[{type:'input_json_delta',partial_json:'{"value":"ok"}'}]) : []),
        event({type:'message_delta',delta:{stop_reason:tool ? 'tool_use' : 'end_turn',stop_sequence:null},usage:{output_tokens:12}}),event({type:'message_stop'})])];
  }
  const item = {id:'fixture-text',type:'message',role:'assistant',content:[]};
  const call = {id:'fixture-function',type:'function_call',call_id:'fixture-call',name:'fixture_echo',arguments:''};
  const output = [
    event({type:'response.created',response:{id:'fixture-response',object:'response',status:'in_progress',output:[]}}),
    event({type:'response.output_item.added',output_index:0,item:{id:'fixture-reasoning',type:'reasoning',summary:[]}}),
    event({type:'response.reasoning_summary_part.added',item_id:'fixture-reasoning',output_index:0,summary_index:0,part:{type:'summary_text',text:''}}),
    event({type:'response.reasoning_summary_text.delta',item_id:'fixture-reasoning',output_index:0,summary_index:0,delta:'synthetic thinking'}),
    event({type:'response.output_item.done',output_index:0,item:{id:'fixture-reasoning',type:'reasoning',summary:[{type:'summary_text',text:'synthetic thinking'}]}}),
    event({type:'response.output_item.added',output_index:1,item}),
    event({type:'response.content_part.added',item_id:item.id,output_index:1,content_index:0,part:{type:'output_text',text:'',annotations:[]}}),
    ...text.map(delta => event({type:'response.output_text.delta',item_id:item.id,output_index:1,content_index:0,delta})),
  ];
  if (hold) return output;
  output.push(event({type:'response.output_item.done',output_index:1,item:{...item,status:'completed',content:[{type:'output_text',text:text.join(''),annotations:[]}]}}));
  if (tool) output.push(event({type:'response.output_item.added',output_index:2,item:call}),
    event({type:'response.function_call_arguments.delta',item_id:call.id,output_index:2,delta:'{"value":"ok"}'}),
    event({type:'response.output_item.done',output_index:2,item:{...call,arguments:'{"value":"ok"}',status:'completed'}}));
  output.push(event({type:'response.completed',response:{id:'fixture-response',status:'completed',output:[],usage:{input_tokens:20,output_tokens:12,total_tokens:32,output_tokens_details:{reasoning_tokens:4}}}}));
  return output;
}
async function scenario(api) {
  const root = await realpath(await mkdtemp('/tmp/kota-magpie-stream-'));
  const home = join(root,'home'), agent = join(root,'agent'), workspace = join(root,'workspace');
  await Promise.all([home,agent,workspace,join(home,'.pi/agent')].map(path => mkdir(path,{recursive:true})));
  const sentinel = join(home,'.pi/agent/auth.json'); await writeFile(sentinel,'synthetic-personal-sentinel\n');
  const requests=[], records=[], faults=[], servers=[], activeResponses=new Set();
  let child, stderr='', closedHeld=false, nextId=0;
  const pending = new Map();
  const until = async (predicate, label, timeout=20000) => {
    const end=Date.now()+timeout;
    while (!predicate()) { assert(Date.now()<end, `${api}: timeout ${label}; ${stderr}; events=${JSON.stringify(records.slice(-3))}`); await delay(10); }
  };
  try {
    for (const source of ['A','B']) {
      const server=createServer(async (request,response) => {
        try {
          let bytes=''; for await (const part of request) bytes+=part;
          const body=JSON.parse(bytes); const index=requests.length;
          const key=source==='A' ? 'fixture-key-A' : 'fixture-key-B';
          requests.push({source,path:request.url,body,keyCorrect:request.headers.authorization===`Bearer ${key}` || request.headers['x-api-key']===key});
          const action=index===0 ? 'tool' : index===1 ? 'done' : 'hold';
          activeResponses.add(response); response.on('close',()=>{activeResponses.delete(response);if(action==='hold')closedHeld=true;});
          response.writeHead(200,{'Content-Type':'text/event-stream','Cache-Control':'no-cache','Connection':'close'});
          for (const part of chunks(api,action)) { if(response.destroyed)break; response.write(part); await delay(15); }
          if(action!=='hold')response.end();
        } catch(error) {faults.push(String(error));response.destroy();}
      });
      await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve)); servers.push(server);
    }
    const ports=servers.map(server=>server.address().port);
    const bases=ports.map(port=>`http://127.0.0.1:${port}`);
    const modelId=api==='anthropic-messages' ? 'upstream/claude-sonnet-4-6' : api==='openai-responses' ? 'upstream/gpt-5-fixture' : 'upstream/family/chat';
    const model={id:modelId,name:'Synthetic Magpie',api,reasoning:true,input:['text'],contextWindow:128000,maxTokens:4096,
      thinkingLevelMap:{minimal:null,low:'low',medium:'medium',high:'high',xhigh:null,max:null},
      ...(api==='anthropic-messages' ? {compat:{forceAdaptiveThinking:true}} : {})};
    const config=base=>({kotaMagpie:{mode:'local',ip:'',port:Number(new URL(base).port)},providers:{magpie:{name:'Magpie',api:'openai-completions',baseUrl:base+'/v1',
      models:[{...model,...(api==='anthropic-messages'?{baseUrl:base}:{})}]}}});
    await writeFile(join(agent,'models.json'),JSON.stringify(config(bases[0])));
    await writeFile(join(agent,'auth.json'),JSON.stringify({magpie:{type:'api_key',key:'fixture-key-A'}}));
    await writeFile(join(agent,'settings.json'),JSON.stringify({defaultThinkingLevel:'high'}));
    const policy=`(version 1)(allow default)(deny network*)
(deny file-read* file-write* (subpath ${JSON.stringify(join(homedir(),'.pi'))}))
(deny file-read* file-write* (subpath ${JSON.stringify(join(home,'.pi'))}))
(deny file-write* (subpath ${JSON.stringify(join(directory,'../../..'))}))
${ports.map(port=>`(allow network-outbound (remote ip "localhost:${port}"))`).join('\n')}`;
    const args=['-p',policy,join(runtime,'pi'),'--mode','rpc','--no-session','--no-approve','--offline','-ne','-ns','-np','--tools','fixture_echo','--no-themes','--no-context-files',
      '-e',productExtension,'-e',join(directory,'magpie-native-tool.ts'),'--provider','magpie','--model',modelId,'--thinking','high'];
    assert(!args.some(arg=>arg.includes('fixture-key-')));
    child=spawn('/usr/bin/sandbox-exec',args,{cwd:workspace,env:{PATH:'/usr/bin:/bin:/opt/homebrew/bin',HOME:home,USERPROFILE:home,TMPDIR:root,
      PI_CODING_AGENT_DIR:agent,PI_OFFLINE:'1',PI_SKIP_VERSION_CHECK:'1',NO_COLOR:'1'},stdio:['pipe','pipe','pipe']});
    child.stderr.on('data',data=>stderr+=data); let text='';
    child.stdout.on('data',data=>{
      text+=data;let at;
      while((at=text.indexOf('\n'))>=0){const line=text.slice(0,at);text=text.slice(at+1);if(!line.trim())continue;
        try {const value=JSON.parse(line);records.push(value);if(value.type==='response'&&pending.has(value.id)){pending.get(value.id)(value);pending.delete(value.id);}}
        catch(error){faults.push(String(error));}
      }
    });
    const command=async (type,fields={})=>{
      const id=`fixture-${++nextId}`;let reply;
      pending.set(id,value=>{reply=value;}); child.stdin.write(JSON.stringify({id,type,...fields})+'\n');
      await until(()=>reply,'RPC '+type); assert(reply.success,JSON.stringify(reply)); return reply;
    };
    const state=await command('get_state'); assert.equal(state.data.model.id,modelId);
    await command('prompt',{message:'Synthetic tool exercise.'});
    await until(()=>records.some(record=>record.type==='agent_end'),'first completion');
    assert.equal(requests.length,2); assert(requests.every(request=>request.source==='A'&&request.keyCorrect&&request.body.model===modelId));
    const path=api==='openai-completions'?'/v1/chat/completions':api==='openai-responses'?'/v1/responses':'/v1/messages';
    assert(requests.every(request=>new URL(request.path,bases[0]).pathname===path&&!request.path.includes('fixture-key-')),
      `${api}: ${JSON.stringify(requests.map(request=>request.path))}`);
    assert(requests[0].body.tools.some(tool=>(tool.name??tool.function?.name)==='fixture_echo'));
    assert(JSON.stringify(requests[1].body).includes('fixture-echo:ok'));
    assert(records.some(record=>record.type==='tool_execution_end'&&record.toolName==='fixture_echo'));
    assert(records.filter(record=>record.type==='message_update'&&record.assistantMessageEvent?.type==='text_delta').length>=2);
    assert(records.some(record=>record.type==='message_update'&&record.assistantMessageEvent?.type==='thinking_delta'));
    if(api==='anthropic-messages'){assert.equal(requests[0].body.thinking.type,'adaptive');assert.equal(requests[0].body.output_config.effort,'high');}
    else if(api==='openai-responses')assert.equal(requests[0].body.reasoning.effort,'high');
    else assert.equal(requests[0].body.reasoning_effort,'high');
    await command('set_thinking_level',{level:'low'});
    await writeFile(join(agent,'models.json'),JSON.stringify(config(bases[1])));
    await writeFile(join(agent,'auth.json'),JSON.stringify({magpie:{type:'api_key',key:'fixture-key-B'}}));
    await command('prompt',{message:'/kota-auth '+JSON.stringify({schema:1,operationId:'fixture-magpie-refresh',operation:'refresh'})});
    const refreshed=await command('get_state');
    assert.equal(refreshed.data.model.id,modelId); assert.equal(refreshed.data.thinkingLevel,'low');
    assert(refreshed.data.model.baseUrl.startsWith(bases[1]));
    const beforeAbort=records.length;
    const held=command('prompt',{message:'Synthetic held stream.'});
    await until(()=>requests.length===3&&records.slice(beforeAbort).some(record=>record.type==='message_update'&&record.assistantMessageEvent?.delta==='held'),'held incremental text');
    await command('abort'); await held;
    await until(()=>closedHeld,'closed upstream stream');
    assert.equal(requests[2].source,'B');assert(requests[2].keyCorrect);
    assert(records.slice(beforeAbort).some(record=>record.type==='message_end'&&record.message?.stopReason==='aborted'));
    assert.equal(await readFile(sentinel,'utf8'),'synthetic-personal-sentinel\n'); assert.deepEqual(faults,[]); assert.equal(stderr,'');
    return {api,passed:true,syntheticRequests:requests.length,textDeltas:true,thinkingDeltas:true,toolRoundTrip:true,sameIdNewTarget:true,thinkingPreserved:true,abortClosedSocket:true};
  } finally {
    if(child&&child.exitCode===null&&child.signalCode===null){
      const exited=new Promise(resolve=>child.once('exit',resolve));
      child.stdin.end();await Promise.race([exited,delay(3000)]);
      if(child.exitCode===null&&child.signalCode===null){child.kill('SIGKILL');await exited;}
    }
    assert(!child||child.exitCode!==null||child.signalCode!==null,'owned Pi child must exit before fixture removal');
    for(const response of activeResponses)response.destroy();
    for(const server of servers){server.closeAllConnections();await new Promise(resolve=>server.close(resolve));}
    await rm(root,{recursive:true,force:true});
  }
}
const results=[];
for(const api of ['openai-completions','openai-responses','anthropic-messages'])results.push(await scenario(api));
console.log(JSON.stringify({results,realUpstreamRequests:0,personalPiAccess:0,providerFactories:0}));
