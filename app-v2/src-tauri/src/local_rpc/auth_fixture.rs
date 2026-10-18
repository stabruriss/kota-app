//! A JSONL transport fixture, not native Pi/auth evidence. It has no network
//! code and reads only its own temporary files. The bundled-Pi gate is separate.
use super::{
    auth::Backend,
    bundle::BundledPi,
    owned::{write_json, OwnedDirectory},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

pub(crate) struct Fixture {
    pub backend: Backend,
    root: PathBuf,
}
impl Fixture {
    pub fn new(mut plan: Value) -> Self {
        let root =
            std::env::temp_dir().join(format!("local-auth-fixture-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let executable = root.join("rpc-fixture");
        fs::write(&executable, CHILD).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        }
        if plan.get("providers").is_none() {
            plan["providers"] = providers();
        }
        fs::write(
            root.join("rpc-fixture.json"),
            serde_json::to_vec(&plan).unwrap(),
        )
        .unwrap();
        let owned = OwnedDirectory::new(&root.join("kota-home"));
        owned.initialize().unwrap();
        let extension = root.join("index.ts");
        fs::write(&extension, "// transport fixture").unwrap();
        Self {
            backend: Backend {
                owned,
                bundled: BundledPi {
                    executable,
                    extension,
                },
            },
            root,
        }
    }
    pub fn store(&self, value: Value) {
        write_json(&self.backend.owned.auth_path(), &value, "fixture").unwrap();
    }
    pub fn events(&self) -> Vec<Value> {
        fs::read_to_string(self.root.join("events.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    pub fn requests(&self, operation: &str) -> usize {
        self.events()
            .iter()
            .filter(|v| v["operation"] == operation)
            .count()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
pub(crate) fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "auth fixture timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}
pub(super) fn providers() -> Value {
    json!([
        {"providerId":"fable","providerLabel":"Fixture","canApiKey":true,"canOAuth":true,
         "models":[{"modelId":"plain","name":"Plain display name","thinkingLevels":["off"]},
                   {"modelId":"reason","name":"Reasoning display name","thinkingLevels":["off","low","high"]}]},
        {"providerId":"other","providerLabel":"Other","canApiKey":true,"canOAuth":false,
         "models":[{"modelId":"other-model","name":"Unlinked display name","thinkingLevels":["off"]}]}])
}

const CHILD: &str = r#"#!/usr/bin/python3
import json,sys,os,pathlib,time
root=pathlib.Path(__file__).parent
plan=json.loads((root/'rpc-fixture.json').read_text())
owned=pathlib.Path(os.environ['PI_CODING_AGENT_DIR'])
providers=plan['providers'];pending={};live=None
def trace(value):
 with (root/'events.jsonl').open('a') as out: out.write(json.dumps(value)+'\n')
def emit(value): print(json.dumps(value),flush=True)
def response(r): emit({'type':'response','id':r['id'],'command':r['type'],'success':True,'data':{'disposition':'handled'}})
def event(r,kind,**values):
 v={'schema':1,'channel':'kota.auth','operationId':r['operationId'],'event':kind,**values}
 if 'providerId' in r: v['providerId']=r['providerId']
 emit({'type':'extension_ui_request','method':'notify','id':'notify-fixture','message':'kota.auth.v1:'+json.dumps(v),'notifyType':'info'})
def result(r,**values): event(r,'result',operation=r['operation'],outcome='succeeded',commit='not-written',**values)
trace({'stage':'spawn','offline':'--offline' in sys.argv})
for line in sys.stdin:
 r=json.loads(line)
 if r['type']=='get_commands':
  trace({'stage':'commands','pid':os.getpid()})
  if plan.get('exitBeforeCommand'): sys.exit(0)
  if plan.get('delayCommands'):
   time.sleep(10 if plan['delayCommands'] is True else plan['delayCommands'])
  emit({'type':'response','id':r['id'],'success':True,'data':{'commands':([] if plan.get('noCommand') else [{'name':'kota-auth','source':'extension'}])}})
 elif r['type']=='prompt':
  if not r['message'].startswith('/kota-auth '): sys.exit(4)
  cmd=json.loads(r['message'][len('/kota-auth '):]);op=cmd['operation'];trace({'operation':op,'pid':os.getpid(),**({'thinkingLevel':cmd['thinkingLevel']} if op=='resolve' else {})})
  if op=='status':
   if plan.get('failFirstStatus') and sum(json.loads(s).get('operation')=='status' for s in (root/'events.jsonl').read_text().splitlines())==1: sys.exit(1)
   statuses=[{k:p[k] for k in ['providerId','providerLabel','canApiKey','canOAuth']} for p in providers]
   for s in statuses: s.update(storedType=None,configured=True,policy='unknown',validity='not-checked')
   result(cmd,statuses=statuses);response(r)
  elif op=='catalog':
   if plan.get('storageFailed'):
    event(cmd,'result',operation=op,outcome='failed',commit='not-written',code='storage-failed');response(r);continue
   if plan.get('failFirstCatalog') and sum(json.loads(s).get('operation')=='catalog' for s in (root/'events.jsonl').read_text().splitlines())==1: sys.exit(1)
   if plan.get('delayCatalog'): time.sleep(plan['delayCatalog'])
   result(cmd,catalog={'providers':providers});response(r)
  elif op=='resolve':
   if plan.get('delayResolve'): time.sleep(plan['delayResolve'])
   if plan.get('removeBeforeResolve'): (owned/'auth.json').write_text('{}')
   p=next((p for p in providers if p['providerId']==cmd['providerId']),None)
   m=next((m for m in p['models'] if m['modelId']==cmd['modelId']),None) if p else None
   if not m: event(cmd,'result',operation=op,outcome='failed',commit='not-written',code='model-not-found')
   else: result(cmd,resolved={'providerId':cmd['providerId'],'modelId':cmd['modelId'],'requestedThinkingLevel':cmd['thinkingLevel'],'thinkingLevel':plan.get('actualThinking',cmd['thinkingLevel'] if cmd['thinkingLevel'] in m['thinkingLevels'] else 'off')})
   response(r)
  elif op=='login':
   if plan.get('exitAfterCommand'): sys.exit(1)
   live=cmd;native='dialog-'+cmd['operationId'];prompt='prompt-fixture'
   pending[native]=cmd
   event(cmd,'started',operation='login')
   if plan.get('expiry'): event(cmd,'auth_event',notification={'type':'device_code','userCode':'FAKE','verificationUri':'http://127.0.0.1:1/fixture','expiresInSeconds':plan['expiry']})
   body={'schema':1,'channel':'kota.auth','operationId':cmd['operationId'],'event':'prompt','providerId':cmd['providerId'],'authType':cmd['authType'],'promptId':prompt,'prompt':{'type':'secret','message':'Enter fixture key'}}
   if cmd['authType']=='api_key': body['credentialField']='api_key.key'
   emit({'type':'extension_ui_request','method':'input','id':native,'title':'kota.auth.v1:'+json.dumps(body)})
   response(r)
  elif op=='cancel':
   if live:
    event(live,'prompt_closed',promptId='prompt-fixture',reason='cancelled')
    event(live,'result',operation='login',outcome=('timeout' if plan.get('expiry') else 'cancelled'),commit='not-written')
    live=None
   result(cmd);response(r)
  elif op=='logout': result(cmd);response(r)
 elif r['type']=='extension_ui_response':
  original=pending.pop(r['id'],None)
  if original:
   trace({'stage':'reply','nativeId':r['id']})
   event(original,'prompt_closed',promptId='prompt-fixture',reason='answered')
   event(original,'result',operation='login',outcome='succeeded',commit='confirmed',credential={'key':'NEVER_FORWARD'},statuses=[{'key':'NEVER_FORWARD'}])
   live=None
"#;
