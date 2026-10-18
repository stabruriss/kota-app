use super::*;
use std::{os::unix::fs::PermissionsExt, thread};

struct Fixture {
    root: PathBuf,
    clones: Clones,
}
impl Fixture {
    fn new(hold: bool) -> Self {
        let root = std::env::temp_dir().join(format!("shadow-clone-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("pi-fixture");
        fs::write(&executable,r#"#!/usr/bin/python3
import sys,json,os,time
a=sys.argv[1:]
def arg(n): return a[a.index(n)+1]
session=arg('--session-id'); directory=arg('--session-dir'); model=arg('--model').split('/',1)
with open(directory+'/argv.json','w') as f: json.dump(a,f)
def emit(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 r=json.loads(line)
 if r['type']=='get_state': emit({'type':'response','id':r['id'],'success':True,'data':{'sessionId':session,'model':{'provider':model[0],'id':model[1]},'thinkingLevel':arg('--thinking'),'isStreaming':False}})
 elif r['type']=='prompt':
  with open(directory+'/history.jsonl','a') as f: f.write(json.dumps({'message':r['message']})+'\n')
  emit({'type':'response','id':r['id'],'success':True,'data':{'disposition':'submitted'}})
  emit({'type':'agent_start'})
  if os.environ.get('FIXTURE_HOLD')=='1':
   while True: time.sleep(.05)
  emit({'type':'message_end','message':{'role':'assistant','stopReason':'stop','content':[{'type':'text','text':'Fixture reply: '+r['message']}]}})
  emit({'type':'agent_settled'})
"#).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = Command::new(executable);
        command
            .current_dir(&root)
            .env_clear()
            .env("FIXTURE_HOLD", if hold { "1" } else { "0" })
            .env("PI_CODING_AGENT_DIR", root.join("owned"));
        let scope = LocalScope {
            project_root: root.clone(),
            agent_id: "fixture-parent".into(),
            session_id: "parent-session".into(),
            epoch: 1,
        };
        let clones = Clones::new(&command, &scope, Arc::new(Mutex::new(BTreeMap::new())));
        Self { root, clones }
    }
    fn action(&mut self, value: Value) -> Value {
        self.clones
            .action(
                serde_json::from_value(value).unwrap(),
                Some(super::super::models::ModelSelection {
                    provider_id: "fixture".into(),
                    model_id: "concrete".into(),
                }),
                "high",
            )
            .unwrap()
    }
    fn finish(&mut self, count: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut reports = Vec::new();
        while reports.len() < count {
            assert!(Instant::now() < deadline);
            reports.extend(self.clones.tick().into_iter().map(|(_, report)| report));
            thread::sleep(Duration::from_millis(5));
        }
        reports
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.clones.stop_all();
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn copies_are_parallel_bounded_and_read_only_has_only_native_file_tools() {
    let mut f = Fixture::new(true);
    let tasks = (0..7)
        .map(|i| json!({"task":format!("task {i}"),"read_only":true,"timeout_minutes":0}))
        .collect::<Vec<_>>();
    assert_eq!(
        f.action(json!({"action":"create","tasks":tasks}))["clone_ids"],
        json!([1, 2, 3, 4, 5, 6, 7])
    );
    let error = f
        .clones
        .action(
            Request::Create {
                tasks: vec![Task {
                    task: "overflow".into(),
                    read_only: false,
                    timeout_minutes: 90.0,
                }],
            },
            None,
            "off",
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Too many clones running: at most 7 at once."
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !(1..=7).all(|id| {
        f.root
            .join(format!("under-table/clone-{id}/history.jsonl"))
            .exists()
    }) {
        assert!(Instant::now() < deadline);
        assert!(f.clones.tick().is_empty());
        thread::sleep(Duration::from_millis(5));
    }
    for id in 1..=7 {
        let args: Vec<String> = serde_json::from_slice(
            &fs::read(f.root.join(format!("under-table/clone-{id}/argv.json"))).unwrap(),
        )
        .unwrap();
        for flag in ["-nc", "-ns", "-ne", "--offline"] {
            assert!(args.iter().any(|arg| arg == flag));
        }
        assert_eq!(
            args[args.iter().position(|arg| arg == "--tools").unwrap() + 1],
            "read,grep,find,ls"
        );
        assert!(!args.iter().any(|arg| arg == "-e" || arg == "--skill"));
        assert_eq!(
            args[args.iter().position(|arg| arg == "--model").unwrap() + 1],
            "fixture/concrete"
        );
        assert_eq!(
            args[args.iter().position(|arg| arg == "--thinking").unwrap() + 1],
            "high"
        );
    }
    assert_eq!(f.clones.running(), 7);
    f.clones.stop_all();
    assert_eq!(f.clones.running(), 0);
    assert!(read(&f.clones.records, None)["clones"]
        .as_array()
        .unwrap()
        .iter()
        .all(|clone| clone["state"] == "stopped"));
}

#[test]
fn all_five_actions_keep_history_until_destroy_and_numbers_follow_existing_directories() {
    let mut f = Fixture::new(false);
    fs::create_dir_all(f.root.join("under-table/clone-8")).unwrap();
    assert_eq!(
        f.action(json!({"action":"create","tasks":[{"task":"first"}]}))["clone_ids"],
        json!([9])
    );
    assert_eq!(f.finish(1), ["Clone #9 finished:\nFixture reply: first"]);
    assert_eq!(
        f.action(json!({"action":"status"}))["clones"][0]["state"],
        "finished"
    );
    f.action(json!({"action":"ask","clone_id":9,"message":"second"}));
    assert_eq!(f.finish(1), ["Clone #9 finished:\nFixture reply: second"]);
    let directory = f.root.join("under-table/clone-9");
    assert_eq!(
        fs::read_to_string(directory.join("history.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(
        read(&f.clones.records, Some(9))["clones"][0]["view"]["blocks"][0]["text"],
        "Fixture reply: second"
    );
    assert_eq!(
        f.action(json!({"action":"stop","clone_id":9}))["message"],
        "Clone #9 stopped."
    );
    assert!(directory.exists());
    assert_eq!(
        f.action(json!({"action":"destroy","clone_id":9}))["message"],
        "Clone #9 was destroyed."
    );
    assert!(!directory.exists());
    assert!(f.root.join("under-table/clone-8").is_dir());
    assert!(read(&f.clones.records, None)["clones"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn timeout_stops_only_its_copy_and_keeps_records_for_ask() {
    let mut f = Fixture::new(true);
    f.action(json!({"action":"create","tasks":[{"task":"limited","timeout_minutes":0.001},{"task":"unlimited","timeout_minutes":0}]}));
    let reports = f.finish(1);
    assert_eq!(
        reports,
        ["Clone #1 timed out after 0.001 minutes. It can still be asked."]
    );
    assert_eq!(f.clones.state(1).unwrap(), State::Stopped);
    assert_eq!(f.clones.state(2).unwrap(), State::Running);
    assert!(f.root.join("under-table/clone-1").exists());
}

#[test]
fn reports_are_utf8_bounded_with_exact_user_wording_and_no_extra_prompt() {
    assert_eq!(report(1, "reply"), "Clone #1 finished:\nreply");
    let text = "字".repeat(20 * 1024);
    let value = report(7, &text);
    assert!(value.len() <= REPORT_LIMIT);
    assert!(value.starts_with("Clone #7 finished:\n"));
    assert!(value.ends_with("\nThe full record is in under-table/clone-7/."));
    assert_eq!(default_timeout(), 90.0);
    assert!(timeout(0.0).unwrap().is_none());
    for value in [f64::NAN, f64::INFINITY, -1.0] {
        assert!(timeout(value).is_err());
    }
    assert!(serde_json::from_value::<Request>(
        json!({"action":"create","tasks":[{"task":"task","extra":true}]})
    )
    .is_err());
}
