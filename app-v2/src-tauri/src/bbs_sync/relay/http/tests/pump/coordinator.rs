//! Account actors through real bounded HTTP workers, real mutual TLS and real
//! content/file IO. Only the remote relay is an in-process state double.
use super::*;
use crate::bbs_sync::{
    control::{Member, Membership, Role},
    coordinator::{now, Authority, Command, Notice, Work},
    exchange as content,
    relay::{coordinator::Actor, discovery::Declaration, wire::Declarations},
    roster,
    transport::NetworkHost,
};
use serde::Deserialize;
use std::sync::{RwLock, Weak};
use tokio::sync::mpsc;

struct Directory {
    boot: String,
    members: Vec<Member>,
    announcements: BTreeMap<String, Value>,
    receipts: BTreeMap<String, Value>,
    wake: Option<Value>,
    ready: [bool; 2],
    offer: Option<Declaration>,
    answer: Option<Declaration>,
    session: Option<String>,
    calls: BTreeMap<String, usize>,
    requests: BTreeMap<String, usize>,
    fail_next_poll: std::collections::BTreeSet<String>,
}
impl Directory {
    fn new(members: Vec<Member>) -> Self {
        Self {
            boot: fixture().contexts[0].boot.clone(),
            members,
            announcements: BTreeMap::new(),
            receipts: BTreeMap::new(),
            wake: None,
            ready: [false; 2],
            offer: None,
            answer: None,
            session: None,
            calls: BTreeMap::new(),
            requests: BTreeMap::new(),
            fail_next_poll: Default::default(),
        }
    }
    fn clear_ready(&mut self) {
        self.ready = [false; 2];
        self.offer = None;
        self.answer = None;
        self.session = None;
    }
    fn poll(&self, local: &str, data: &RelayState) -> Vec<u8> {
        let mut items = Vec::new();
        for m in &self.members {
            let other = m.device_id != local;
            let wake = if other { self.wake.as_ref() } else { None };
            let closed = self
                .session
                .as_ref()
                .and_then(|id| data.sessions.get(id))
                .is_some_and(|s| s.directions.iter().any(|d| d.closed));
            let view = if wake.is_some() {
                format!("{{\"ready\":{{\"client\":{},\"server\":{}}},\"client\":{},\"server\":{},\"session\":{},\"closed\":{}}}",
                    self.ready[0],self.ready[1],
                    if closed {"null".into()} else {self.offer.as_ref().map(|d|String::from_utf8(d.bytes().to_vec()).unwrap()).unwrap_or("null".into())},
                    if closed {"null".into()} else {self.answer.as_ref().map(|d|String::from_utf8(d.bytes().to_vec()).unwrap()).unwrap_or("null".into())},
                    serde_json::to_string(&self.session).unwrap(),closed)
            } else {
                "null".into()
            };
            items.push(format!("{{\"device\":{},\"announcement\":{},\"currentWake\":{},\"wake\":{},\"handshake\":{}}}",
                serde_json::to_string(&m.device_id).unwrap(),serde_json::to_string(&self.announcements.get(&m.device_id)).unwrap(),
                serde_json::to_string(&wake.map(|w|&w["id"])).unwrap(),serde_json::to_string(&wake).unwrap(),view));
        }
        format!(
            "{{\"boot\":{},\"items\":[{}],\"next\":null}}",
            serde_json::to_string(&self.boot).unwrap(),
            items.join(",")
        )
        .into_bytes()
    }
}
struct Service {
    directory: Arc<Mutex<Directory>>,
    data: Arc<Mutex<RelayState>>,
}
impl Backend for Service {
    fn discard(&mut self) {}
    fn run(
        &mut self,
        endpoint: &Endpoint,
        request: &SignedRequest,
        body: Option<&Body>,
        response: &mut Buffer,
        guard: &Guard,
    ) -> Result<Parts> {
        let url = request.url();
        let path = url.split('?').next().unwrap().rsplit('/').next().unwrap();
        // Count every backend attempt, including an early boot/session refusal
        // and retries. Accepted upload/receive counters alone are not a total.
        *self.directory.lock().unwrap().requests.entry(path.into()).or_default() += 1;
        if matches!(path, "send" | "ack" | "receive") {
            let boot = request
                .receive_request()
                .map(|r| r.boot.clone())
                .or_else(|| request.headers().ok()?.get("x-kota-relay-boot").cloned());
            let missing = {
                let data = self.data.lock().unwrap();
                if let Some(read) = request.receive_request() {
                    read.cursors.iter().any(|(id, _)| !data.sessions.contains_key(id))
                } else {
                    request.headers().ok().and_then(|h| h.get("x-kota-relay-session").cloned())
                        .is_none_or(|id| !data.sessions.contains_key(&id))
                }
            };
            if missing || boot.as_deref() != Some(self.directory.lock().unwrap().boot.as_str()) {
                let bytes = br#"{"ok":false,"error":"relay_session_lost"}"#;
                response.spare_mut()[..bytes.len()].copy_from_slice(bytes);
                response.advance(bytes.len())?;
                return Ok(Parts {
                    status: 409,
                    headers: BTreeMap::from([(
                        "content-type".into(),
                        vec!["application/json".into()],
                    )]),
                });
            }
            return Relay(self.data.clone()).run(endpoint, request, body, response, guard);
        }
        guard.check()?;
        let mut bytes = Vec::new();
        if let Some(b) = body {
            b.reader().read_to_end(&mut bytes).unwrap();
        }
        assert!(request.matches_body(&bytes));
        let h = request.headers()?;
        let local = &h["x-kota-relay-device"];
        let mut s = self.directory.lock().unwrap();
        *s.calls.entry(path.into()).or_default() += 1;
        let actor = s
            .members
            .iter()
            .position(|m| &m.device_id == local)
            .ok_or(Error::Unauthorized)?;
        assert_eq!(s.members[actor].membership_id, h["x-kota-relay-membership"]);
        let mut status = 200;
        let result = match path {
            "poll" => {
                assert!(bytes.is_empty());
                if s.fail_next_poll.remove(local) {
                    return Err(Error::Timeout);
                }
                s.poll(local, &self.data.lock().unwrap())
            }
            "announce" | "wake" => {
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                let id = value["requestId"].as_str().unwrap().to_string();
                let result = if let Some(old) = s.receipts.get(&id) {
                    old.clone()
                } else {
                    let current = if path == "announce" {
                        s.announcements.get(local).map(|a| a["requestId"].clone())
                    } else {
                        s.wake.as_ref().map(|w| w["id"].clone())
                    }
                    .unwrap_or(Value::Null);
                    if current != value["previous"] {
                        json!({"ok":false,"current":current,"changed":false})
                    } else if path == "announce" {
                        s.announcements.insert(local.clone(),json!({"device":local,"membership":h["x-kota-relay-membership"],
                            "requestId":id,"fingerprint":raw_sha256(&bytes),"instance":value["instance"],"revision":value["revision"],"peerVersion":4}));
                        json!({"ok":true,"current":id,"changed":true})
                    } else {
                        let other = value["peer"].as_str().unwrap();
                        let pi = s.members.iter().position(|m| m.device_id == other).unwrap();
                        assert_eq!(s.announcements[local]["instance"], value["instance"]);
                        assert_eq!(s.announcements[other]["instance"], value["peerInstance"]);
                        let wake = raw_sha256(
                            &serde_json::to_vec(&json!([
                                "kota-bbs-relay.wake-id.v1",
                                fixture().contexts[0].peer.group_id,
                                local,
                                h["x-kota-relay-membership"],
                                id
                            ]))
                            .unwrap(),
                        );
                        let (ci, si) = if actor < pi {
                            (value["instance"].clone(), value["peerInstance"].clone())
                        } else {
                            (value["peerInstance"].clone(), value["instance"].clone())
                        };
                        s.wake = Some(
                            json!({"id":wake,"group":fixture().contexts[0].peer.group_id,"client":s.members[0].device_id,"server":s.members[1].device_id,
                            "clientMembership":s.members[0].membership_id,"serverMembership":s.members[1].membership_id,"clientInstance":ci,"serverInstance":si,
                            "createdAt":now(),"expiresAt":now()+120000}),
                        );
                        s.clear_ready();
                        self.data.lock().unwrap().sessions.clear();
                        if current.is_null() {
                            json!({"ok":true,"current":wake,"changed":true})
                        } else {
                            json!({"ok":true,"current":wake,"changed":true,"replacedWake":current})
                        }
                    }
                };
                s.receipts.insert(id, result.clone());
                serde_json::to_vec(&result).unwrap()
            }
            "ready" => {
                let v: Value = serde_json::from_slice(&bytes).unwrap();
                let w = s.wake.as_ref().unwrap();
                assert_eq!(v["wake"], w["id"]);
                assert_eq!(v["clientInstance"], w["clientInstance"]);
                assert_eq!(v["serverInstance"], w["serverInstance"]);
                s.ready[actor] = true;
                serde_json::to_vec(&json!({"boot":s.boot,"client":s.ready[0],"server":s.ready[1]}))
                    .unwrap()
            }
            "open" => {
                #[derive(Deserialize)]
                struct Open {
                    client: Declaration,
                    server: Option<Declaration>,
                }
                let v: Open = serde_json::from_slice(&bytes).unwrap();
                assert!(s.ready.iter().all(|x| *x));
                if let Some(server) = v.server {
                    assert_eq!(actor, 1);
                    assert_eq!(s.offer.as_ref().unwrap().bytes(), v.client.bytes());
                    let id = Declarations {
                        client: v.client.signed.clone(),
                        server: server.signed.clone(),
                    }
                    .session_id()?;
                    self.data
                        .lock()
                        .unwrap()
                        .sessions
                        .entry(id.clone())
                        .or_insert_with(|| Session {
                            devices: [
                                s.members[0].device_id.clone(),
                                s.members[1].device_id.clone(),
                            ],
                            directions: Default::default(),
                        });
                    s.answer = Some(server);
                    s.session = Some(id.clone());
                    serde_json::to_vec(&json!({"boot":s.boot,"session":id,"closed":false})).unwrap()
                } else {
                    assert_eq!(actor, 0);
                    if let Some(old) = &s.offer {
                        assert_eq!(old.bytes(), v.client.bytes());
                    }
                    s.offer = Some(v.client);
                    serde_json::to_vec(&json!({"boot":s.boot,"offered":true})).unwrap()
                }
            }
            _ => {
                status = 400;
                serde_json::to_vec(&json!({"ok":false,"error":"invalid_relay_route"})).unwrap()
            }
        };
        response.spare_mut()[..result.len()].copy_from_slice(&result);
        response.advance(result.len())?;
        Ok(Parts {
            status,
            headers: BTreeMap::from([("content-type".into(), vec!["application/json".into()])]),
        })
    }
}

#[tokio::test]
async fn account_coordinators_announce_wake_handshake_sync_and_cancel_without_rtc() {
    account_round(false, 20).await;
}
#[tokio::test]
async fn boot_loss_retires_written_partial_and_retransfers_whole_file_via_account_owners() {
    account_round(true, 0).await;
}
#[tokio::test]
#[ignore = "capacity measurement: two full 1,000-version histories through actual account actors"]
async fn thousand_version_history_measures_automatic_delta_and_manual_full_requests() {
    account_round(false, 1000).await;
}
async fn account_round(restart: bool, history: usize) {
    account_scenario(restart, history, false).await;
}

#[tokio::test]
async fn idle_failure_then_automatic_and_one_sided_retries_complete_without_new_content() {
    account_scenario(false, 0, true).await;
}

async fn account_scenario(restart: bool, history: usize, recovery: bool) {
    let f = fixture();
    let (_ra, sa, _) = content::tests::board(&f.contexts[0].peer.local_membership_id);
    let (_rb, sb, _) = content::tests::board(&f.contexts[1].peer.local_membership_id);
    let body = if restart {
        "restart bytes\n".repeat(150_000)
    } else {
        "Coordinator-owned HTTPS round".into()
    };
    let raw = content::tests::publish(&sa, &body);
    let _reverse = content::tests::publish(&sb, "Coordinator reverse Fork");
    // Both devices already own the same real history. Initial and
    // Manual rounds must enumerate it; one automatic edit must use the delta.
    for i in 0..history {
        let id=format!("history-{i:08}");
        let meta=json!({"schema":"kota.bbs.post.v1","threadId":"thread-one","postId":id,
            "projectId":"source","agentId":"author","agentDisplayName":"Author",
            "projectDisplayName":"Source","createdAt":"2026-09-12T10:00:00Z","kind":"reply"});
        let raw=format!("---\n{}---\n\nHistory {i}\n",serde_yaml::to_string(&meta).unwrap());
        for store in [&sa,&sb] {fs::write(store.root().join(format!("threads/thread-one/posts/{id}.md")),&raw).unwrap();}
    }
    let members: Vec<_> = (0..2)
        .map(|i| Member {
            device_id: f.identities[i].device_id().unwrap(),
            public_key: f.identities[i].public_key.clone(),
            name: format!("Device {i}"),
            role: if i == 0 { Role::Owner } else { Role::Member },
            membership_id: f.contexts[i].peer.local_membership_id.clone(),
            last_seen_at: now(),
            online: true,
        })
        .collect();
    for (i, store) in [&sa, &sb].into_iter().enumerate() {
        let path = store.state.control_path();
        let mut control: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        control["current"]["groupId"] = json!(f.contexts[0].peer.group_id);
        control["current"]["workerUrl"] = json!(f.contexts[0].origin);
        control["current"]["role"] = json!(members[i].role);
        fs::write(path, serde_json::to_vec(&control).unwrap()).unwrap();
        store.state.save_identity(&f.identities[i]).unwrap();
        let mut state = store.state.load_state().unwrap().unwrap();
        let shared = state.groups.remove("group-test").unwrap();
        state
            .groups
            .insert(f.contexts[0].peer.group_id.clone(), shared);
        store.state.save_state(&state).unwrap();
        let current = crate::bbs_sync::control::read_membership(&store.state).unwrap();
        roster::persist_members(&store.state, current.as_ref(), &members).unwrap();
    }
    let roster = [
        roster::runtime::Runtime::test_source(sa.clone(), vec![]),
        roster::runtime::Runtime::test_source(sb.clone(), vec![]),
    ];
    let files = [roster[0].files().unwrap(), roster[1].files().unwrap()];
    let host = NetworkHost::start_with_files(files[0].0.clone(), files[0].1.clone())
        .await
        .unwrap();
    let directory = Arc::new(Mutex::new(Directory::new(members.clone())));
    let data = Arc::new(Mutex::new(RelayState::default()));
    let dirs = directory.clone();
    let dat = data.clone();
    let changed_source=sa.clone();
    let result=host.execute(move |_|async move{
        let mut engines=Vec::new();let mut controls=Vec::new();let mut tasks=Vec::new();let mut pools=Vec::new();let mut stops=Vec::new();
        let notices:Arc<Mutex<Vec<(usize,Notice)>>>=Default::default();let work=[Arc::new(Work::default()),Arc::new(Work::default())];
        let phase_origin = Instant::now();
        let phases: Arc<Mutex<Vec<(usize, u128, &'static str)>>> = Default::default();
        let phase_report = |label: &str, from: usize, since: u128| {
            let events = phases.lock().unwrap().iter().skip(from)
                .map(|(peer, at, phase)| json!({"peer":peer,"elapsedMs":at.saturating_sub(since),"phase":phase}))
                .collect::<Vec<_>>();
            println!("SYNC_IDLE_PHASES {}", json!({"scenario":label,"events":events,
                "fixture":"two real Actors/TLS/Exchange/FileIo; process-local relay, no Cloudflare"}));
        };
        let started_revisions:Arc<Mutex<[Option<String>;2]>>=Default::default();
        for (i,store) in [sa,sb.clone()].into_iter().enumerate(){
            let d=dirs.clone();let wire=dat.clone();let created=dat.clone();
            let pool=HttpPool::start_with(files[i].1.clone(),move |_|{created.lock().unwrap().created+=1;Box::new(Service{directory:d.clone(),data:wire.clone()})}).await?;
            let stop=Cancellation::default();let context=Context{io:files[i].0.clone(),limits:files[i].1.clone(),shutdown:stop.clone()};
            // The capacity fixture does not run the management heartbeat owner.
            // Its one synthetic authority must cover the whole measurement;
            // production authorization expiry/renewal is unchanged.
            let auth=Authority{membership:Membership{group_id:f.contexts[0].peer.group_id.clone(),worker_url:f.contexts[0].origin.clone(),membership_id:members[i].membership_id.clone(),role:members[i].role.clone()},identity:f.identities[i].clone(),members:members.clone(),valid_until:now()+if history>=1000 {1_800_000} else {600_000}};
            let observed=notices.clone();let (tx,rx)=mpsc::channel(16);
            let observed_phases = phases.clone();
            let engine_slot:Arc<Mutex<Option<Arc<content::Engine>>>>=Default::default();
            let observed_engine=engine_slot.clone();let pinned=started_revisions.clone();
            let a=Actor::new(context,store,auth.clone(),pool.bind(&auth.membership.worker_url)?,Arc::new(move|_,event|{
                if recovery {
                    let phase = match &event {
                        Notice::Connecting(_) => Some("connecting"),
                        Notice::Connected(_) => Some("connected"),
                        Notice::Disconnected(_) => Some("disconnected"),
                        Notice::Error(_, _) => Some("error"),
                        Notice::Exchange(content::Event::Started(_)) => Some("started"),
                        Notice::Exchange(content::Event::Finished(_, _)) => Some("finished"),
                        Notice::Exchange(content::Event::Failed(_, _)) => Some("failed"),
                        _ => None,
                    };
                    if let Some(phase) = phase {
                        observed_phases.lock().unwrap().push((i, phase_origin.elapsed().as_millis(), phase));
                    }
                }
                // Started is within the pinned round; a post-Finished refresh
                // may already include newly installed content and is not its revision.
                if matches!(&event, Notice::Exchange(content::Event::Started(_))) {
                    pinned.lock().unwrap()[i]=Some(observed_engine.lock().unwrap().as_ref().unwrap().revision().unwrap());
                }
                match &event {Notice::Error(_,e)=>eprintln!("RELAY_OWNER {i} error {e}"),
                    Notice::Exchange(content::Event::Finished(_,o))=>eprintln!("RELAY_OWNER {i} finished {}",serde_json::to_string(o).unwrap()),_=>{}}
                observed.lock().unwrap().push((i,event));}),work[i].clone(),work[i].epoch(),roster[i].clone(),Arc::new(RwLock::new(auth)),Arc::new(Mutex::new(Vec::<Weak<crate::bbs_sync::transport::Connection>>::new())))?;
            let engine=a.test_engine();*engine_slot.lock().unwrap()=Some(engine.clone());
            engines.push(engine);controls.push(tx);tasks.push(tokio::spawn(a.run(rx)));pools.push(pool);stops.push(stop);
        }
        let expected=raw_sha256(&raw);
        let mut interrupted = None;
        let mut old_session = None;
        let watch_started = Instant::now(); let mut watch_at = Instant::now();
        let wait=tokio::time::timeout(Duration::from_secs(if history >= 1000 {360} else if restart {240} else {90}),async{
            loop{
                if restart && interrupted.is_none() {
                    if let Ok(entries) = fs::read_dir(sb.root().join(".sync-staging")) {
                        for entry in entries.flatten() {
                            if entry.file_name().to_string_lossy().starts_with(".receive-") {
                                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                                if size > 0 && size < raw.len() as u64 {
                                    let mut directory = dirs.lock().unwrap();
                                    old_session = directory.session.clone();
                                    directory.boot = uuid::Uuid::new_v4().to_string();
                                    directory.clear_ready();
                                    dat.lock().unwrap().sessions.clear();
                                    interrupted = Some(entry.path());
                                    break;
                                }
                            }
                        }
                    }
                }
                if restart && Instant::now() >= watch_at {
                    watch_at = Instant::now()+Duration::from_secs(10);
                    let state = dat.lock().unwrap();
                    let partial = fs::read_dir(sb.root().join(".sync-staging")).into_iter().flatten().flatten().map(|e|e.metadata().map(|m|m.len()).unwrap_or(0)).collect::<Vec<_>>();
                    eprintln!("RELAY_RESTART elapsed={} uploads={} receive={} ack={} bytes={} partial={partial:?}", watch_started.elapsed().as_secs(),state.send_sizes.len(),state.receives,state.acknowledgements,state.send_sizes.iter().sum::<usize>());
                }
                let finished=notices.lock().unwrap().iter().filter(|(_,n)|matches!(n,Notice::Exchange(content::Event::Finished(_,o)) if o.failures.is_empty() && !o.more)).count();
                if finished>=2{break;}
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await;
        let checkpoint_pairs=(0..2).map(|i|(engines[i].checkpoint(&f.identities[1-i].device_id().unwrap()).unwrap(),started_revisions.lock().unwrap()[1-i].clone())).collect::<Vec<_>>();
        let usage=|| {
            let directory=dirs.lock().unwrap();let data=dat.lock().unwrap();
            let metadata=directory.calls.values().sum::<usize>();
            let total=directory.requests.values().sum::<usize>();
            let classified=metadata+data.send_sizes.len()+data.receives+data.acknowledgements+data.small_reads;
            assert!(total>=classified);
            json!({"metadata":directory.calls,"uploads":data.send_sizes.len(),"receive":data.receives,
                "acks":data.acknowledgements,"small":data.small_reads,
                "requestsByRoute":directory.requests,"total":total,"otherAttempts":total-classified,
                "manifestItems":engines.iter().map(|e|e.manifest_counts()).collect::<Vec<_>>()})
        };
        let initial_usage=usage();
        if history>=1000 { eprintln!("BBS_DELTA_ACTOR_STAGE initialFull elapsedMs={} {}",watch_started.elapsed().as_millis(),initial_usage); }
        let mut changed_usage=Value::Null;
        let mut automatic_ok=restart;
        if wait.is_ok() && !restart && !recovery {
            let edited=content::tests::publish(&changed_source,"Automatic edit after matching checkpoints");
            let resource=Resource{identity:ResourceKind::Post{thread_id:"thread-one".into(),post_id:"root".into(),version_id:raw_sha256(&edited)},sha256:raw_sha256(&edited),size_bytes:edited.len() as u64};
            let fence=crate::bbs::sync::GroupFence{group_id:f.contexts[0].peer.group_id.clone(),membership_id:f.contexts[1].peer.local_membership_id.clone()};
            work[0].changed();
            automatic_ok=tokio::time::timeout(Duration::from_secs(120),async {
                loop {
                    let done=notices.lock().unwrap().iter().filter(|(_,n)|matches!(n,Notice::Exchange(content::Event::Finished(_,o)) if o.failures.is_empty()&&!o.more)).count();
                    if done>=4 && sb.source(&fence,&resource).is_ok() {break;}
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }).await.is_ok();
            changed_usage=usage();
            if history>=1000 { eprintln!("BBS_DELTA_ACTOR_STAGE automaticChange elapsedMs={} {}",watch_started.elapsed().as_millis(),changed_usage); }
            let after=engines.iter().map(|e|e.manifest_counts()[2]).sum::<u64>();
            let before=initial_usage["manifestItems"].as_array().unwrap().iter().map(|v|v[2].as_u64().unwrap()).sum::<u64>();
            assert!(after-before<10,"automatic change must not resend the {history} unchanged versions: {initial_usage} -> {changed_usage}");
        }
        let mut manual_ok=restart;
        if wait.is_ok() && !restart && !recovery {
            let before=engines.iter().map(|e|e.manifest_counts()[2]).sum::<u64>();
            let target=notices.lock().unwrap().iter().filter(|(_,n)|matches!(n,Notice::Exchange(content::Event::Finished(_,o)) if o.failures.is_empty()&&!o.more)).count()+2;
            controls[1].send(Command::Manual(work[1].epoch())).await.unwrap();
            manual_ok=tokio::time::timeout(Duration::from_secs(if history >= 1000 {360} else {90}),async {
                loop {
                    let count=notices.lock().unwrap().iter().filter(|(_,n)|matches!(n,Notice::Exchange(content::Event::Finished(_,o)) if o.failures.is_empty()&&!o.more)).count();
                    let versions=engines.iter().map(|e|e.manifest_counts()[2]).sum::<u64>();
                    if count>=target && versions>=before+2*history as u64 {break;}
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }).await.is_ok();
        }
        let mut recovery_ok = true;
        if wait.is_ok() && recovery {
            // No peer action or content mutation: settle all follow-up empty
            // rounds, then fail exactly one real idle metadata request.
            // All requests use the existing local service double, never CF.
            let quiet = tokio::time::timeout(Duration::from_secs(90), async {
                let mut previous = 0;
                let mut since = Instant::now();
                loop {
                    let count = notices.lock().unwrap().len();
                    if count != previous { previous = count; since = Instant::now(); }
                    let closed = dat.lock().unwrap().sessions.values()
                        .all(|s| s.directions.iter().all(|d| d.closed));
                    if closed && since.elapsed() >= Duration::from_secs(8) { break; }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }).await.is_ok();
            assert!(quiet, "initial transfer must really become idle before fault injection");
            let finished = || notices.lock().unwrap().iter().filter(|(_, n)|
                matches!(n, Notice::Exchange(content::Event::Finished(_, o)) if o.failures.is_empty() && !o.more)).count();
            let baseline = finished();
            let fault_start = notices.lock().unwrap().len();
            let phase_start = phases.lock().unwrap().len();
            let phase_since = phase_origin.elapsed().as_millis();
            dirs.lock().unwrap().fail_next_poll.insert(members[0].device_id.clone());
            let observed = tokio::time::timeout(Duration::from_secs(40), async {
                loop {
                    if notices.lock().unwrap().iter().skip(fault_start).any(|(i, n)|
                        *i == 0 && matches!(n, Notice::Error(_, Error::Timeout))) { break; }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }).await.is_ok();
            assert!(observed, "injected idle poll timeout must be delivered to the account owner");
            let recovery_started = Instant::now();
            recovery_ok = tokio::time::timeout(Duration::from_secs(90), async {
                while finished() < baseline + 2 { tokio::time::sleep(Duration::from_millis(50)).await; }
            }).await.is_ok();
            println!("SYNC_IDLE_AUTOMATIC_RECOVERY={recovery_ok} before={baseline} after={} elapsedMs={}", finished(), recovery_started.elapsed().as_millis());
            phase_report("automatic-after-idle-poll-timeout", phase_start, phase_since);
            // Both TLS roles must be independently able to request a round.
            // This is a functional regression, not a quota/capacity benchmark.
            if recovery_ok {
                for side in [0, 1] {
                    let target = finished() + 2;
                    let retry_started = Instant::now();
                    let phase_start = phases.lock().unwrap().len();
                    let phase_since = phase_origin.elapsed().as_millis();
                    controls[side].send(Command::Manual(work[side].epoch())).await.unwrap();
                    let okay = tokio::time::timeout(Duration::from_secs(90), async {
                        while finished() < target { tokio::time::sleep(Duration::from_millis(50)).await; }
                    }).await.is_ok();
                    println!("SYNC_IDLE_SINGLE_RETRY side={side} okay={okay} elapsedMs={}", retry_started.elapsed().as_millis());
                    phase_report(&format!("single-manual-{side}"), phase_start, phase_since);
                    recovery_ok &= okay;
                    if !okay { break; }
                }
            }
            automatic_ok = recovery_ok;
            manual_ok = recovery_ok;
        }
        if !restart {println!("BBS_DELTA_ACTOR_COUNTS {}",json!({"history":history,"elapsedMs":watch_started.elapsed().as_millis(),"initialFull":initial_usage,"afterAutomaticChange":changed_usage,"afterManualFull":usage(),"fixture":"two real account owners/TLS/Exchange/FileIo; in-process relay, no Cloudflare"}));}
        if restart {
            assert!(interrupted.as_ref().is_some_and(|p| !p.exists()), "retired partial removed");
            assert_ne!(old_session, dirs.lock().unwrap().session, "fresh TLS session after boot loss");
        }
        let report:Vec<_>=notices.lock().unwrap().iter().map(|(i,n)|format!("{i}:{}",match n{Notice::Error(_,e)=>format!("error {e}"),Notice::Exchange(content::Event::Failed(_,e))=>format!("failed {e}"),Notice::Exchange(content::Event::Finished(_,o))=>format!("finished {}/{} failures={} more={}",o.completed,o.total,o.failures.len(),o.more),Notice::Connecting(_)=>"connecting".into(),Notice::Connected(_)=>"connected".into(),Notice::Disconnected(_)=>"disconnected".into(),_=>"progress".into()})).collect();
        eprintln!("RELAY_COORDINATOR events={report:?} requests={:?}",dirs.lock().unwrap().calls);
        for w in &work {w.cancel();}
        for stop in stops {stop.cancel();}
        for t in tasks {let _=t.await.unwrap();}for p in pools{p.stop();}
        assert!(wait.is_ok(),"complete real exchange through metadata owner: {report:?}");
        assert!(recovery_ok, "an idle metadata failure must recover without both ends pressing Retry: {report:?}");
        assert!(manual_ok,"responder Manual must wake the offerer again: {report:?}");
        assert!(automatic_ok,"automatic change must complete via the checkpoint delta: {report:?}");
        for (checkpoint,remote) in checkpoint_pairs {assert!(remote.is_some());assert_eq!(checkpoint,remote);}
        let fence=crate::bbs::sync::GroupFence{group_id:f.contexts[0].peer.group_id.clone(),membership_id:f.contexts[1].peer.local_membership_id.clone()};
        let resource=Resource{identity:ResourceKind::Post{thread_id:"thread-one".into(),post_id:"root".into(),version_id:expected.clone()},sha256:expected,size_bytes:raw.len()as u64};
        assert_eq!(fs::read(sb.source(&fence,&resource).unwrap()).unwrap(),raw);
        assert_eq!(dat.lock().unwrap().created,4);
        if !recovery { assert!(report.iter().all(|r|!r.contains("error")&&!r.contains("failed")),"{report:?}"); }
        else { assert_eq!(report.iter().filter(|r|r.contains("error")||r.contains("failed")).cloned().collect::<Vec<_>>(),vec!["0:error sync_timeout"],"only the deliberately injected failure is expected"); }
        Ok(())
    }).await;
    host.stop_and_wait().await;
    result.unwrap();
}
