use super::*;
use std::{process::Command, sync::atomic::AtomicBool};

struct Discard;
impl local_rpc::Sink for Discard { fn notify(&self, _: &local_rpc::LocalNotice) -> std::result::Result<(), String> { Ok(()) } }
struct Fixture {
    auth: local_rpc::auth_fixture::Fixture,
    registry: AgentRegistry,
    runtime: LocalRuntime,
    root: PathBuf,
    cwd: PathBuf,
    current: Arc<AtomicBool>,
}
impl Fixture {
    fn new() -> Self {
        let auth = local_rpc::auth_fixture::Fixture::new(json!({}));
        auth.store(json!({"fable":{"type":"api_key","key":"fixture-only"}}));
        let root = auth.backend.owned.agent.join("project");
        let cwd = root.join(".agent-workspaces/agent-config"); fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join("SHELL.yaml"), "provider: kota-local\nmodel: fable/plain\neffort: off\nskills: [retained]\nunknown: preserved\n").unwrap();
        let current = Arc::new(AtomicBool::new(true)); let check = current.clone();
        let mut command = Command::new("/usr/bin/python3"); command.args(["-u", "-c", CHILD]).arg(&root);
        let scope = LocalScope { project_root:root.clone(), agent_id:"agent-config".into(), session_id:"fixture-session".into(), epoch:1 };
        let runtime = LocalRuntime::start(local_rpc::Config { scope, ledger:root.join("ledger.jsonl"), command,
            expected_model:None, current:Arc::new(move || check.load(Ordering::SeqCst)) }, Arc::new(Discard)).unwrap();
        let request: AgentSpawnRequest = serde_json::from_value(json!({"agentId":"agent-config","cli":"kota-local","cwd":cwd,"projectRoot":root,"sessionId":"fixture-session"})).unwrap();
        let registry = AgentRegistry::default();
        registry.locals.lock().unwrap().insert("agent-config".into(), LocalAgent { child_pid:runtime.snapshot().child_pid,
            runtime:runtime.clone(), request, lease_path:root.join("lease.json"), backend:Some(auth.backend.clone()) });
        fs::write(root.join("requests.jsonl"), "").unwrap();
        Self {auth,registry,runtime,root,cwd,current}
    }
    fn shell(&self) -> String { fs::read_to_string(self.cwd.join("SHELL.yaml")).unwrap() }
    fn revision(&self) -> String { crate::puppeteer_setup::revision(Some(self.shell().as_bytes())) }
    fn request(&self) -> Value { json!({"type":"set_model","provider":"fable","modelId":"reason","level":"high","shellRevision":self.revision()}) }
    fn apply(&self, request:Value) -> Result<Option<Value>> { self.registry.local_detail_control(&self.root,"agent-config",request) }
    fn commands(&self) -> Vec<Value> {
        fs::read_to_string(self.root.join("requests.jsonl")).unwrap_or_default().lines().map(|line|serde_json::from_str(line).unwrap()).collect()
    }
    fn flag(&self, flag:&str) { fs::write(self.root.join(flag), "fixture").unwrap(); }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = self.runtime.close(); } }

#[test]
fn detail_applies_model_and_thinking_then_saves_confirmed_clamp_and_revision_once() {
    let f = Fixture::new(); f.flag("clamp");
    let result = f.apply(f.request()).unwrap().unwrap();
    assert_eq!(result["model"], json!({"provider":"fable","id":"reason"}));
    assert_eq!(result["thinkingLevel"], "low"); assert_eq!(result["shellRevision"], f.revision());
    let shell = crate::parse_shell_yaml(&f.shell()).unwrap();
    assert_eq!(shell.model.as_deref(),Some("fable/reason")); assert_eq!(shell.effort.as_deref(),Some("low"));
    assert_eq!(shell.skills,["retained"]); assert!(f.shell().contains("unknown: preserved"));
    assert_eq!(shell.args,["--model","fable/reason","--thinking","low"]);
    let mut identity_stage = shell.clone();
    crate::sync_shell_launch_args(&mut identity_stage,AgentCli::KotaLocal);
    assert_eq!(identity_stage,shell,"the identity worker must not need another selection write");
    let commands = f.commands();
    let changes:Vec<_> = commands.iter().filter(|request|matches!(request["type"].as_str(),Some("set_model"|"set_thinking_level"))).collect();
    assert_eq!(changes.len(),2); assert_eq!(changes[0]["modelId"],"reason"); assert_eq!(changes[1]["level"],"high");
    assert!(changes.iter().all(|request|request.get("shellRevision").is_none() && request.get("autoRoute").is_none()));
    assert!(changes[0].get("level").is_none());
    assert_eq!(f.runtime.snapshot().thinking_level.as_deref(),Some("low"));
}

#[test]
fn detail_revision_conflict_is_rejected_before_auth_or_runtime_calls() {
    let f = Fixture::new(); let mut request = f.request(); request["shellRevision"] = json!("stale");
    let before = f.shell();
    assert_eq!(f.apply(request).unwrap_err().to_string(),"local_config_changed");
    assert_eq!(f.shell(),before); assert_eq!(f.auth.requests("resolve"),0);
    assert!(!f.commands().iter().any(|r|r["type"]=="set_model"));
}

#[test]
fn detail_does_not_wake_an_offline_owner_or_cross_a_project_scope() {
    let f = Fixture::new(); let before=f.shell();
    assert_eq!(f.registry.local_detail_control(&f.root.join("other"),"agent-config",f.request()).unwrap_err().to_string(),"local_session_changed");
    assert!(f.registry.local_detail_control(&f.root,"absent",f.request()).unwrap().is_none());
    f.runtime.close().unwrap();
    assert!(f.apply(f.request()).unwrap().is_none()); assert_eq!(f.shell(),before); assert_eq!(f.auth.requests("resolve"),0);
    // A stopped captured owner that has lost its epoch is not an offline-save fallback.
    f.current.store(false,Ordering::SeqCst);
    assert_eq!(f.apply(f.request()).unwrap_err().to_string(),"local_session_changed");
    assert_eq!(f.shell(),before);
}

#[test]
fn detail_rpc_partial_failure_does_not_save_or_claim_the_session_is_unchanged() {
    let f=Fixture::new(); f.flag("fail-thinking"); let before=f.shell();
    assert_eq!(f.apply(f.request()).unwrap_err().to_string(),"local_model_apply_unconfirmed");
    assert_eq!(f.shell(),before);
    let state=f.runtime.rpc(json!({"type":"get_state"})).unwrap();
    assert_eq!(state["model"]["id"],"reason"); assert_eq!(state["thinkingLevel"],"off");
}

#[test]
fn runtime_rejects_mismatched_readback_and_closes_the_session_without_saving() {
    let f=Fixture::new(); f.flag("wrong-model"); let before=f.shell();
    // observe_state stops the actor before registry receives a state to inspect.
    // This is an unconfirmed RPC result, not evidence of an unchanged session.
    assert_eq!(f.apply(f.request()).unwrap_err().to_string(),"local_model_apply_unconfirmed");
    local_rpc::auth_fixture::until(||f.runtime.snapshot().child_pid.is_none());
    assert_eq!(f.shell(),before);
}

#[test]
fn detail_successful_pi_application_followed_by_file_conflict_is_partial_success() {
    let f=Fixture::new(); f.flag("external-write");
    assert_eq!(f.apply(f.request()).unwrap_err().to_string(),"local_model_applied_save_failed");
    assert!(f.shell().contains("# external configuration"));
    assert_eq!(crate::parse_shell_yaml(&f.shell()).unwrap().model.as_deref(),Some("fable/plain"));
    assert_eq!(f.runtime.snapshot().model.unwrap()["id"],"reason");
}

#[test]
fn detail_holds_no_metadata_lock_over_rpc_and_never_writes_after_replacement() {
    let f=Fixture::new(); f.flag("pause-model"); let before=f.shell();
    std::thread::scope(|threads| {
        let task=threads.spawn(||f.apply(f.request()));
        local_rpc::auth_fixture::until(||f.root.join("model-entered").exists());
        let lock=crate::shell_switch::metadata_lock(&f.cwd);
        let guard=lock.try_lock().expect("no metadata lock while Pi is in flight"); drop(guard);
        f.current.store(false,Ordering::SeqCst); f.flag("release-model");
        assert_eq!(task.join().unwrap().unwrap_err().to_string(),"local_model_session_replaced");
    });
    assert_eq!(f.shell(),before);
}

#[test]
fn same_auto_selection_sends_the_complete_updated_group_without_reselecting_the_model() {
    let f=Fixture::new();
    local_rpc::jev::save_key_with(&f.auth.backend,"fixture","openrouter","fixture-only",|_,_,_,_|Ok("verified")).unwrap();
    let group=local_rpc::auto_route::fixture_group();
    let first=json!({"type":"set_model","provider":"kota","modelId":"auto","autoRoute":group,"shellRevision":f.revision()});
    f.apply(first).unwrap();
    fs::write(f.root.join("requests.jsonl"),"").unwrap();
    let mut next=group.clone();next.threshold=0.9;next.models[1].intent="Next-input updated intent".into();
    let result=f.apply(json!({"type":"set_model","provider":"kota","modelId":"auto","autoRoute":next,"shellRevision":f.revision()})).unwrap().unwrap();
    assert_eq!(result["autoRoute"],json!(next));assert_eq!(result["shellRevision"],f.revision());
    assert!(!f.commands().iter().any(|r|r["type"]=="set_model"));
    let prompts:Vec<_>=f.commands().into_iter().filter(|r|r["type"]=="prompt").collect();
    assert_eq!(prompts.len(),1);assert!(prompts[0]["message"].as_str().unwrap().contains("Next-input updated intent"));
    assert_eq!(crate::parse_shell_yaml(&f.shell()).unwrap().auto_route.unwrap().group(),next);
}

#[test]
fn ordinary_workbench_thinking_only_keeps_its_existing_model_none_writeback() {
    let f=Fixture::new();
    fs::write(f.cwd.join("SHELL.yaml"),"provider: kota-local\nmodel: fable/reason\neffort: high\n").unwrap();
    let scope=f.runtime.snapshot().scope;
    f.registry.local_workbench_control(&scope,json!({"type":"set_thinking_level","level":"off"})).unwrap();
    assert_eq!(crate::parse_shell_yaml(&f.shell()).unwrap().model.as_deref(),Some("fable/reason"));
    assert!(!f.shell().contains("args:"),"ordinary Workbench keeps its existing args behavior");
    assert!(!f.commands().iter().any(|r|r["type"]=="set_model"));
}

const CHILD:&str=r#"
import sys,json,pathlib,time
root=pathlib.Path(sys.argv[1]); model={'provider':'fable','id':'plain'}; thinking='off'; changed=False
for line in sys.stdin:
 r=json.loads(line); kind=r['type']
 with (root/'requests.jsonl').open('a') as out: out.write(json.dumps(r)+'\n')
 if kind=='set_model':
  model={'provider':r['provider'],'id':r['modelId']};changed=True;data=model
  if (root/'pause-model').exists():
   (root/'model-entered').write_text('ready')
   while not (root/'release-model').exists(): time.sleep(.001)
 elif kind=='set_thinking_level':
  if (root/'fail-thinking').exists():
   print(json.dumps({'type':'response','id':r['id'],'success':False,'error':'fixture thinking failure'}),flush=True);continue
  thinking='low' if (root/'clamp').exists() else r['level'];data={'level':thinking}
 elif kind=='get_state':
  actual={'provider':'fable','id':'wrong'} if changed and (root/'wrong-model').exists() else model
  data={'sessionId':'fixture-session','isStreaming':False,'model':actual,'thinkingLevel':thinking}
  if changed and (root/'external-write').exists():
   with (root/'.agent-workspaces/agent-config/SHELL.yaml').open('a') as out: out.write('# external configuration\n')
   (root/'external-write').unlink()
 elif kind=='get_entries': data={'entries':[],'leafId':None}
 elif kind=='prompt':
  data={'disposition':'handled'}
  if r['message'].startswith('/kota-history '):
   v=json.loads(r['message'][len('/kota-history '):])
   receipt={'schema':1,'requestId':v['requestId'],'result':{'status':'ready','entries':[],'leafId':None}}
   print(json.dumps({'type':'extension_ui_request','method':'notify','message':'kota.history.v1:'+json.dumps(receipt)}),flush=True)
 else: data={}
 print(json.dumps({'type':'response','id':r['id'],'success':True,'data':data}),flush=True)
"#;
