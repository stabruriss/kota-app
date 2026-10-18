use super::*;
use crate::local_rpc::shadow_clone::{Request, Task};

#[test]
#[ignore = "requires pinned bundled Pi; isolated HOME and loopback fake provider only"]
fn real_pi_clone_continues_own_history_and_reports_once_to_the_parent_queue() {
    let f = Fixture::new(
        json!([
            {"type":"tool","name":"read","arguments":{"path":"fixture.txt"}},
            {"type":"text","text":"Copy final reply."},
            {"type":"text","text":"Parent received copy report."},
            {"type":"text","text":"Copy follow-up reply."},
            {"type":"text","text":"Parent received follow-up."}
        ]),
        false,
    );
    let sink = Arc::new(Capture::default());
    let runtime = LocalRuntime::start(f.config(1, SESSION), sink.clone()).unwrap();
    runtime.activate(false).unwrap();
    let created = runtime
        .clone_control(Request::Create {
            tasks: vec![Task {
                task: "Read fixture.txt and report.".into(),
                read_only: true,
                timeout_minutes: 90.0,
            }],
        })
        .unwrap();
    assert_eq!(created["clone_ids"], json!([1]));
    until(|| {
        f.requests() == 3
            && runtime.snapshot().state == OnlineState::Ready
            && runtime
                .workbench(None)
                .blocks
                .unwrap()
                .iter()
                .any(|block| block.text.contains("Parent received copy report."))
    });
    until(|| runtime.clones(Some(1))["clones"][0]["state"] == "finished");
    let copy = f.root.join("workspace/under-table/clone-1");
    let files = || {
        fs::read_dir(&copy)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
            .map(|entry| entry.path())
            .collect::<Vec<_>>()
    };
    until(|| files().len() == 1);
    let native = fs::read_to_string(&files()[0]).unwrap();
    assert!(native
        .lines()
        .any(|line| serde_json::from_str::<Value>(line).unwrap()["id"] == "clone-1"));
    assert!(native.contains("Harmless read-only fixture."));
    assert!(!native.contains("kota.clone.v1:") && !native.contains("kota_shadow_clone"));
    let reports = || {
        sink.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|notice| {
                if let LocalNotice::Entries { entries, .. } = notice {
                    Some(entries.clone())
                } else {
                    None
                }
            })
            .flatten()
            .filter(|entry| entry["customType"] == "kota.shadow_clone")
            .collect::<Vec<_>>()
    };
    until(|| {
        reports()
            .iter()
            .any(|entry| entry["content"] == "Clone #1 finished:\nCopy final reply.")
    });
    let report_ids = reports()
        .iter()
        .filter_map(|entry| entry["details"]["kotaId"].as_str().map(str::to_owned))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(report_ids.len(), 1);
    runtime
        .clone_control(Request::Ask {
            clone_id: 1,
            message: "Continue with your earlier read.".into(),
            timeout_minutes: None,
        })
        .unwrap();
    until(|| {
        f.requests() == 5
            && runtime.snapshot().state == OnlineState::Ready
            && runtime.clones(None)["clones"][0]["state"] == "finished"
            && runtime
                .workbench(None)
                .blocks
                .unwrap()
                .iter()
                .any(|block| block.text.contains("Parent received follow-up."))
    });
    let continued = fs::read_to_string(&files()[0]).unwrap();
    assert!(continued.contains("Copy final reply.") && continued.contains("Copy follow-up reply."));
    assert_eq!(
        files().len(),
        1,
        "ask resumes the exact existing clone session"
    );
    runtime
        .clone_control(Request::Stop { clone_id: 1 })
        .unwrap();
    assert!(copy.is_dir());
    runtime
        .clone_control(Request::Destroy { clone_id: 1 })
        .unwrap();
    assert!(!copy.exists());
    assert_eq!(runtime.snapshot().state, OnlineState::Ready);
    runtime.close().unwrap();
}

#[test]
#[ignore = "requires pinned bundled Pi; actual tool/UI channel and loopback fake provider only"]
fn real_pi_thin_tool_returns_before_copy_finishes_and_busy_parent_consumes_its_report_once() {
    let f = Fixture::new(
        json!([
            {"type":"tool","name":"kota_shadow_clone","arguments":{"action":"create","tasks":[{"task":"Synthetic independent copy task","read_only":true}]}},
            {"type":"text","text":"Parent continues without waiting.","holdBeforeHeaders":true},
            {"type":"text","text":"Background copy result."},
            {"type":"text","text":"Parent consumes the queued copy report."}
        ]),
        false,
    );
    let sink = Arc::new(Capture::default());
    let runtime = LocalRuntime::start(f.config(1, SESSION), sink.clone()).unwrap();
    runtime.activate(false).unwrap();
    let input = fixture("parent-tool-create");
    runtime.submit(input.clone()).unwrap();
    until(|| {
        f.requests() == 3
            && runtime.clones(None)["clones"]
                .as_array()
                .is_some_and(|rows| rows.first().is_some_and(|row| row["state"] == "finished"))
    });
    assert_eq!(runtime.snapshot().state, OnlineState::Busy);
    let tool_returned=sink.0.lock().unwrap().iter().any(|notice|matches!(notice,LocalNotice::Event {event,..}
        if event["type"]=="tool_execution_end" && event["toolName"]=="kota_shadow_clone" && event["result"]["details"]["clone_ids"]==json!([1])));
    assert!(
        tool_returned,
        "create returns the ID while the parent is still working"
    );
    // The clone record becomes Finished before its process is reaped and the
    // report is handed to the parent ledger. Observe that queue boundary too.
    until(|| runtime.workbench(None).queue.unwrap()
        .iter().any(|entry| entry.custom_type == "kota.shadow_clone"));
    f.release(2);
    until(|| {
        f.requests() == 4
            && runtime.snapshot().state == OnlineState::Ready
            && runtime.workbench(None).blocks.unwrap().iter().any(|block| {
                block
                    .text
                    .contains("Parent consumes the queued copy report.")
            })
    });
    let entries: Vec<_> = sink
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|notice| {
            if let LocalNotice::Entries { entries, .. } = notice {
                Some(entries.clone())
            } else {
                None
            }
        })
        .flatten()
        .collect();
    let ids = entries
        .iter()
        .filter(|entry| entry["customType"] == "kota.shadow_clone")
        .filter_map(|entry| entry["details"]["kotaId"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 1);
    assert!(!serde_json::to_string(&*sink.0.lock().unwrap())
        .unwrap()
        .contains("kota.clone.v1:"));
    assert!(!fs::read_to_string(f.root.join("ledger.jsonl"))
        .unwrap()
        .contains("kota.clone.v1:"));
    for file in fs::read_dir(f.root.join("sessions")).unwrap().flatten() {
        if file.path().extension().is_some_and(|ext| ext == "jsonl") {
            assert!(!fs::read_to_string(file.path())
                .unwrap()
                .contains("kota.clone.v1:"));
        }
    }
    runtime.close().unwrap();
}

#[test]
#[ignore = "requires pinned bundled Pi; loopback fake provider only"]
fn real_pi_parent_natural_finish_keeps_copy_but_human_stop_and_exit_stop_it() {
    let f = Fixture::new(
        json!([
            {"type":"text","text":"copy remains running","holdBeforeHeaders":true},
            {"type":"text","text":"Parent naturally finished."},
            {"type":"text","text":"second copy remains running","holdBeforeHeaders":true}
        ]),
        false,
    );
    let sink = Arc::new(Capture::default());
    let runtime = LocalRuntime::start(f.config(1, SESSION), sink.clone()).unwrap();
    runtime.activate(false).unwrap();
    let task = || Request::Create {
        tasks: vec![Task {
            task: "Keep working on this task.".into(),
            read_only: true,
            timeout_minutes: 0.0,
        }],
    };
    runtime.clone_control(task()).unwrap();
    until(|| f.requests() == 1);
    let input = fixture("parent-natural-end");
    runtime.submit(input.clone()).unwrap();
    settle(&runtime, &sink, &input);
    assert_eq!(runtime.clones(None)["clones"][0]["state"], "running");
    let pid = runtime.clones(Some(1))["clones"][0]["view"]["childPid"]
        .as_u64()
        .unwrap() as i32;
    runtime.rpc(json!({"type":"abort"})).unwrap();
    assert_eq!(runtime.clones(None)["clones"][0]["state"], "stopped");
    unsafe {
        assert_eq!(libc::kill(pid, 0), -1);
    }
    runtime.clone_control(task()).unwrap();
    until(|| f.requests() == 3);
    let second = runtime.clones(Some(2))["clones"][1]["view"]["childPid"]
        .as_u64()
        .unwrap() as i32;
    let owner = runtime.snapshot().child_pid.unwrap();
    unsafe {
        assert_eq!(libc::kill(owner as i32, libc::SIGTERM), 0);
    }
    until(|| runtime.snapshot().state == OnlineState::Error);
    assert_eq!(runtime.clones(None)["clones"][1]["state"], "stopped");
    unsafe {
        assert_eq!(libc::kill(second, 0), -1);
    }
    runtime
        .clone_control(Request::Destroy { clone_id: 2 })
        .unwrap(); // Offline Dismiss, no wake.
    assert!(!f.root.join("workspace/under-table/clone-2").exists());
    runtime.close().unwrap();
}
