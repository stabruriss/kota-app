use super::*;
use crate::{
    agent_directory::tests::Account,
    bbs_sync::transport::{Cancellation, FileIo, Limits},
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
mod remote;
mod runtime_tests;
mod safety;

#[test]
fn human_commands_emit_actual_safe_errors_and_durable_bridge_fixture() {
    use crate::bbs_sync::roster;
    let b = Board::new();
    let identity = human_identity("source", "Human".into(), None);
    let private = b.post("Existing private thread", &[]);
    let remote = mentions::Mention::parse(&format!("{}/remote/agent", "a".repeat(64))).unwrap();
    let new_request = |target: mentions::Mention| {
        serde_json::from_value::<BbsHumanPostRequest>(
        json!({"projectId":"source","body":"Original human input","projectTags":["source"],"mentions":[target]})
    ).unwrap()
    };
    let before = fs::read_dir(b.content.root().join("threads"))
        .unwrap()
        .count();
    let unjoined = human_post_scoped(&b.content, &identity, new_request(remote)).unwrap_err();
    let (state, device, peer) = roster::tests::joined(&b.account);
    let remote = mentions::Mention::parse(&format!("{peer}/remote/agent")).unwrap();
    let unsynced =
        human_post_scoped(&b.content, &identity, new_request(remote.clone())).unwrap_err();
    let cache = roster::PeerRoster {
        schema_version: 1,
        group_id: "group-a".into(),
        device_id: peer.clone(),
        membership_id: "peer-membership".into(),
        version: roster::version(&[]).unwrap(),
        received_at: now_iso(),
        projects: vec![],
    };
    fs::write(
        state
            .identity_path()
            .parent()
            .unwrap()
            .join("rosters")
            .join(format!("{peer}.json")),
        serde_json::to_vec(&cache).unwrap(),
    )
    .unwrap();
    let reply: BbsHumanReplyRequest = serde_json::from_value(json!({
        "projectId":"source","threadId":private,"body":"Must not post", "mentions":[remote]
    }))
    .unwrap();
    let private_error = human_reply_scoped(&b.content, &identity, reply).unwrap_err();
    let errors = json!({"unjoined":unjoined,"notSynced":unsynced,"privateThread":private_error});
    assert_eq!(
        errors,
        json!({
            "unjoined":{"code":"mention_requires_group"},
            "notSynced":{"code":"agent_roster_not_synced"},
            "privateThread":{"code":"mention_thread_not_shared"}
        })
    );
    assert_eq!(
        fs::read_dir(b.content.root().join("threads"))
            .unwrap()
            .count(),
        before
    );
    assert_eq!(load_thread(b.content.root(), &private).unwrap().1.len(), 1);
    assert!(!b.queue.root().exists());
    let request_json = json!({"projectId":"source","body":"Original human input","projectTags":["source"],"mentions":[b.target()]});
    let request = serde_json::from_value(request_json.clone()).unwrap();
    let thread = human_post_scoped(&b.content, &identity, request).unwrap();
    let (_, posts) = load_thread(b.content.root(), &thread).unwrap();
    assert_eq!(posts[0].body, "@Receiver\n\nOriginal human input");
    assert_eq!(posts[0].meta.mentions[0].device_id, device);
    let key = b.keys("pending")[0].clone();
    let sink = RecordingSink::default();
    delivery::dispatch(&b.queue, claim(&b.queue, &key, "human", now_ms()), &sink).unwrap();
    let calls = sink.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["eventId"], format!("bbs-mention:{key}"));
    println!(
        "BBS_MENTION_FIXTURE={}",
        json!({
            "errors":errors,"request":request_json,"threadId":thread,
            "postId":posts[0].meta.post_id,"body":posts[0].body,"mentions":posts[0].meta.mentions,
            "delivery":calls[0],"processedCount":b.keys("processed").len()
        })
    );
}

struct Board {
    account: Account,
    content: sync::ContentStore,
    queue: store::Queue,
}
impl Board {
    fn new() -> Self {
        let account = Account::new();
        account.project("source", false);
        account.project("target", false);
        account.agent("target", "agent", "display-name: Receiver\n");
        let content = sync::ContentStore::at(account.0.join("Workspaces/bbs"), account.0.clone());
        content.prepare_layout().unwrap();
        let queue = store::Queue::new(&content);
        Self {
            account,
            content,
            queue,
        }
    }
    fn target(&self) -> mentions::Mention {
        mentions::Mention::parse("local/target/agent").unwrap()
    }
    fn post(&self, body: &str, targets: &[mentions::Mention]) -> String {
        create_thread_mentions_scoped(
            &self.content,
            &human_identity("source", "Human".into(), None),
            vec!["source".into()],
            false,
            body.into(),
            &[],
            targets,
        )
        .unwrap()
    }
    fn keys(&self, phase: &str) -> Vec<String> {
        let mut keys = fs::read_dir(self.queue.root().join(phase))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
            .filter(|k| bbs_sync::valid_hash(k).is_ok())
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }
    fn record(&self, key: &str) -> Record {
        serde_json::from_slice(&fs::read(self.queue.path("pending", key).unwrap()).unwrap())
            .unwrap()
    }
}
#[derive(Default)]
struct RecordingSink {
    calls: Mutex<Vec<Value>>,
    bus: crate::agent_bus::AgentBusManager,
    fail_after_record: std::sync::atomic::AtomicBool,
}
impl delivery::Sink for RecordingSink {
    fn submit(&self, envelope: delivery::Envelope) -> Result<()> {
        let persisted = self.bus.persist_bbs_notice_for_test(
            &envelope.project.root,
            envelope.notice(),
            &envelope.event_id,
        )?;
        if persisted.recorded {
            let r = envelope.request();
            self.calls.lock().unwrap().push(json!({"projectId":envelope.project.id,"target":r.target,
                "senderAgentId":r.sender_agent_id,"intent":r.intent,"text":r.text,"eventId":r.event_id,
                "dedupeKey":r.dedupe_key,"wake":envelope.agent.is_some()}));
        }
        if self
            .fail_after_record
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            bail!("simulated_receipt_gap");
        }
        Ok(())
    }
}
fn claim(queue: &store::Queue, key: &str, run: &str, now: i64) -> store::Claim {
    match queue.check(key, run, now).unwrap() {
        store::Check::Deliver(claim) => claim,
        _ => panic!("expected delivery"),
    }
}

#[test]
fn actual_human_publication_queues_once_without_identity_and_never_parses_plain_at_text() {
    let b = Board::new();
    b.post("@Receiver is plain text", &[]);
    assert!(!b.queue.root().exists());
    let request: BbsHumanPostRequest =
        serde_json::from_value(json!({"projectId":"source", "body":"Original human input",
        "projectTags":["source"],"mentions":[b.target(),b.target()]}))
        .unwrap();
    let thread = human_post_scoped(
        &b.content,
        &human_identity("source", "Human".into(), None),
        request,
    )
    .unwrap();
    let (_, posts) = load_thread(b.content.root(), &thread).unwrap();
    assert_eq!(posts[0].body, "@Receiver\n\nOriginal human input");
    assert_eq!(posts[0].meta.mentions, vec![b.target()]);
    assert_eq!(b.keys("pending").len(), 1);
    let key = &b.keys("pending")[0];
    let pending = b.record(key);
    assert!(pending.ready && pending.proof.is_some());
    assert_eq!(pending.destination.key(), *key);
    let sink = RecordingSink::default();
    delivery::dispatch(&b.queue, claim(&b.queue, key, "first", now_ms()), &sink).unwrap();
    assert!(matches!(
        b.queue.check(key, "restart", now_ms()).unwrap(),
        store::Check::Done
    ));
    let calls = sink.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["projectId"], "target");
    assert_eq!(calls[0]["senderAgentId"], "bbs");
    assert_eq!(calls[0]["intent"], "bbs-thread");
    assert!(calls[0]["text"]
        .as_str()
        .unwrap()
        .contains("Original human input"));
    assert!(calls[0]["text"]
        .as_str()
        .unwrap()
        .contains("local account user"));
    assert!(!b.content.state.identity_path().exists());
    assert!(!b.content.state.control_path().exists());
}

#[test]
fn processing_claim_is_exclusive_and_crash_after_real_bus_record_dedupes_on_restart() {
    let b = Board::new();
    b.post("record then crash", &[b.target()]);
    let key = &b.keys("pending")[0];
    let first = claim(&b.queue, key, "run-one", now_ms());
    assert!(matches!(
        b.queue.check(key, "other-app", now_ms()).unwrap(),
        store::Check::Done
    ));
    let sink = RecordingSink::default();
    sink.fail_after_record
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(delivery::dispatch(&b.queue, first, &sink).is_err());
    assert!(matches!(
        b.queue.check(key, "run-one", now_ms()).unwrap(),
        store::Check::Done
    ));
    delivery::dispatch(&b.queue, claim(&b.queue, key, "restart", now_ms()), &sink).unwrap();
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    assert_eq!(b.keys("processed"), vec![key.clone()]);
}

#[tokio::test]
async fn awaiting_post_recovers_only_its_published_hash_and_keeps_original_deadline() {
    let b = Board::new();
    let thread = b.post("durable body before ready", &[b.target()]);
    let key = b.keys("pending")[0].clone();
    let mut record = b.record(&key);
    let original_deadline = record.deadline_ms;
    record.ready = false;
    record.proof = None;
    fs::write(
        b.queue.path("pending", &key).unwrap(),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        b.queue.check(&key, "restart", now_ms()).unwrap(),
        store::Check::Recover
    ));
    let files = FileIo::start(Limits::default()).unwrap();
    let queue = b.queue.clone();
    let k = key.clone();
    files
        .run_when_available(&Cancellation::default(), move |io| queue.recover(&k, io))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.record(&key).deadline_ms, original_deadline);
    assert!(b.record(&key).ready);
    let sink = RecordingSink::default();
    delivery::dispatch(&b.queue, claim(&b.queue, &key, "restart", now_ms()), &sink).unwrap();
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    assert!(render_thread_scoped(&b.content, &thread)
        .unwrap()
        .contains("durable body before ready"));
    files.stop_and_wait().await;
}

#[test]
fn missing_agent_has_one_room_notice_missing_project_has_none_and_large_body_is_a_pointer() {
    let b = Board::new();
    b.post(&"large original. ".repeat(2000), &[b.target()]);
    let key = b.keys("pending")[0].clone();
    b.account.agent(
        "target",
        "agent",
        "display-name: Receiver\nstatus: archived\n",
    );
    let sink = RecordingSink::default();
    delivery::dispatch(&b.queue, claim(&b.queue, &key, "run", now_ms()), &sink).unwrap();
    let calls = sink.calls.lock().unwrap();
    assert_eq!(calls[0]["wake"], false);
    assert!(calls[0]["text"]
        .as_str()
        .unwrap()
        .contains("Post body omitted from this notice because it exceeds 16 KiB."));
    assert!(!calls[0]["text"]
        .as_str()
        .unwrap()
        .contains("large original."));
    drop(calls);
    b.post("project archived", &[b.target()]);
    let key = b.keys("pending")[0].clone();
    b.account.project("target", true);
    delivery::dispatch(&b.queue, claim(&b.queue, &key, "run", now_ms()), &sink).unwrap();
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
}
