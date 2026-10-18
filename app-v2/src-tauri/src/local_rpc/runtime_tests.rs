use super::*;
use crate::local_rpc::protocol::fixture;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

struct Capture { notices: Mutex<Vec<LocalNotice>>, reject_entries: AtomicBool }
impl Sink for Capture {
    fn notify(&self, notice: &LocalNotice) -> std::result::Result<(), String> {
        self.notices.lock().unwrap().push(notice.clone());
        if matches!(notice, LocalNotice::Entries { .. }) && self.reject_entries.swap(false, Ordering::SeqCst) {
            Err("synthetic sink failure".into())
        } else { Ok(()) }
    }
}
fn capture() -> Arc<Capture> {
    Arc::new(Capture { notices: Mutex::new(Vec::new()), reject_entries: AtomicBool::new(false) })
}

// A protocol fixture only. The separate Pi integration uses the real extension
// and loopback fake provider; this fixture can deterministically reorder ACKs.
const CHILD: &str = r#"
import sys,json,pathlib,urllib.parse,time
root=pathlib.Path(sys.argv[1]); session=sys.argv[2]; persist=sys.argv[3]=='save'
file=root/'entries.json'; entries=json.loads(file.read_text()) if file.exists() else []
busy=False; held=[]; drained=[]; attempts={}; model={'provider':'fable','id':'confirmed'}; pending_model=None
def emit(v): print(json.dumps(v),flush=True)
def response(r,data=None): emit({'type':'response','id':r['id'],'command':r['type'],'success':True,'data':data})
def consume(v):
 global entries
 details={**v['details'],'kotaId':v['kotaId']}
 m={'role':'custom','customType':v['customType'],'content':v['content'],'details':details}
 entries.append({'id':str(len(entries)+1),'parentId':entries[-1]['id'] if entries else None,'type':'custom_message','customType':v['customType'],'content':v['content'],'details':details})
 emit({'type':'message_end','message':m})
 if persist: file.write_text(json.dumps(entries))
for line in sys.stdin:
 r=json.loads(line); kind=r['type']
 if kind=='get_state':
  if (root/'finish').exists():
   for v in held: consume(v)
   held=[];busy=False;emit({'type':'agent_settled'});(root/'finish').unlink()
  state_model=model
  if pending_model and (root/'release-model').exists():
   response(pending_model,model);pending_model=None
   state_model={'provider':'fable','id':'confirmed'}
  response(r,{'sessionId':session,'isStreaming':busy,'isCompacting':False,'model':state_model,'thinkingLevel':'off'})
 elif kind=='set_model':
  model={'provider':r['provider'],'id':r['modelId']}
  if (root/'delay-model').exists(): pending_model=r;(root/'model-applied').write_text('ready')
  else: response(r,model)
 elif kind=='get_entries':
  values=entries
  if 'since' in r:
   indices=[i for i,e in enumerate(entries) if e['id']==r['since']]
   values=entries[indices[0]+1:] if indices else entries
  response(r,{'entries':values,'leafId':entries[-1]['id'] if entries else None})
 elif kind=='prompt':
  msg=r['message']
  if msg.startswith('/kota-auth '):
   v=json.loads(msg[len('/kota-auth '):])
   with (root/'auth-refresh.jsonl').open('a') as f: f.write(json.dumps({'operation':v['operation'],'busy':busy})+'\n')
   result={'schema':1,'channel':'kota.auth','operationId':v['operationId'],'event':'result','operation':'refresh','outcome':('failed' if (root/'auth-fail').exists() else 'succeeded'),'commit':'not-written'}
   if (root/'ack-first').exists(): response(r,{'disposition':'handled'})
   emit({'type':'extension_ui_request','method':'notify','id':'notify-fixture','message':'kota.auth.v1:'+json.dumps(result),'notifyType':'info'})
   if not (root/'ack-first').exists(): response(r,{'disposition':'handled'})
   continue
  if msg.startswith('/kota-mcp '):
   v=json.loads(msg[len('/kota-mcp '):])
   if (root/'mcp-error').exists():
    emit({'type':'extension_error','extensionPath':'command:kota-mcp','event':'command','error':'fake sensitive error'})
    response(r,{'disposition':'handled'});continue
   result={'schema':1,'requestId':v['requestId'],'available':True,'servers':[{'name':'fake-server','status':'connected'}]}
   if (root/'mcp-bad').exists(): result['servers'][0]['reason']='fake sensitive error'
   if (root/'ack-first').exists(): response(r,{'disposition':'handled'})
   emit({'type':'extension_ui_request','method':'notify','id':'notify-mcp','message':'kota.mcp.v1:'+json.dumps(result)})
   if not (root/'ack-first').exists(): response(r,{'disposition':'handled'})
   continue
  if msg.startswith('/kota-reorder '):
   v=json.loads(msg[len('/kota-reorder '):]); byid={x['kotaId']:x for x in held}
   (root/'reorder-started').write_text('ready')
   if (root/'reorder-barrier').exists():
    while not (root/'reorder-release').exists(): time.sleep(.001)
   if set(v['ids'])!=set(byid):
    emit({'type':'extension_error','extensionPath':'command:kota-reorder','event':'command','error':'kota-reorder-changed requestId='+v['requestId']})
   elif (root/'reorder-error').exists():
    emit({'type':'extension_error','extensionPath':'command:kota-reorder','event':'command','error':'synthetic failure'})
   else: held=[byid[key] for key in v['ids']]
   if (root/'reorder-silent').exists(): continue
   response(r,{'disposition':'handled'});continue
  if msg.startswith('/kota-cancel '):
   key=msg[len('/kota-cancel '):];found=next((v for v in held if v['kotaId']==key),None)
   if found and found['details']['messageId']=='hold-cancel-barrier':
    (root/'cancel-started').write_text('ready')
    while not (root/'cancel-release').exists(): time.sleep(.001)
   with (root/'cancelled.jsonl').open('a') as f: f.write(key+'\n')
   if found and found['details']['messageId']=='hold-cancel-unexpected':
    emit({'type':'extension_error','extensionPath':'command:kota-cancel','event':'command','error':'synthetic unexpected handler failure'})
   elif found: held.remove(found)
   else: emit({'type':'extension_error','extensionPath':'command:kota-cancel','event':'command','error':'kota-cancel-too-late kotaId='+urllib.parse.quote(key,safe='')})
   response(r,{'disposition':'handled'});continue
  if msg.startswith('/kota-hold-'):
   command,raw=msg.split(' ',1); action=command.removeprefix('/kota-hold-'); control=json.loads(raw)
   with (root/'stop-order.jsonl').open('a') as f: f.write(action+'\n')
   if (root/('fail-'+action)).exists():
    emit({'type':'response','id':r['id'],'command':kind,'success':False,'error':'synthetic handoff failure'});continue
   if (root/('error-'+action)).exists():
    emit({'type':'extension_error','extensionPath':'command:kota-hold-'+action,'event':'command','error':'synthetic unexpected handoff failure'})
    response(r,{'disposition':'handled'});continue
   if (root/('missing-'+action)).exists():
    response(r,{'disposition':'handled'});continue
   if action=='pause' and any(v['details']['messageId']=='hold-barrier' for v in held):
    (root/'stop-started').write_text('ready')
    while not (root/'stop-release').exists(): time.sleep(.001)
   ids=[v['kotaId'] for v in held]
   if action=='resume':
    for v in held: consume(v)
    held=[]
   if (root/'ack-first').exists(): response(r,{'disposition':'handled'})
   result={'schema':1,'requestId':control['requestId'],'action':action,'kotaIds':ids}
   emit({'type':'extension_ui_request','method':'notify','id':'notify-queue','message':'kota.queue.v1:'+json.dumps(result)})
   if not (root/'ack-first').exists(): response(r,{'disposition':'handled'})
   continue
  v=json.loads(r['message'].removeprefix('/kota-inject '))
  with (root/'received.jsonl').open('a') as f: f.write(json.dumps(v)+'\n')
  key=v['details']['messageId']; attempts[key]=attempts.get(key,0)+1
  if key=='hold-inject-barrier':
   (root/'inject-started').write_text('ready')
   while not (root/'inject-release').exists(): time.sleep(.001)
  if key=='fail-once' and attempts[key]==1:
   error='kota-inject-failed kotaId='+urllib.parse.quote(v['kotaId'],safe='')+' code=invalid-envelope'
   emit({'type':'extension_error','extensionPath':'command:kota-inject','event':'command','error':error})
   response(r,{'disposition':'handled'});continue
  if key=='unrelated-error':
   emit({'type':'extension_error','extensionPath':'other-extension','event':'command','error':'synthetic uncorrelated fault'})
   response(r,{'disposition':'handled'}); continue
  if v['details']['messageId'].startswith('hold'):
   (drained if key=='hold-drained' else held).append(v); busy=True; emit({'type':'agent_start'})
  else: consume(v)
  response(r,{'disposition':'handled'})
  if not busy: emit({'type':'agent_settled'})
 elif kind=='clear_queue':
  with (root/'stop-order.jsonl').open('a') as f: f.write('clear_queue\n')
  response(r,{})
  for v in drained: consume(v)
  drained=[];busy=False;emit({'type':'agent_settled'})
 elif kind=='abort':
  with (root/'stop-order.jsonl').open('a') as f: f.write('abort\n')
  if (root/'missing-abort').exists(): (root/'abort-started').write_text('ready');continue
  busy=False; response(r,{}); emit({'type':'agent_settled'})
 else: response(r,{})
"#;

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!("local-owner-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&path).unwrap(); path
}
fn config(root: &std::path::Path, epoch: u64, save: bool) -> Config {
    let mut command = Command::new("/usr/bin/python3");
    command.args(["-u", "-c", CHILD]).arg(root).arg("session-fixture").arg(if save { "save" } else { "lose" });
    Config { scope: LocalScope { project_root: root.into(), agent_id: "agent-fable-copper".into(), session_id: "session-fixture".into(), epoch },
        ledger: root.join("ledger.jsonl"), command, expected_model:None, current: Arc::new(|| true) }
}

// One executable serves both the parent protocol fixture and a bare copy.
// Stop faults are injected only in the copy's Rust cleanup, after its real
// process has exited; neither credentials nor a model endpoint are involved.
fn clone_config(root: &std::path::Path, finish: bool, fail_stop: bool) -> Config {
    use std::os::unix::fs::PermissionsExt;
    fs::write(root.join("parent-fixture.py"), CHILD).unwrap();
    let executable = root.join("clone-fixture");
    fs::write(&executable, r#"#!/usr/bin/python3
import sys,json,pathlib,os,time
a=sys.argv[1:]
if '--session-id' not in a:
 exec((pathlib.Path(a[0])/'parent-fixture.py').read_text())
else:
 def arg(name): return a[a.index(name)+1]
 directory=pathlib.Path(arg('--session-dir')); model=arg('--model').split('/',1)
 def emit(v): print(json.dumps(v),flush=True)
 for line in sys.stdin:
  r=json.loads(line)
  if r['type']=='get_state':
   emit({'type':'response','id':r['id'],'success':True,'data':{'sessionId':arg('--session-id'),'isStreaming':False,'model':{'provider':model[0],'id':model[1]},'thinkingLevel':arg('--thinking')}})
  elif r['type']=='prompt':
   directory.joinpath('prompt-started').write_text('ready')
   emit({'type':'response','id':r['id'],'success':True,'data':{}})
   emit({'type':'agent_start'})
   if os.environ.get('FIXTURE_CLONE_FINISH')!='1':
    while True: time.sleep(.005)
   emit({'type':'message_end','message':{'role':'assistant','stopReason':'stop','content':[{'type':'text','text':'Synthetic copy reply.'}]}})
   emit({'type':'agent_settled'})
"#).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = config(root, 1, true);
    config.command = Command::new(executable);
    config.command.current_dir(root).arg(root).args(["session-fixture", "save"])
        .env_clear().env("FIXTURE_CLONE_FINISH", if finish { "1" } else { "0" })
        .env("KOTA_TEST_CLONE_STOP_FAILURE", if fail_stop { "1" } else { "0" });
    config
}

#[test]
fn clone_stop_failure_keeps_parent_ready_and_its_existing_queue_unchanged() {
    use crate::local_rpc::shadow_clone::{Request, Task};
    // Background expiry, successful completion's reap, and human Stop all
    // have to contain the same low-level stop error.
    for mode in ["timeout", "finished", "human-stop"] {
        let root = root(); let sink = capture();
        let runtime = LocalRuntime::start(clone_config(&root, mode == "finished", true), sink.clone()).unwrap();
        // Keep pump inactive, as during a registry's activation boundary, so
        // the parent can be Ready with a real held ledger item to preserve.
        let queued = fixture("parent-queued");
        runtime.submit(queued.clone()).unwrap();
        let before = fs::read(root.join("ledger.jsonl")).unwrap();
        let queue = serde_json::to_value(runtime.workbench(None).queue).unwrap();
        let parent_pid = runtime.snapshot().child_pid;
        runtime.clone_control(Request::Create { tasks: vec![Task {
            task: "Synthetic copy task.".into(), read_only: true,
            timeout_minutes: if mode == "timeout" { 0.01 } else { 0.0 },
        }] }).unwrap();
        until(|| root.join("under-table/clone-1/prompt-started").exists());
        if mode == "human-stop" { runtime.rpc(json!({"type":"abort"})).unwrap(); }
        until(|| runtime.clones(Some(1))["clones"][0]["view"]["error"].as_str().is_some_and(|error| error.contains("Operation not permitted")));
        let copy = runtime.clones(Some(1));
        assert_eq!(copy["clones"][0]["state"], "stopped", "{mode}");
        assert_eq!(copy["clones"][0]["view"]["state"], "offline");
        assert_eq!(runtime.snapshot().state, OnlineState::Ready, "{mode}");
        assert_eq!(runtime.snapshot().child_pid, parent_pid);
        assert_eq!(runtime.snapshot().error, None);
        assert_eq!(fs::read(root.join("ledger.jsonl")).unwrap(), before);
        assert_eq!(serde_json::to_value(runtime.workbench(None).queue).unwrap(), queue);
        assert_eq!(received(&root), 0);
        assert!(runtime.rpc(json!({"type":"get_state"})).is_ok());
        assert!(!sink.notices.lock().unwrap().iter().any(|notice| matches!(notice, LocalNotice::Status { state:OnlineState::Error, .. })));
        // Prove that the unchanged input still runs when activation resumes.
        runtime.activate(false).unwrap();
        until(|| seen(&sink, &queued.kota_id, DeliveryState::Consumed));
        runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn rejected_clone_report_is_recorded_on_the_copy_without_failing_the_parent() {
    use crate::local_rpc::shadow_clone::{Request, Task};
    let root = root(); let sink = capture();
    let mut config = clone_config(&root, true, false);
    // This malformed synthetic identity makes the ordinary Injection validator
    // reject the report before a ledger write. It exercises the report error
    // branch without adding a production fault switch or corrupting a file.
    config.scope.agent_id = "invalid target".into();
    let runtime = LocalRuntime::start(config, sink.clone()).unwrap();
    let before = fs::read(root.join("ledger.jsonl")).unwrap();
    runtime.clone_control(Request::Create { tasks: vec![Task {
        task: "Synthetic copy task.".into(), read_only: true, timeout_minutes: 0.0,
    }] }).unwrap();
    until(|| runtime.clones(Some(1))["clones"][0]["view"]["error"] == "invalid Local injection identity or source");
    assert_eq!(runtime.clones(None)["clones"][0]["state"], "stopped");
    assert_eq!(runtime.snapshot().state, OnlineState::Ready);
    assert_eq!(runtime.snapshot().error, None);
    assert_eq!(runtime.snapshot().queued, 0);
    assert_eq!(fs::read(root.join("ledger.jsonl")).unwrap(), before);
    assert_eq!(received(&root), 0);
    assert!(runtime.rpc(json!({"type":"get_state"})).is_ok());
    runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
}
fn until(mut predicate: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !predicate() { assert!(Instant::now() < end, "Local fixture timed out"); thread::sleep(Duration::from_millis(2)); }
}
fn seen(sink: &Capture, id: &str, state: DeliveryState) -> bool {
    sink.notices.lock().unwrap().iter().any(|n| matches!(n,
        LocalNotice::Delivery { kota_id, state: current, .. } if kota_id == id && *current == state))
}
fn received(root: &std::path::Path) -> usize {
    fs::read_to_string(root.join("received.jsonl")).unwrap_or_default().lines().count()
}

#[test]
fn startup_refuses_pi_fallback_to_a_different_model_before_delivery() {
    let root=root();let sink=capture();let mut config=config(&root,1,true);
    config.expected_model=Some(crate::local_rpc::models::ModelSelection {provider_id:"fable".into(),model_id:"requested".into()});
    assert!(LocalRuntime::start(config,sink.clone()).is_err());
    assert_eq!(received(&root),0);
    assert!(!sink.notices.lock().unwrap().iter().any(|n|matches!(n,LocalNotice::Status {state:OnlineState::Ready,..})));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mcp_status_joins_both_ack_orders_and_never_enters_workbench_ledger_or_history() {
    for ack_first in [false,true] {
        let root=root();let sink=capture();if ack_first { fs::write(root.join("ack-first"),"").unwrap(); }
        let runtime=LocalRuntime::start(config(&root,1,true),sink.clone()).unwrap();runtime.activate(false).unwrap();
        let before=fs::read(root.join("ledger.jsonl")).unwrap();
        let value=runtime.rpc(json!({"type":"get_mcp_status"})).unwrap();
        assert_eq!(value["servers"][0]["status"],"connected");
        assert_eq!(fs::read(root.join("ledger.jsonl")).unwrap(),before);
        assert_eq!(received(&root),0);
        assert!(!serde_json::to_string(&runtime.workbench(None)).unwrap().contains("kota.mcp"));
        assert!(!serde_json::to_string(&*sink.notices.lock().unwrap()).unwrap().contains("kota.mcp"));
        assert!(!root.join("entries.json").exists());
        runtime.close().unwrap();fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn invalid_mcp_state_or_command_failure_does_not_stop_the_owner_or_lock_other_controls() {
    for marker in ["mcp-error","mcp-bad"] {
        let root=root();fs::write(root.join(marker),"").unwrap();
        let runtime=LocalRuntime::start(config(&root,1,true),capture()).unwrap();runtime.activate(false).unwrap();
        assert_eq!(runtime.rpc(json!({"type":"get_mcp_status"})).unwrap_err().to_string(),"mcp_status_unavailable");
        assert_eq!(runtime.snapshot().state,OnlineState::Ready);
        assert!(runtime.rpc(json!({"type":"get_state"})).is_ok());
        assert!(runtime.rpc(json!({"type":"get_mcp_status","path":"/other"})).is_err());
        runtime.close().unwrap();fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn model_switch_keeps_the_old_confirmed_snapshot_and_ignores_a_stale_state_reply() {
    let root = root(); let mut cfg = config(&root, 1, true);
    cfg.expected_model = Some(crate::local_rpc::models::ModelSelection { provider_id:"fable".into(), model_id:"confirmed".into() });
    let runtime = LocalRuntime::start(cfg, capture()).unwrap(); runtime.activate(false).unwrap();
    fs::write(root.join("delay-model"), "").unwrap();
    let changing = runtime.clone();
    let switch = thread::spawn(move || changing.rpc(json!({"type":"set_model","provider":"other","modelId":"replacement"})));
    until(|| root.join("model-applied").exists());
    let during = runtime.rpc(json!({"type":"get_state"})).unwrap();
    assert_eq!(during["model"]["provider"], "other");
    assert_eq!(runtime.snapshot().model.unwrap()["provider"], "fable", "in-use remains the acknowledged model until set_model returns");
    fs::write(root.join("release-model"), "").unwrap();
    let stale = runtime.rpc(json!({"type":"get_state"})).unwrap();
    assert_eq!(stale["model"]["provider"], "fable");
    assert_eq!(switch.join().unwrap().unwrap()["provider"], "other");
    assert_eq!(runtime.snapshot().model.unwrap()["provider"], "other", "the older read cannot overwrite the newly acknowledged model");
    assert_eq!(runtime.rpc(json!({"type":"get_state"})).unwrap()["model"]["id"], "replacement");
    assert!(runtime.snapshot().child_pid.is_some());
    runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
}

#[test]
fn auth_refresh_waits_for_idle_is_private_and_never_stops_the_agent_on_metadata_failure() {
    for ack_first in [false,true] {
        let root=root();let sink=capture();if ack_first {fs::write(root.join("ack-first"),"").unwrap();}
        let mut config=config(&root,1,true);
        config.expected_model=Some(crate::local_rpc::models::ModelSelection {provider_id:"fable".into(),model_id:"confirmed".into()});
        let runtime=LocalRuntime::start(config,sink.clone()).unwrap();runtime.activate(false).unwrap();
        let input=fixture("hold-auth-refresh");runtime.submit(input.clone()).unwrap();
        until(||seen(&sink,&input.kota_id,DeliveryState::Accepted));
        runtime.refresh_auth();runtime.rpc(json!({"type":"get_state"})).unwrap();
        assert!(!root.join("auth-refresh.jsonl").exists());
        fs::write(root.join("finish"),"").unwrap();runtime.rpc(json!({"type":"get_state"})).unwrap();
        until(||fs::read_to_string(root.join("auth-refresh.jsonl")).unwrap_or_default().lines().count()==1);
        fs::write(root.join("auth-fail"),"").unwrap();runtime.refresh_auth();
        until(||fs::read_to_string(root.join("auth-refresh.jsonl")).unwrap_or_default().lines().count()==2);
        runtime.rpc(json!({"type":"get_state"})).unwrap();
        assert_eq!(runtime.snapshot().state,OnlineState::Ready);assert!(runtime.snapshot().child_pid.is_some());
        assert!(!sink.notices.lock().unwrap().iter().any(|n|matches!(n,LocalNotice::Event {event,..} if event["type"]=="extension_ui_request")));
        for line in fs::read_to_string(root.join("auth-refresh.jsonl")).unwrap().lines() {assert_eq!(serde_json::from_str::<Value>(line).unwrap()["busy"],false);}
        runtime.close().unwrap();fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn ordered_owner_preserves_early_consumption_idempotency_and_clear_race() {
    let root = root(); let sink = capture();
    let runtime = LocalRuntime::start(config(&root, 1, true), sink.clone()).unwrap();
    assert_eq!(runtime.snapshot().state, OnlineState::Ready);
    runtime.activate(false).unwrap();
    let a = fixture("input-a");
    runtime.submit(a.clone()).unwrap();
    until(|| seen(&sink, &a.kota_id, DeliveryState::Consumed));
    runtime.submit(a.clone()).unwrap();
    runtime.rpc(json!({"type":"get_state"})).unwrap();
    assert_eq!(received(&root), 1);
    let held = fixture("hold-drained");
    runtime.submit(held.clone()).unwrap();
    until(|| seen(&sink, &held.kota_id, DeliveryState::Accepted));
    runtime.rpc(json!({"type":"abort"})).unwrap();
    until(|| seen(&sink, &held.kota_id, DeliveryState::Consumed));
    assert!(seen(&sink, &held.kota_id, DeliveryState::Cleared));
    runtime.close().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovery_reloads_full_entries_without_replaying_old_missing_inputs() {
    let root=root();let sink=capture();let runtime=LocalRuntime::start(config(&root,1,false),sink.clone()).unwrap();
    runtime.activate(false).unwrap();let old=fixture("input-lost-on-disk");runtime.submit(old.clone()).unwrap();
    until(||seen(&sink,&old.kota_id,DeliveryState::Consumed));runtime.close().unwrap();
    let recovered=LocalRuntime::start(config(&root,2,true),sink.clone()).unwrap();recovered.activate(false).unwrap();
    recovered.rpc(json!({"type":"get_state"})).unwrap();assert_eq!(received(&root),1);
    assert!(seen(&sink,&old.kota_id,DeliveryState::Abandoned));
    let new=fixture("input-new");recovered.submit(new.clone()).unwrap();until(||seen(&sink,&new.kota_id,DeliveryState::Consumed));
    recovered.close().unwrap();let confirmed=LocalRuntime::start(config(&root,3,true),sink.clone()).unwrap();confirmed.activate(false).unwrap();
    until(||seen(&sink,&new.kota_id,DeliveryState::Durable));confirmed.submit(new).unwrap();confirmed.rpc(json!({"type":"get_state"})).unwrap();
    assert_eq!(received(&root),2);confirmed.close().unwrap();fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_entries_projection_keeps_cursor_for_the_next_read() {
    let root = root(); let sink = capture();
    let runtime = LocalRuntime::start(config(&root, 1, true), sink.clone()).unwrap();
    runtime.activate(false).unwrap();
    sink.reject_entries.store(true, Ordering::SeqCst);
    runtime.submit(fixture("input-a")).unwrap();
    until(|| received(&root) == 1);
    runtime.rpc(json!({"type":"abort"})).unwrap();
    until(|| sink.notices.lock().unwrap().iter().filter(|n| matches!(n, LocalNotice::Entries { entries, since: None, .. } if !entries.is_empty())).count() >= 2);
    runtime.close().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn controlled_refusal_wins_late_success_and_retry_keeps_original_identity() {
    let root = root(); let sink = capture();
    let runtime = LocalRuntime::start(config(&root, 1, true), sink.clone()).unwrap();
    runtime.activate(false).unwrap();
    let busy = fixture("busy-once"); runtime.submit(busy.clone()).unwrap();
    until(|| seen(&sink, &busy.kota_id, DeliveryState::Consumed));
    assert_eq!(received(&root), 1);
    let failed = fixture("fail-once"); runtime.submit(failed.clone()).unwrap();
    until(|| seen(&sink, &failed.kota_id, DeliveryState::Failed));
    runtime.rpc(json!({"type":"get_state"})).unwrap();
    assert!(!seen(&sink, &failed.kota_id, DeliveryState::Accepted));
    runtime.submit(failed.clone()).unwrap();
    runtime.rpc(json!({"type":"get_state"})).unwrap();
    assert_eq!(received(&root), 2, "resubmission alone is not an explicit retry");
    runtime.retry(failed.kota_id.clone()).unwrap();
    until(|| seen(&sink, &failed.kota_id, DeliveryState::Consumed));
    let inputs: Vec<Injection> = fs::read_to_string(root.join("received.jsonl")).unwrap().lines()
        .map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(inputs, vec![busy, failed.clone(), failed]);
    let unrelated = fixture("unrelated-error"); runtime.submit(unrelated.clone()).unwrap();
    until(|| seen(&sink, &unrelated.kota_id, DeliveryState::Accepted));
    assert!(!seen(&sink, &unrelated.kota_id, DeliveryState::Failed));
    runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
}

#[test]
fn superseded_epoch_closes_its_child_and_releases_ledger_without_more_input() {
    let root = root(); let sink = capture();
    let current = Arc::new(AtomicBool::new(true)); let check = current.clone();
    let mut cfg = config(&root, 1, false); cfg.current = Arc::new(move || check.load(Ordering::SeqCst));
    let runtime = LocalRuntime::start(cfg, sink).unwrap();
    runtime.submit(fixture("never-dispatched")).unwrap();
    current.store(false, Ordering::SeqCst);
    until(|| runtime.inner_join_finished_for_test());
    assert_eq!(runtime.snapshot().child_pid, None); assert_eq!(received(&root), 0);
    assert!(runtime.submit(fixture("late-old-owner")).is_err());
    let recovered = LocalRuntime::start(config(&root, 2, true), capture()).unwrap();
    recovered.activate(false).unwrap(); recovered.rpc(json!({"type":"get_state"})).unwrap(); assert_eq!(received(&root),0);
    recovered.close().unwrap(); runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovery_projects_before_restatement_and_retains_every_pending_item() {
    let root = root(); let sink = capture();
    let count = 160; // More than the notifier's live-event channel capacity.
    {
        let mut ledger = Ledger::open(&root.join("ledger.jsonl")).unwrap();
        for i in 0..count { ledger.hold("session-fixture", fixture(&format!("held-{i:03}"))).unwrap(); }
        let terminal = fixture("historical-terminal");
        ledger.hold("session-fixture", terminal.clone()).unwrap();
        ledger.record("session-fixture", &terminal.kota_id, DeliveryState::Cleared).unwrap();
    }
    let runtime = LocalRuntime::start(config(&root, 1, true), sink.clone()).unwrap();
    assert_eq!(runtime.snapshot().queued, 0);
    assert_eq!(received(&root), 0, "no dispatch before registry activation");
    {
        let notices = sink.notices.lock().unwrap();
        let recovery = notices.iter().position(|n| matches!(n, LocalNotice::Entries { recovery: true, since: None, .. })).unwrap();
        let deliveries: Vec<_> = notices.iter().enumerate().filter_map(|(i, n)| match n {
            LocalNotice::Delivery { kota_id, state, .. } => Some((i, kota_id, state)), _ => None,
        }).collect();
        assert_eq!(deliveries.len(), count);
        for (i, (position, id, state)) in deliveries.iter().enumerate() {
            assert!(*position > recovery);
            assert_eq!(**id, fixture(&format!("held-{i:03}")).kota_id);
            assert_eq!(**state, DeliveryState::Abandoned);
        }
    }
    runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_initial_projection_closes_child_before_any_replay() {
    let root = root(); let sink = capture();
    let input = fixture("waiting-for-recovery");
    {
        let mut ledger = Ledger::open(&root.join("ledger.jsonl")).unwrap();
        ledger.hold("session-fixture", input.clone()).unwrap();
        ledger.record("session-fixture", &input.kota_id, DeliveryState::Accepted).unwrap();
    }
    sink.reject_entries.store(true, Ordering::SeqCst);
    let failed = LocalRuntime::start(config(&root, 1, true), sink.clone());
    assert!(failed.err().unwrap().to_string().contains("recovery projection failed"));
    assert_eq!(received(&root), 0);
    assert!(!seen(&sink, &input.kota_id, DeliveryState::Held));
    let next = LocalRuntime::start(config(&root, 2, true), sink.clone()).unwrap();
    assert!(seen(&sink, &input.kota_id, DeliveryState::Abandoned));
    next.activate(false).unwrap();
    next.rpc(json!({"type":"get_state"})).unwrap();
    assert_eq!(received(&root), 0);
    next.close().unwrap(); fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_stop_pause_abandons_waiting_inputs_and_recovery_never_replays_them() {
    let root = root(); let sink = capture();
    let runtime = LocalRuntime::start(config(&root, 1, true), sink.clone()).unwrap();
    runtime.activate(false).unwrap();
    fs::write(root.join("fail-pause"), "").unwrap();
    let input = fixture("hold-pause-fails"); runtime.submit(input.clone()).unwrap();
    until(|| seen(&sink, &input.kota_id, DeliveryState::Accepted));
    assert!(runtime.rpc(json!({"type":"abort"})).is_err());
    until(|| runtime.inner_join_finished_for_test() && seen(&sink, &input.kota_id, DeliveryState::Abandoned));
    assert!(!seen(&sink, &input.kota_id, DeliveryState::Cleared));
    let next = LocalRuntime::start(config(&root, 2, true), sink.clone()).unwrap();
    next.activate(false).unwrap();
    assert!(seen(&sink, &input.kota_id, DeliveryState::Abandoned));
    assert_eq!(received(&root), 1, "Failed Stop input must never replay");
    next.close().unwrap(); runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
}

#[test]
fn precise_edit_blocks_ordinary_retry_and_requeues_at_the_tail() {
 let root=root();let sink=capture();let runtime=LocalRuntime::start(config(&root,1,true),sink.clone()).unwrap();runtime.activate(false).unwrap();
 let a=fixture("hold-edit");let b=fixture("hold-other");
 runtime.submit(a.clone()).unwrap();runtime.submit(b.clone()).unwrap();until(||seen(&sink,&b.kota_id,DeliveryState::Accepted));
 let cancel=runtime.queue_action(json!({"type":"queue_cancel","kotaId":a.kota_id,"edit":true})).unwrap();assert_eq!(cancel["cancelled"],true);
 let token=cancel["editToken"].clone();assert!(runtime.retry(a.kota_id.clone()).is_err());
 assert_eq!(runtime.queue_action(json!({"type":"queue_edit_valid","kotaId":a.kota_id,"editToken":token})).unwrap()["valid"],true);
 runtime.queue_action(json!({"type":"queue_requeue","kotaId":a.kota_id,"editToken":token})).unwrap();
 until(||received(&root)==3);fs::write(root.join("finish"),"").unwrap();runtime.rpc(json!({"type":"get_state"})).unwrap();
 until(||seen(&sink,&a.kota_id,DeliveryState::Consumed));
 let entries:Vec<Value>=serde_json::from_slice(&fs::read(root.join("entries.json")).unwrap()).unwrap();
 assert_eq!(entries.iter().filter_map(custom_receipt).map(|r|r.kota_id).collect::<Vec<_>>(),vec![b.kota_id.clone(),a.kota_id.clone()]);
 let late=runtime.queue_action(json!({"type":"queue_cancel","kotaId":a.kota_id})).unwrap();assert_eq!(late["reason"],"too_late");
 runtime.close().unwrap();fs::remove_dir_all(root).unwrap();
}

#[test]
fn stop_retains_held_order_accepts_later_inputs_last_and_preserves_the_editor() {
 for ack_first in [false, true] {
 let root=root();let sink=capture();if ack_first {fs::write(root.join("ack-first"),"").unwrap();}
 let runtime=LocalRuntime::start(config(&root,1,true),sink.clone()).unwrap();runtime.activate(false).unwrap();
 let edit=fixture("hold-edit");runtime.submit(edit.clone()).unwrap();until(||seen(&sink,&edit.kota_id,DeliveryState::Accepted));
 let token=runtime.queue_action(json!({"type":"queue_cancel","kotaId":edit.kota_id,"edit":true})).unwrap()["editToken"].clone();
 let a=fixture("hold-barrier");let b=fixture("hold-b");runtime.submit(a.clone()).unwrap();runtime.submit(b.clone()).unwrap();until(||seen(&sink,&b.kota_id,DeliveryState::Accepted));
 let copy=runtime.clone();let stop=thread::spawn(move||copy.rpc(json!({"type":"abort"})));
 until(||root.join("stop-started").exists());let c=fixture("later");runtime.submit(c.clone()).unwrap();
 assert_eq!(received(&root),3,"pump remains closed while pause is in flight");
 assert_eq!(runtime.queue_action(json!({"type":"queue_edit_valid","kotaId":edit.kota_id,"editToken":token})).unwrap()["valid"],true);
 fs::write(root.join("stop-release"),"").unwrap();stop.join().unwrap().unwrap();
 until(||seen(&sink,&c.kota_id,DeliveryState::Consumed));
 let entries:Vec<Value>=serde_json::from_slice(&fs::read(root.join("entries.json")).unwrap()).unwrap();
 assert_eq!(entries.iter().filter_map(custom_receipt).map(|r|r.kota_id).collect::<Vec<_>>(),vec![a.kota_id.clone(),b.kota_id.clone(),c.kota_id.clone()]);
 assert_eq!(fs::read_to_string(root.join("stop-order.jsonl")).unwrap(),"pause\nabort\nclear_queue\nresume\n");
 for id in [&a.kota_id,&b.kota_id,&c.kota_id] {assert!(!seen(&sink,id,DeliveryState::Cleared));}
 assert_eq!(runtime.queue_action(json!({"type":"queue_edit_valid","kotaId":edit.kota_id,"editToken":token})).unwrap()["valid"],true);
 runtime.queue_action(json!({"type":"queue_requeue","kotaId":edit.kota_id,"editToken":token})).unwrap();
 until(||received(&root)==5);
 assert!(!sink.notices.lock().unwrap().iter().any(|n|matches!(n,LocalNotice::Event {event,..} if event["message"].as_str().is_some_and(|s|s.starts_with(QUEUE_CONTROL_CODEC)))));
 runtime.close().unwrap();fs::remove_dir_all(root).unwrap();
 }
}

#[test]
fn outstanding_item_credit_keeps_the_overflow_in_rust_until_precise_cancel() {
 let root=root();let sink=capture();let runtime=LocalRuntime::start(config(&root,1,true),sink.clone()).unwrap();runtime.activate(false).unwrap();
 for i in 0..66 {runtime.submit(fixture(&format!("hold-credit-{i}"))).unwrap();}
 until(||received(&root)==64);runtime.rpc(json!({"type":"get_state"})).unwrap();assert_eq!(received(&root),64);
 let first=fixture("hold-credit-0");runtime.queue_action(json!({"type":"queue_cancel","kotaId":first.kota_id})).unwrap();until(||received(&root)==65);
 runtime.rpc(json!({"type":"abort"})).unwrap();until(||received(&root)==66);assert_eq!(runtime.snapshot().queued,1,"Rust-held overflow was not cleared; it is delivered after resume");
 runtime.close().unwrap();fs::remove_dir_all(root).unwrap();
}

#[test]
fn unexpected_cancel_error_cannot_become_a_successful_withdrawal() {
    let root = root(); let sink = capture();
    let runtime = LocalRuntime::start(config(&root, 1, true), sink.clone()).unwrap(); runtime.activate(false).unwrap();
    let input = fixture("hold-cancel-unexpected"); runtime.submit(input.clone()).unwrap();
    until(|| seen(&sink, &input.kota_id, DeliveryState::Accepted));
    let result = runtime.queue_action(json!({"type":"queue_cancel","kotaId":input.kota_id}));
    assert!(result.unwrap_err().to_string().contains("Couldn't remove"));
    assert!(!seen(&sink, &input.kota_id, DeliveryState::Cleared));
    assert_eq!(runtime.snapshot().queued, 1);
    runtime.rpc(json!({"type":"abort"})).unwrap(); runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
}

#[test]
fn unexpected_pause_or_resume_error_stops_the_child_and_abandons_its_queue() {
 for action in ["pause", "resume"] {
    let root = root(); let sink = capture(); fs::write(root.join(format!("error-{action}")), "").unwrap();
    let runtime = LocalRuntime::start(config(&root, 1, true), sink.clone()).unwrap(); runtime.activate(false).unwrap();
    let input = fixture("hold-unexpected"); runtime.submit(input.clone()).unwrap();
    until(|| seen(&sink, &input.kota_id, DeliveryState::Accepted));
    assert!(runtime.rpc(json!({"type":"abort"})).is_err());
    until(|| runtime.snapshot().state == OnlineState::Error && runtime.inner_join_finished_for_test());
    assert!(runtime.snapshot().child_pid.is_none());
    assert!(seen(&sink, &input.kota_id, DeliveryState::Abandoned));
    assert!(!seen(&sink, &input.kota_id, DeliveryState::Consumed));
    runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
 }
}

#[test]
fn consecutive_removals_wait_in_fifo_instead_of_failing_busy() {
    let root = root(); let sink = capture();
    let runtime = LocalRuntime::start(config(&root, 1, true), sink.clone()).unwrap(); runtime.activate(false).unwrap();
    let first = fixture("hold-cancel-barrier"); let second = fixture("hold-second");
    runtime.submit(first.clone()).unwrap(); runtime.submit(second.clone()).unwrap();
    until(|| seen(&sink, &second.kota_id, DeliveryState::Accepted));
    let (tx1, rx1) = mpsc::sync_channel(1); let (tx2, rx2) = mpsc::sync_channel(1);
    runtime.inner.controls.send(Control::QueueAction(json!({"type":"queue_cancel","kotaId":first.kota_id}), tx1)).unwrap();
    until(|| root.join("cancel-started").exists());
    runtime.inner.controls.send(Control::QueueAction(json!({"type":"queue_cancel","kotaId":second.kota_id}), tx2)).unwrap();
    // An owner-memory round trip proves the second action was queued while Pi
    // is still blocked in the first handler; it must not return a busy error.
    runtime.queue_action(json!({"type":"queue_edit_valid","kotaId":second.kota_id,"editToken":"none"})).unwrap();
    assert!(rx2.try_recv().is_err());
    fs::write(root.join("cancel-release"), "").unwrap();
    assert_eq!(rx1.recv_timeout(Duration::from_secs(5)).unwrap().unwrap()["cancelled"], true);
    assert_eq!(rx2.recv_timeout(Duration::from_secs(5)).unwrap().unwrap()["cancelled"], true);
    let order = fs::read_to_string(root.join("cancelled.jsonl")).unwrap();
    assert_eq!(order.lines().collect::<Vec<_>>(), vec![first.kota_id, second.kota_id]);
    runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
}

#[test]
fn stop_fails_waiting_removals_instead_of_reporting_them_as_cancelled() {
    let root = root(); let sink = capture();
    let runtime = LocalRuntime::start(config(&root, 1, true), sink.clone()).unwrap(); runtime.activate(false).unwrap();
    let first = fixture("hold-cancel-barrier"); let second = fixture("hold-second");
    runtime.submit(first.clone()).unwrap(); runtime.submit(second.clone()).unwrap();
    until(|| seen(&sink, &second.kota_id, DeliveryState::Accepted));
    let (tx1, _rx1) = mpsc::sync_channel(1); let (tx2, rx2) = mpsc::sync_channel(1); let (stop_tx, stop_rx) = mpsc::sync_channel(1);
    runtime.inner.controls.send(Control::QueueAction(json!({"type":"queue_cancel","kotaId":first.kota_id}), tx1)).unwrap();
    until(|| root.join("cancel-started").exists());
    runtime.inner.controls.send(Control::QueueAction(json!({"type":"queue_cancel","kotaId":second.kota_id}), tx2)).unwrap();
    runtime.inner.controls.send(Control::Stop(stop_tx)).unwrap();
    assert!(rx2.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err().contains("Stop"));
    fs::write(root.join("cancel-release"), "").unwrap(); stop_rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    assert_eq!(fs::read_to_string(root.join("cancelled.jsonl")).unwrap().lines().count(), 1);
    runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
}

#[test]
fn outstanding_bytes_hold_the_next_message_until_credit_is_released() {
    let root = root(); let sink = capture();
    let runtime = LocalRuntime::start(config(&root, 1, true), sink.clone()).unwrap(); runtime.activate(false).unwrap();
    for index in 0..4 {
        let mut input = fixture(&format!("hold-bytes-{index}"));
        input.content = Value::String("x".repeat(10 * 1024 * 1024)); runtime.submit(input).unwrap();
    }
    until(|| received(&root) == 3); runtime.rpc(json!({"type":"get_state"})).unwrap(); assert_eq!(received(&root), 3);
    let first = fixture("hold-bytes-0"); runtime.queue_action(json!({"type":"queue_cancel","kotaId":first.kota_id})).unwrap();
    until(|| received(&root) == 4); runtime.rpc(json!({"type":"abort"})).unwrap();
    runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
}

#[test]
fn abort_stage_timeout_fails_immediately_without_waiting_for_later_stop_stages() {
    let root=root();let sink=capture();fs::write(root.join("missing-abort"),"").unwrap();
    let runtime=LocalRuntime::start(config(&root,1,true),sink.clone()).unwrap();runtime.activate(false).unwrap();
    let a=fixture("hold-timeout");runtime.submit(a.clone()).unwrap();until(||seen(&sink,&a.kota_id,DeliveryState::Accepted));
    let start=Instant::now();let result=runtime.rpc(json!({"type":"abort"}));
    assert!(result.unwrap_err().to_string().contains("timed out"));
    assert!(start.elapsed() >= COMMAND_TIMEOUT);
    assert!(start.elapsed() < COMMAND_TIMEOUT + Duration::from_secs(5),"failure must not wait for clear_queue/resume budgets");
    until(||runtime.inner_join_finished_for_test());
    assert_eq!(runtime.snapshot().state,OnlineState::Error);
    assert!(seen(&sink,&a.kota_id,DeliveryState::Abandoned));
    assert_eq!(fs::read_to_string(root.join("stop-order.jsonl")).unwrap(),"pause\nabort\n");
    runtime.close().unwrap();fs::remove_dir_all(root).unwrap();
}

#[test]
fn edit_already_in_flight_can_requeue_during_stop_without_clearing_the_new_sequence() {
    let root=root();let sink=capture();let runtime=LocalRuntime::start(config(&root,1,true),sink.clone()).unwrap();runtime.activate(false).unwrap();
    let a=fixture("hold-cancel-barrier");let b=fixture("hold-barrier");
    runtime.submit(a.clone()).unwrap();runtime.submit(b.clone()).unwrap();until(||seen(&sink,&b.kota_id,DeliveryState::Accepted));
    let copy=runtime.clone();let id=a.kota_id.clone();let editing=thread::spawn(move||copy.queue_action(json!({"type":"queue_cancel","kotaId":id,"edit":true})));
    until(||root.join("cancel-started").exists());
    let (reply,done)=mpsc::sync_channel(1);runtime.inner.controls.send(Control::Stop(reply)).unwrap();
    // Owner-memory round trip puts Stop before the pending cancel completes.
    runtime.queue_action(json!({"type":"queue_edit_valid"})).unwrap();
    fs::write(root.join("cancel-release"),"").unwrap();let edit=editing.join().unwrap().unwrap();assert_eq!(edit["valid"],true);
    until(||root.join("stop-started").exists());
    runtime.queue_action(json!({"type":"queue_requeue","kotaId":a.kota_id,"editToken":edit["editToken"]})).unwrap();
    fs::write(root.join("stop-release"),"").unwrap();done.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    until(||received(&root)==3);
    assert!(seen(&sink,&b.kota_id,DeliveryState::Consumed));
    assert_eq!(runtime.snapshot().queued,1,"new A remains queued after resumed B");
    let view=runtime.workbench(None);assert_eq!(view.queue.unwrap()[0].kota_id,a.kota_id);
    runtime.close().unwrap();fs::remove_dir_all(root).unwrap();
}


#[test]
fn reorder_ack_changes_view_new_add_stays_tail_and_remove_waits_in_fifo() {
    let root=root();let sink=capture();let runtime=LocalRuntime::start(config(&root,1,true),sink.clone()).unwrap();runtime.activate(false).unwrap();
    let inputs=[fixture("hold-A"),fixture("hold-B"),fixture("hold-C")];
    for input in &inputs { runtime.submit(input.clone()).unwrap(); }
    until(||seen(&sink,&inputs[2].kota_id,DeliveryState::Accepted));
    fs::write(root.join("reorder-barrier"),"").unwrap();
    let ids=vec![inputs[2].kota_id.clone(),inputs[0].kota_id.clone(),inputs[1].kota_id.clone()];
    let copy=runtime.clone();let wanted=ids.clone();let reorder=thread::spawn(move||copy.queue_action(json!({"type":"queue_reorder","ids":wanted})));
    until(||root.join("reorder-started").exists());
    let d=fixture("hold-D");runtime.submit(d.clone()).unwrap();
    assert_eq!(received(&root),3,"pump stays shut before ACK");
    let (tx,rx)=mpsc::sync_channel(1);runtime.inner.controls.send(Control::QueueAction(json!({"type":"queue_cancel","kotaId":inputs[0].kota_id}),tx)).unwrap();
    runtime.queue_action(json!({"type":"queue_edit_valid"})).unwrap();assert!(rx.try_recv().is_err());
    fs::write(root.join("reorder-release"),"").unwrap();
    assert_eq!(reorder.join().unwrap().unwrap()["reordered"],true);
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap()["cancelled"],true);
    until(||seen(&sink,&d.kota_id,DeliveryState::Accepted));
    assert_eq!(runtime.workbench(None).queue.unwrap().iter().map(|e|e.kota_id.clone()).collect::<Vec<_>>(),vec![ids[0].clone(),ids[2].clone(),d.kota_id]);
    runtime.close().unwrap();fs::remove_dir_all(root).unwrap();
}

#[test]
fn reorder_mismatch_is_controlled_but_unexpected_or_lost_ack_stops_owner_without_replay() {
    for mode in ["mismatch","reorder-error","reorder-silent"] {
        let root=root();let sink=capture();let runtime=LocalRuntime::start(config(&root,1,true),sink.clone()).unwrap();runtime.activate(false).unwrap();
        let inputs=[fixture("hold-A"),fixture("hold-B"),fixture("hold-C")];
        for input in &inputs { runtime.submit(input.clone()).unwrap(); }
        until(||seen(&sink,&inputs[2].kota_id,DeliveryState::Accepted));
        let ids=if mode=="mismatch" {vec![inputs[1].kota_id.clone(),inputs[0].kota_id.clone()]} else { fs::write(root.join(mode),"").unwrap();inputs.iter().rev().map(|x|x.kota_id.clone()).collect() };
        let result=runtime.queue_action(json!({"type":"queue_reorder","ids":ids}));assert!(result.is_err());
        if mode=="reorder-silent" {assert!(result.as_ref().unwrap_err().to_string().contains("command timed out"));}
        if mode=="mismatch" {assert_eq!(result.unwrap_err().to_string(),"queue_changed");assert!(runtime.snapshot().child_pid.is_some());}
        else {until(||runtime.snapshot().child_pid.is_none());assert!(inputs.iter().all(|i|seen(&sink,&i.kota_id,DeliveryState::Abandoned)));}
        assert!(!sink.notices.lock().unwrap().iter().any(|notice|matches!(notice,LocalNotice::Event {event,..} if event.to_string().contains("kota-reorder"))));
        runtime.close().unwrap();
        let reload=LocalRuntime::start(config(&root,2,true),capture()).unwrap();reload.activate(false).unwrap();reload.rpc(json!({"type":"get_state"})).unwrap();assert_eq!(received(&root),3);
        reload.close().unwrap();fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn stop_after_reorder_send_keeps_held_and_consumes_it_in_the_acknowledged_order() {
    let root=root();let sink=capture();let runtime=LocalRuntime::start(config(&root,1,true),sink.clone()).unwrap();runtime.activate(false).unwrap();
    let inputs=[fixture("hold-A"),fixture("hold-B")];for input in &inputs {runtime.submit(input.clone()).unwrap();}
    until(||seen(&sink,&inputs[1].kota_id,DeliveryState::Accepted));
    fs::write(root.join("reorder-barrier"),"").unwrap();let copy=runtime.clone();let ids=vec![inputs[1].kota_id.clone(),inputs[0].kota_id.clone()];let wanted=ids.clone();
    let reordered=thread::spawn(move||copy.queue_action(json!({"type":"queue_reorder","ids":wanted})));
    until(||root.join("reorder-started").exists());let (tx,rx)=mpsc::sync_channel(1);runtime.inner.controls.send(Control::Stop(tx)).unwrap();
    runtime.queue_action(json!({"type":"queue_edit_valid"})).unwrap();fs::write(root.join("reorder-release"),"").unwrap();
    assert_eq!(reordered.join().unwrap().unwrap_err().to_string(),"queue_changed");rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    until(||inputs.iter().all(|i|seen(&sink,&i.kota_id,DeliveryState::Consumed)));
    let entries:Value=serde_json::from_str(&fs::read_to_string(root.join("entries.json")).unwrap()).unwrap();
    assert_eq!(entries.as_array().unwrap().iter().map(|e|e["details"]["kotaId"].as_str().unwrap().to_owned()).collect::<Vec<_>>(),ids);
    runtime.close().unwrap();fs::remove_dir_all(root).unwrap();
}


#[test]
fn reorder_waits_for_the_inflight_injection_and_stop_cancels_that_wait() {
    for stop in [false,true] {
        let root=root();let sink=capture();let runtime=LocalRuntime::start(config(&root,1,true),sink.clone()).unwrap();runtime.activate(false).unwrap();
        let a=fixture("hold-A");let b=fixture("hold-inject-barrier");runtime.submit(a.clone()).unwrap();until(||seen(&sink,&a.kota_id,DeliveryState::Accepted));
        runtime.submit(b.clone()).unwrap();until(||root.join("inject-started").exists());
        let (tx,rx)=mpsc::sync_channel(1);runtime.inner.controls.send(Control::QueueAction(json!({"type":"queue_reorder","ids":[b.kota_id,a.kota_id]}),tx)).unwrap();
        runtime.queue_action(json!({"type":"queue_edit_valid"})).unwrap();assert!(!root.join("reorder-started").exists());assert!(rx.try_recv().is_err());
        let (stop_tx,stop_rx)=mpsc::sync_channel(1);if stop {runtime.inner.controls.send(Control::Stop(stop_tx)).unwrap();assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err(),"queue_changed");}
        fs::write(root.join("inject-release"),"").unwrap();
        if stop {stop_rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();assert!(!root.join("reorder-started").exists());}
        else {assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap()["reordered"],true);assert_eq!(runtime.workbench(None).queue.unwrap()[0].kota_id,b.kota_id);}
        runtime.close().unwrap();fs::remove_dir_all(root).unwrap();
    }
}
