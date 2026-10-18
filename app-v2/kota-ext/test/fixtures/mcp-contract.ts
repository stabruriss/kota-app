/** Pin probe: only public APIs, independent of Kota's projected status parser. */
import { createMcpExtension, type ExtensionAPI } from '@earendil-works/pi-coding-agent';
export default async function contract(pi:ExtensionAPI) {
  let completions: ((prefix:string)=>unknown)|undefined;
  await createMcpExtension()(new Proxy(pi,{get(target,key){
    if(key==='registerCommand')return(name:string,options:Parameters<ExtensionAPI['registerCommand']>[1])=>{
      if(name==='mcp')completions=options.getArgumentCompletions;
      target.registerCommand(name,options);
    };
    return Reflect.get(target,key);
  }}));
  pi.registerCommand('test-mcp-format',{description:'Test-only native MCP contract',handler:async(_args,ctx)=>{
    const result=completions?.('reconnect ');
    if(result instanceof Promise)throw Error('Pi MCP status callback changed to async');
    ctx.ui.notify('test.mcp-format:'+JSON.stringify(result),'info');
  }});
}
