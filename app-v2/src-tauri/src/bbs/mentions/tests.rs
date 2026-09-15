use super::*;
use crate::agent_directory::tests::Account;
use crate::bbs_sync::roster::{self, PeerRoster};

fn content(account: &Account) -> sync::ContentStore {
    let root = account.0.join("Workspaces/bbs");
    ensure_layout_at(&root).unwrap();
    sync::ContentStore::at(root, account.0.clone())
}
fn author() -> Identity {
    human_identity("p", "Human".into(), None)
}
fn post(content: &sync::ContentStore, targets: &[Mention], text: &str) -> Result<String> {
    create_thread_mentions_scoped(content, &author(), vec![], true, text.into(), &[], targets)
}
fn install_roster(account: &Account, peer: &str, projects: Vec<roster::Project>) {
    let roster = PeerRoster {
        schema_version: 1,
        group_id: "group-a".into(),
        device_id: peer.into(),
        membership_id: "peer-membership".into(),
        version: roster::version(&projects).unwrap(),
        received_at: "2026-09-13T10:00:00Z".into(),
        projects,
    };
    let path = account
        .0
        .join("bbs-sync/rosters")
        .join(format!("{peer}.json"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(&roster).unwrap()).unwrap();
}

#[test]
fn local_mentions_are_explicit_identity_free_deduplicated_and_keep_historical_names() {
    let account = Account::new();
    account.project("p", false);
    account.agent("p", "a", "display-name: Before\n");
    let content = content(&account);
    let at = Mention::parse("local/p/a").unwrap();
    let thread = post(&content, &[at.clone(), at.clone()], "Original").unwrap();
    let (_, posts) = load_thread(content.root(), &thread).unwrap();
    assert_eq!(posts[0].meta.mentions, vec![at.clone()]);
    assert_eq!(posts[0].body, "@Before\n\nOriginal");
    assert!(!content.state.identity_path().exists());
    assert!(!content.state.control_path().exists());
    account.agent("p", "a", "display-name: After\n");
    let reply =
        reply_mentions_scoped(&content, &author(), &thread, "Reply".into(), &[], &[at]).unwrap();
    reply_scoped(
        &content,
        &author(),
        &thread,
        "@After is plain text".into(),
        &[],
    )
    .unwrap();
    let (_, posts) = load_thread(content.root(), &thread).unwrap();
    assert!(posts
        .iter()
        .find(|p| p.meta.post_id == reply)
        .unwrap()
        .body
        .starts_with("@After"));
    assert_eq!(
        posts.iter().find(|p| p.meta.kind == "topic").unwrap().body,
        "@Before\n\nOriginal"
    );
    assert!(posts
        .iter()
        .find(|p| p.body == "@After is plain text")
        .unwrap()
        .meta
        .mentions
        .is_empty());
    let literal = Mention::parse("local/p/a").unwrap();
    assert!(!literal.received_here("local"));
    assert!(!literal.received_here(&"a".repeat(64)));
}

#[test]
fn refs_validate_before_io_and_remote_local_is_never_rebound() {
    for value in [
        "a",
        "local/p/a/x",
        "local/../a",
        "local//a",
        "../p/a",
        "device-name/p/a",
        "local/p/a b",
    ] {
        assert!(Mention::parse(value).is_err(), "{value}");
    }
    let id = "a".repeat(64);
    assert!(Mention::parse(&format!("{id}/p/a"))
        .unwrap()
        .received_here(&id));
    for args in [
        vec!["new", "--broadcast", "--at"],
        vec![
            "new",
            "--broadcast",
            "--at",
            "local/p/a",
            "--at",
            "local/p/b",
        ],
        vec!["agents", "--json"],
        vec!["agents", "--device", "local"],
        vec!["reply", "thread-one", "--at", "../p/a"],
    ] {
        assert!(parse_cli_args(&args.into_iter().map(str::to_string).collect::<Vec<_>>()).is_err());
    }
    let parsed = parse_cli_args(
        &[
            "reply",
            "thread-one",
            "--at",
            "local/p/a",
            "--attach",
            "./pic.png",
        ]
        .map(str::to_string),
    )
    .unwrap();
    assert!(
        matches!(parsed, BbsCliCommand::Reply { at:Some(_), attachments, .. } if attachments.len()==1)
    );
}

#[test]
fn remote_refs_require_roster_and_shared_reply_and_cannot_publish_across_changed_group() {
    let account = Account::new();
    account.project("p", false);
    account.agent("p", "a", "display-name: Local same ID\n");
    let content = content(&account);
    let private = post(&content, &[], "History").unwrap();
    let invalid = Mention::parse(&format!("{}/p/a", "a".repeat(64))).unwrap();
    assert_eq!(
        post(&content, &[invalid], "No group")
            .unwrap_err()
            .to_string(),
        UNJOINED_NEW_ERROR
    );
    let (store, own, peer) = roster::tests::joined(&account);
    let _lease = store.control_lease().unwrap();
    let at = Mention::parse(&format!("{peer}/p/a")).unwrap();
    assert_eq!(
        post(&content, &[at.clone()], "No roster")
            .unwrap_err()
            .to_string(),
        "agent_roster_not_synced"
    );
    install_roster(
        &account,
        &peer,
        vec![roster::Project {
            project_id: "p".into(),
            name: "Same project".into(),
            agents: vec![roster::Agent {
                agent_id: "a".into(),
                name: "Remote same ID".into(),
                avatar: roster::Avatar::None,
            }],
        }],
    );
    let before = load_thread(content.root(), &private).unwrap().1.len();
    assert_eq!(
        reply_mentions_scoped(
            &content,
            &author(),
            &private,
            "Do not post".into(),
            &[],
            &[at.clone()]
        )
        .unwrap_err()
        .to_string(),
        PRIVATE_REPLY_ERROR
    );
    assert_eq!(
        load_thread(content.root(), &private).unwrap().1.len(),
        before
    );
    let shared = post(&content, &[at.clone()], "New shared").unwrap();
    let (_, posted) = load_thread(content.root(), &shared).unwrap();
    assert_eq!(posted[0].body, "@Remote same ID\n\nNew shared");
    let canonical = post(
        &content,
        &[
            Mention::parse("local/p/a").unwrap(),
            Mention::parse(&format!("{own}/p/a")).unwrap(),
        ],
        "Once",
    )
    .unwrap();
    let (_, posts) = load_thread(content.root(), &canonical).unwrap();
    assert_eq!(posts[0].meta.mentions.len(), 1);
    assert_eq!(posts[0].meta.mentions[0].device_id, own);
    let prepared = Prepared::load(&content, &[at], false).unwrap();
    let lock = acquire_write_lock(content.root()).unwrap();
    let mut control: serde_json::Value =
        serde_json::from_slice(&fs::read(store.control_path()).unwrap()).unwrap();
    control["current"]["membershipId"] = "changed".into();
    fs::write(store.control_path(), serde_json::to_vec(&control).unwrap()).unwrap();
    assert_eq!(
        prepared
            .finalize(&content, &lock, &shared, false, "No post".into())
            .unwrap_err()
            .to_string(),
        "group_context_changed"
    );
}

#[test]
fn a_known_empty_remote_roster_allows_stale_target_id_without_guessing_a_name() {
    let account = Account::new();
    let content = content(&account);
    let (_, _, peer) = roster::tests::joined(&account);
    install_roster(&account, &peer, vec![]);
    let at = Mention::parse(&format!("{peer}/missing/old-agent")).unwrap();
    let thread = post(&content, &[at], "Old target").unwrap();
    assert_eq!(
        load_thread(content.root(), &thread).unwrap().1[0].body,
        "@old-agent\n\nOld target"
    );
}
