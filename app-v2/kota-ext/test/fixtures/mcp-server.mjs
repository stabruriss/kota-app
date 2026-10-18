/** A local stdio MCP server, with no SDK/network/credentials. */
import { createInterface } from 'node:readline';
const lines=createInterface({input:process.stdin});
lines.on('line',line=>{
  let request;try{request=JSON.parse(line);}catch{return;}
  if(request.id===undefined)return;
  let result;
  switch(request.method){
    case 'initialize':result={protocolVersion:request.params.protocolVersion,capabilities:{tools:{},resources:{}},serverInfo:{name:'fixture',version:'1'}};break;
    case 'tools/list':result={tools:[{name:'echo',description:'Synthetic echo',inputSchema:{type:'object',properties:{value:{type:'string'}}}}]};break;
    case 'resources/list':result={resources:[{uri:'fixture://one',name:'Synthetic resource',mimeType:'text/plain'}]};break;
    case 'resources/templates/list':result={resourceTemplates:[]};break;
    case 'tools/call':result={content:[{type:'text',text:String(request.params.arguments?.value??'fixture')}]};break;
    case 'resources/read':result={contents:[{uri:'fixture://one',mimeType:'text/plain',text:'synthetic resource'}]};break;
    default:result={};
  }
  process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result})+'\n');
});
lines.on('close',()=>process.exit(0));
