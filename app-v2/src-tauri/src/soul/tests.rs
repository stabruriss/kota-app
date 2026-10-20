//! Synthetic under-table and account directories only: no real agent, no credentials.
use super::{
    cli::{self, Env, HELP},
    dream_usage, dreams_usage_path, episodes, marks, project_slug,
    pushes::{self, Delivery, Pushed},
    read_jsonl, SoulPaths,
};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);
const T1: &str = "2026-10-14T07:02:11Z";
const T2: &str = "2026-10-15T09:30:00Z";
const T3: &str = "2026-10-16T01:00:00Z";
const SESSION: &str = "codex:01a0f0fc";
const MESSAGE: &str = "violet-6b1d0e9f2a7c4d31e";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kota-soul-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("agent")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        Self { root }
    }
    fn agent(&self) -> PathBuf {
        self.root.join("agent")
    }
    fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    fn paths(&self) -> SoulPaths {
        SoulPaths::new(&self.agent())
    }
    fn env(&self, jev: bool) -> Env {
        Env {
            agent_cwd: self.agent(),
            kota_home: self.home(),
            agent_id: "agent-fixture".into(),
            project: Some("project-fixture".into()),
            jev_ready: Some(jev),
        }
    }
    fn run(&self, args: &[&str], ts: &str) -> String {
        self.try_run(false, args, "", ts).unwrap()
    }
    fn try_run(&self, jev: bool, args: &[&str], stdin: &str, ts: &str) -> Result<String, String> {
        let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        cli::run(&args, &self.env(jev), stdin, ts)
    }
    fn file(&self, path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }
    fn add(&self, text: &str, ts: &str) -> String {
        let out = self.run(&["add-mark", text], ts);
        assert!(out.starts_with("Added "), "{out}");
        out.split_whitespace().nth(1).unwrap().to_string()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn delivery(ts: &str) -> Delivery<'_> {
    delivery_for(ts, MESSAGE)
}

fn delivery_for<'a>(ts: &'a str, message: &'a str) -> Delivery<'a> {
    Delivery { agent_id: "agent-fixture", message, session: SESSION, project: Some("project-fixture"), ts, turns: None }
}

fn pushed(id: &str, p: f64) -> Pushed {
    Pushed { id: id.into(), p }
}

#[test]
fn help_is_the_approved_text_for_every_spelling() {
    let f = Fixture::new();
    for args in [&[][..], &["help"][..], &["--help"][..], &["-h"][..]] {
        assert_eq!(f.run(args, T1), HELP);
    }
    assert!(HELP.starts_with("kota-my-soul — your marks (lessons you keep across projects)"));
    assert_eq!(HELP.lines().count(), 10);
    assert!(HELP.lines().skip(2).all(|line| line.starts_with("  ")));
}

#[test]
fn add_mark_mints_a_dated_id_and_writes_the_sample_shape() {
    let f = Fixture::new();
    let text = "被用户纠正时先复述他的原话再改方案，不要先辩解。";
    let out = f.run(&["add-mark", text], T1);
    let id = out.split_whitespace().nth(1).unwrap().to_string();
    assert!(id.starts_with("m-20261014-") && id.len() == "m-20261014-4f2a".len(), "{id}");
    assert!(id[11..].chars().all(|ch| ch.is_ascii_hexdigit()), "{id}");
    assert_eq!(out, format!("Added {id} (revision 1). 1 of 20 marks in use.\n{id}  2026-10-14  {text}"));
    let expected = format!(
        "{{\n  \"version\": 1,\n  \"revision\": 1,\n  \"marks\": [\n    {{\n      \"id\": \"{id}\",\n      \"text\": \"{text}\",\n      \"created_at\": \"{T1}\",\n      \"updated_at\": \"{T1}\",\n      \"project\": \"project-fixture\",\n      \"used\": 0\n    }}\n  ]\n}}\n"
    );
    assert_eq!(f.file(&f.paths().marks()), expected);
    assert_eq!(f.try_run(true, &["read-mark"], "", T2).unwrap(), format!("{id}  used 0 times  2026-10-14  {text}"));
    assert_eq!(f.run(&["read-mark"], T2), format!("{id}  2026-10-14  {text}"));
    assert_eq!(f.file(&f.paths().episodes()), "");
}

#[test]
fn mark_text_collapses_whitespace_and_counts_unicode_characters() {
    let f = Fixture::new();
    let out = f.run(&["add-mark", "first  line\nsecond\tline"], T1);
    assert!(out.ends_with("  2026-10-14  first line second line"), "{out}");
    let two_hundred = "字".repeat(200);
    assert!(f.run(&["add-mark", &two_hundred], T1).starts_with("Added "));
    let too_long = "字".repeat(201);
    assert_eq!(f.run(&["add-mark", &too_long], T1), "Mark too long: 201 of 200 characters.");
    assert_eq!(f.run(&["add-mark", "   "], T1), "Mark text is empty.");
    assert_eq!(marks::snapshot(&f.paths()).unwrap().revision, 2);
}

#[test]
fn add_mark_stops_at_twenty_with_the_approved_reply() {
    let f = Fixture::new();
    for n in 0..20 {
        f.add(&format!("mark {n}"), T1);
    }
    assert_eq!(
        f.run(&["add-mark", "one more"], T1),
        "You already have 20 marks, the maximum. Run read-mark, then delete-mark or edit-mark one before adding."
    );
    let marks = marks::snapshot(&f.paths()).unwrap();
    assert_eq!((marks.marks.len(), marks.revision), (20, 20));
    assert_eq!(marks.marks.iter().map(|mark| mark.id.as_str()).collect::<HashSet<_>>().len(), 20);
}

#[test]
fn edit_keeps_the_id_without_an_episode_delete_retires_under_the_mark_id() {
    let f = Fixture::new();
    let id = f.add("交付前跑一遍测试。", T1);
    pushes::record(&f.paths(), &f.home(), &delivery(T1), &[pushed(&id, 0.99)]).unwrap();
    let push_log = f.file(&f.paths().pushes());
    let out = f.run(&["edit-mark", &id, "交付前自己跑一遍四项收据。"], T2);
    assert_eq!(
        out,
        format!("Updated {id} (revision 2).\n{id}  2026-10-14  交付前自己跑一遍四项收据。")
    );
    let mut marks = marks::snapshot(&f.paths()).unwrap();
    let mark = marks.marks.remove(0);
    assert_eq!((mark.created_at.as_str(), mark.updated_at.as_str()), (T1, T2));
    assert!(!f.paths().episodes().exists());
    assert_eq!(mark.used, 1);
    assert_eq!(f.file(&f.paths().pushes()), push_log);
    assert_eq!(
        f.run(&["edit-mark", "m-20260101-0000", "x"], T2),
        "No mark with id m-20260101-0000. Run read-mark to see your marks."
    );
    assert_eq!(f.run(&["delete-mark", &id], T3), format!("Deleted {id} (revision 3). Its text is kept as an episode."));
    let episodes_text = f.file(&f.paths().episodes());
    assert!(
        episodes_text.ends_with(&format!("{{\"id\":\"{id}\",\"kind\":\"retired_mark\",\"project\":\"project-fixture\",\"ts\":\"{T3}\",\"text\":\"交付前自己跑一遍四项收据。\",\"indexed\":true,\"w0\":0.5}}\n")),
        "{episodes_text}"
    );
    assert_eq!(f.run(&["read-mark"], T3), "No marks yet. Add one with add-mark \"<text>\".");
    assert_eq!(f.run(&["delete-mark", &id], T3), format!("No mark with id {id}. Run read-mark to see your marks."));
    assert_eq!(read_jsonl::<episodes::Episode>(&f.paths().episodes()).unwrap().len(), 1);
}

#[test]
fn text_arguments_accept_a_dash_separator_and_fall_back_to_stdin() {
    let f = Fixture::new();
    let out = f.run(&["add-mark", "--", "-starts", "with a dash"], T1);
    assert!(out.ends_with("  2026-10-14  -starts with a dash"), "{out}");
    let id = out.split_whitespace().nth(1).unwrap().to_string();
    let out = f.try_run(false, &["add-mark"], "from stdin\n", T1).unwrap();
    assert!(out.ends_with("  2026-10-14  from stdin"), "{out}");
    assert!(f.run(&["edit-mark", &id, "--", "--still a mark"], T2).contains("  --still a mark"));
    assert_eq!(f.try_run(false, &["edit-mark"], "", T2), Err(HELP.to_string()));
    assert_eq!(f.try_run(false, &["clear-heat", "--id"], "", T2), Err(HELP.to_string()));
    assert!(f.try_run(false, &["bogus"], "", T2).unwrap_err().starts_with("unknown kota-my-soul command: bogus"));
    assert_eq!(f.try_run(false, &["search-episode"], "", T2), Err(HELP.to_string()));
    assert_eq!(f.try_run(false, &["search-episode", "--id"], "", T2), Err(HELP.to_string()));
    assert_eq!(f.run(&["search-episode", "anything"], T2), "No episodes match \"anything\".");
}

fn pool(f: &Fixture, lines: &[&str]) {
    fs::create_dir_all(f.paths().episodes_dir()).unwrap();
    fs::write(f.paths().episodes(), format!("{}\n", lines.join("\n"))).unwrap();
}

const POOL: [&str; 7] = [
    r#"{"id":"violet-u1","kind":"message","role":"user","origin":"user","project":"project-fixture","ts":"2026-09-20T02:14:05Z","from":"老无","text":"以后除非有我的明确指令，禁止修改已经固定的流程","indexed":true,"w0":0.5}"#,
    r#"{"id":"violet-a1","kind":"message","role":"assistant","origin":"end_turn","project":"project-fixture","ts":"2026-09-20T02:16:40Z","from":"self","prev":"violet-u1","text":"收到。固定流程清单老子记在收据里，之后只在你明确指令下改动。","indexed":true,"w0":0.5}"#,
    r#"{"id":"violet-u2","kind":"message","role":"user","origin":"user","project":"project-fixture","ts":"2026-09-21T11:31:02Z","from":"老无","text":"补一个，复杂么","indexed":false}"#,
    r#"{"id":"m-20260925-77b1","kind":"retired_mark","project":"cedar-fixture","ts":"2026-10-05T08:02:31Z","text":"交付前跑一遍测试。","indexed":true,"w0":1.0,"replaced_by":"m-20261002-9c17"}"#,
    r#"{"id":"d-7c1e55a0","kind":"old_dream","project":null,"ts":"2026-10-09T09:00:00Z","text":"老无派活时喜欢一次只聊一个话题，聊完清点再开工。","indexed":true,"w0":0.5}"#,
    r#"{"id":"violet-p2","kind":"message","role":"user","origin":"telegram","project":"cedar-fixture","ts":"2026-10-04T11:30:12Z","from":"老无","text":"固定流程在另一个项目也要遵守","indexed":true,"w0":0.5}"#,
    r#"{"id":"violet-e1","kind":"message","role":"assistant","origin":"end_turn","project":"project-fixture","ts":"2026-10-02T21:40:10Z","from":"self","text":"Run the four receipts before delivery.\nNever write PASS for a test that did not run.","indexed":true,"w0":0.5}"#,
];

#[test]
fn tokens_are_lowercase_words_and_cjk_bigrams() {
    assert_eq!(
        super::search::tokens("固定流程 改动 Hello_World v2，字"),
        ["固定", "定流", "流程", "改动", "hello_world", "v2", "字"]
    );
    assert!(super::search::tokens(" — ...").is_empty());
}

#[test]
fn search_returns_hits_with_neighbour_lines_and_records_returns() {
    let f = Fixture::new();
    pool(&f, &POOL);
    let out = f.run(&["search-episode", "固定流程"], T1);
    let blocks = out.split("\n\n").collect::<Vec<_>>();
    // Every 2-gram of the query must match: "已经固定的流程" has no "定流", so violet-u1 is a neighbour, not a hit.
    assert_eq!(blocks.len(), 3, "{out}");
    assert_eq!(blocks[2], "Use `search-episode --id <id>` for the full text.");
    assert!(!out.contains("[violet-u1]"), "{out}");
    let a1 = blocks.iter().find(|b| b.starts_with("[violet-a1]")).unwrap();
    assert_eq!(
        *a1,
        "[violet-a1] 2026-09-20 · project-fixture · you\n收到。\n↳ before: 老无: 以后除非有我的明确指令，禁止修改已经固定的流程"
    );
    assert!(blocks.iter().any(|b| b.starts_with("[violet-p2] 2026-10-04 · cedar-fixture · 老无\n")), "{out}");
    let usage = read_jsonl::<episodes::Usage>(&f.paths().episodes_usage()).unwrap();
    assert_eq!(usage.len(), 2);
    assert!(usage.iter().all(|u| u.op == "return" && u.w == Some(0.5) && u.q.as_deref() == Some("固定流程") && u.ts == T1));
    let scoped = f.run(&["search-episode", "--project", "project-fixture", "--", "固定流程"], T1);
    assert!(scoped.contains("[violet-a1]") && !scoped.contains("[violet-p2]"), "{scoped}");
    let exact = f.run(&["search-episode", "固定的流程"], T1);
    assert!(exact.starts_with("[violet-u1] 2026-09-20 · project-fixture · 老无\n以后除非有我的明确指令，禁止修改已经固定的流程\n"), "{exact}");
    assert_eq!(f.run(&["search-episode", "复杂"], T1), "No episodes match \"复杂\".");
    let english = f.run(&["search-episode", "RECEIPTS"], T1);
    assert!(english.starts_with("[violet-e1] 2026-10-02 · project-fixture · you\nRun the four receipts before delivery.\n"), "{english}");
    let dream = f.run(&["search-episode", "话题"], T1);
    assert!(dream.starts_with("[d-7c1e55a0] 2026-10-09 · old dream\n"), "{dream}");
    // Existing pool lines are neither migrated nor deleted by the new export policy.
    assert_eq!(f.file(&f.paths().episodes()), format!("{}\n", POOL.join("\n")));
}

#[test]
fn expand_shows_the_full_text_and_reminders_follow_the_usage_thresholds() {
    let f = Fixture::new();
    pool(&f, &POOL);
    assert_eq!(
        f.run(&["search-episode", "--id", "violet-e1"], T1),
        "[violet-e1] 2026-10-02 · project-fixture · you\nRun the four receipts before delivery.\nNever write PASS for a test that did not run."
    );
    assert_eq!(f.run(&["search-episode", "--id", "violet-none"], T1), "No episode with id violet-none.");
    let usage = read_jsonl::<episodes::Usage>(&f.paths().episodes_usage()).unwrap();
    assert_eq!((usage.len(), usage[0].op.as_str(), usage[0].w, usage[0].q.is_none()), (1, "expand", Some(1.0), true));
    for _ in 0..3 {
        assert!(!f.run(&["search-episode", "receipts"], T1).contains("↑ Returned"));
    }
    let fourth = f.run(&["search-episode", "receipts"], T2);
    assert!(fourth.contains("\n↑ Returned 4 times, expanded 1. If this is a lasting lesson, add-mark it; then run clear-heat violet-e1 either way.\n\nUse `search-episode"), "{fourth}");
    assert_eq!(f.run(&["clear-heat", "violet-e1"], T2), "Cleared usage for violet-e1.");
    assert!(!f.run(&["search-episode", "receipts"], T3).contains("↑ Returned"));
    f.run(&["search-episode", "--id", "violet-e1"], T3);
    f.run(&["search-episode", "--id", "violet-e1"], T3);
    assert!(f.run(&["search-episode", "receipts"], T3).contains("↑ Returned 2 times, expanded 2. If this is a lasting lesson"));
}

#[test]
fn index_follows_the_pool_and_is_only_a_cache() {
    let f = Fixture::new();
    pool(&f, &POOL[..2]);
    assert!(f.run(&["search-episode", "固定流程"], T1).contains("[violet-a1]"));
    assert!(!f.run(&["search-episode", "receipts"], T1).contains("[violet-e1]"));
    thread::sleep(Duration::from_millis(20));
    episodes::append(&f.paths(), &[serde_json::from_str::<episodes::Episode>(POOL[6]).unwrap()]).unwrap();
    assert!(f.run(&["search-episode", "receipts"], T1).contains("[violet-e1]"));
    fs::remove_file(f.paths().episodes_index()).unwrap();
    assert!(f.run(&["search-episode", "receipts"], T1).contains("[violet-e1]"));
    assert!(f.paths().episodes_index().is_file());
}

fn hit(id: &str, ts: &str, relevance: f64, heat: f64) -> super::search::Hit {
    super::search::Hit {
        episode: super::search::Stored { id: id.into(), kind: "message".into(), project: None, ts: ts.into(), from: None, prev: None, text: String::new(), w0: Some(0.5) },
        neighbour: None,
        relevance,
        heat,
    }
}

#[test]
fn rank_bounds_heat_inside_the_relevant_set_and_keeps_a_newest_fifth() {
    let ids = |hits: Vec<super::search::Hit>| hits.into_iter().map(|h| h.episode.id).collect::<Vec<_>>();
    // A weakly relevant hot episode (0.6 + 0.3) stays behind the most relevant cold one (1.0).
    assert_eq!(ids(super::search::rank(vec![hit("cold", T1, 10.0, f64::NEG_INFINITY), hit("hot", T1, 6.0, 2.0)])), ["cold", "hot"]);
    // Nearly as relevant and hot: heat decides (0.9 + 0.3 > 1.0).
    assert_eq!(ids(super::search::rank(vec![hit("cold", T1, 10.0, -0.5), hit("hot", T1, 9.0, 1.0)])), ["hot", "cold"]);
    // All cold: pure BM25, ties to the newer episode.
    assert_eq!(ids(super::search::rank(vec![hit("old", T1, 5.0, -1.0), hit("new", T2, 5.0, -1.0), hit("best", T1, 7.0, -3.0)])), ["best", "new", "old"]);
    // Six relevant: four by score, then the newest of the rest, never six.
    let six = vec![
        hit("r1", "2026-10-01T00:00:00Z", 10.0, 0.0),
        hit("r2", "2026-10-02T00:00:00Z", 9.0, 0.0),
        hit("r3", "2026-10-03T00:00:00Z", 8.0, 0.0),
        hit("r4", "2026-10-04T00:00:00Z", 7.0, 0.0),
        hit("r5", "2026-10-05T00:00:00Z", 6.0, 0.0),
        hit("newest", "2026-10-09T00:00:00Z", 1.0, 0.0),
    ];
    assert_eq!(ids(super::search::rank(six)), ["r1", "r2", "r3", "r4", "newest"]);
}

#[test]
fn record_writes_the_agent_ledger_used_and_the_account_dream_ledger() {
    let f = Fixture::new();
    let a = f.add("mark a", T1);
    let b = f.add("mark b", T1);
    pushes::record(&f.paths(), &f.home(), &delivery(T1), &[pushed(&a, 0.91), pushed("d-9f31c2a4", 0.88)]).unwrap();
    assert_eq!(
        f.file(&f.paths().pushes()),
        format!(
            "{{\"ts\":\"{T1}\",\"mark\":\"{a}\",\"p\":0.91,\"message\":\"{MESSAGE}\",\"session\":\"{SESSION}\",\"project\":\"project-fixture\"}}\n{{\"ts\":\"{T1}\",\"dream\":\"d-9f31c2a4\",\"p\":0.88,\"message\":\"{MESSAGE}\",\"session\":\"{SESSION}\",\"project\":\"project-fixture\"}}\n"
        )
    );
    let marks = marks::snapshot(&f.paths()).unwrap();
    assert_eq!((marks.revision, marks.marks[0].used, marks.marks[1].used), (2, 1, 0));
    assert_eq!(
        f.file(&dreams_usage_path(&f.home())),
        format!("{{\"ts\":\"{T1}\",\"op\":\"push\",\"id\":\"d-9f31c2a4\",\"p\":0.88,\"agent\":\"agent-fixture\",\"message\":\"{MESSAGE}\"}}\n")
    );
    assert_eq!(pushes::last_push(&f.paths(), &a, Some(SESSION)).unwrap().unwrap().ts, T1);
    assert!(pushes::last_push(&f.paths(), &a, Some("other")).unwrap().is_none());
    assert!(pushes::last_push(&f.paths(), &b, None).unwrap().is_none());
    assert_eq!(pushes::pushed_since_clear(&f.paths(), &f.home(), "agent-fixture", "d-9f31c2a4").unwrap(), 1);
    assert_eq!(f.run(&["clear-heat", "d-9f31c2a4"], T2), "Cleared usage for d-9f31c2a4.");
    assert!(f
        .file(&dreams_usage_path(&f.home()))
        .ends_with(&format!("{{\"ts\":\"{T2}\",\"op\":\"clear\",\"id\":\"d-9f31c2a4\",\"agent\":\"agent-fixture\"}}\n")));
    assert_eq!(pushes::pushed_since_clear(&f.paths(), &f.home(), "agent-fixture", "d-9f31c2a4").unwrap(), 0);
    pushes::record(&f.paths(), &f.home(), &delivery_for(T3, "violet-6b1d0e9f2a7c4d31f"), &[pushed("d-9f31c2a4", 0.93), pushed(&a, 0.9)]).unwrap();
    assert_eq!(pushes::pushed_since_clear(&f.paths(), &f.home(), "agent-fixture", "d-9f31c2a4").unwrap(), 1);
    assert_eq!(pushes::pushed_since_clear(&f.paths(), &f.home(), "agent-fixture", &a).unwrap(), 2);
    assert_eq!(pushes::last_push(&f.paths(), "d-9f31c2a4", Some(SESSION)).unwrap().unwrap().ts, T3);
    let listed = f.try_run(true, &["read-mark"], "", T3).unwrap();
    assert!(listed.starts_with(&format!("{a}  used 2 times  2026-10-14  mark a\n{b}  used 0 times")), "{listed}");
    assert!(!f.file(&f.paths().episodes_usage()).contains("d-9f31c2a4"));
}

#[test]
fn record_is_idempotent_per_message_and_item() {
    let f = Fixture::new();
    let a = f.add("mark a", T1);
    let items = [pushed(&a, 0.91), pushed("d-9f31c2a4", 0.88)];
    pushes::record(&f.paths(), &f.home(), &delivery(T1), &items).unwrap();
    // The same message confirmed again, even later and with other probabilities: nothing moves.
    pushes::record(&f.paths(), &f.home(), &delivery(T2), &[pushed(&a, 0.5), pushed("d-9f31c2a4", 0.5)]).unwrap();
    assert_eq!(pushes::read_all(&f.paths()).unwrap().len(), 2);
    assert_eq!(marks::snapshot(&f.paths()).unwrap().marks[0].used, 1);
    assert_eq!(dream_usage::read_all(&f.home()).unwrap().len(), 1);
    // A new item on the same message is new; the known one is still skipped.
    pushes::record(&f.paths(), &f.home(), &delivery(T2), &[pushed(&a, 0.5), pushed("d-7c1e55a0", 0.92)]).unwrap();
    let all = pushes::read_all(&f.paths()).unwrap();
    assert_eq!(all.iter().map(|p| p.id().to_string()).collect::<Vec<_>>(), [a.clone(), "d-9f31c2a4".into(), "d-7c1e55a0".into()]);
    assert_eq!(all[2].ts, T2);
    assert_eq!(dream_usage::read_all(&f.home()).unwrap().len(), 2);
    // Another message with the same items is another delivery.
    pushes::record(&f.paths(), &f.home(), &delivery_for(T3, "violet-other"), &items).unwrap();
    assert_eq!(pushes::read_all(&f.paths()).unwrap().len(), 5);
    assert_eq!(marks::snapshot(&f.paths()).unwrap().marks[0].used, 2);
}

#[test]
fn summaries_count_pushes_only_and_tolerate_a_torn_last_line() {
    let f = Fixture::new();
    assert!(dream_usage::summaries(&f.home()).unwrap().is_empty());
    dream_usage::append_pushes(&f.home(), "agent-a", MESSAGE, T1, &[("d-1", 0.9), ("d-2", 0.95)]).unwrap();
    dream_usage::append_pushes(&f.home(), "agent-b", MESSAGE, T2, &[("d-1", 0.91)]).unwrap();
    dream_usage::append_clear(&f.home(), "agent-a", "d-1", T3).unwrap();
    let path = dreams_usage_path(&f.home());
    fs::write(&path, format!("{}{{\"ts\":\"2026-10-1", f.file(&path))).unwrap();
    let summary = dream_usage::summaries(&f.home()).unwrap();
    assert_eq!(summary.len(), 2);
    assert_eq!(summary["d-1"], dream_usage::DreamUsage { pushed: 2, agents: 2, last: Some("2026-10-15".into()) });
    assert_eq!(summary["d-2"], dream_usage::DreamUsage { pushed: 1, agents: 1, last: Some("2026-10-14".into()) });
    assert_eq!(dream_usage::pushed_since_clear(&f.home(), "agent-a", "d-1").unwrap(), 0);
    assert_eq!(dream_usage::pushed_since_clear(&f.home(), "agent-b", "d-1").unwrap(), 1);
    assert!(dream_usage::has_record(&f.home(), "d-2").unwrap());
    assert!(!dream_usage::has_record(&f.home(), "d-3").unwrap());
    fs::remove_file(&path).unwrap();
    fs::create_dir_all(&path).unwrap();
    assert!(dream_usage::summaries(&f.home()).is_err());
}

#[test]
fn clear_heat_clears_an_episode_ledger_and_reports_nothing_to_clear() {
    let f = Fixture::new();
    assert_eq!(f.run(&["clear-heat", "violet-46d2ec1194aec0a07"], T1), "No usage recorded for violet-46d2ec1194aec0a07.");
    assert_eq!(f.run(&["clear-heat", "d-7c1e55a0"], T1), "No usage recorded for d-7c1e55a0.");
    assert!(!dreams_usage_path(&f.home()).exists());
    episodes::append_usage(
        &f.paths(),
        &[episodes::Usage {
            ts: T1.into(),
            op: "return".into(),
            id: "violet-46d2ec1194aec0a07".into(),
            w: Some(0.5),
            q: Some("固定流程 改动".into()),
        }],
    )
    .unwrap();
    assert_eq!(f.run(&["clear-heat", "violet-46d2ec1194aec0a07"], T2), "Cleared usage for violet-46d2ec1194aec0a07.");
    assert_eq!(
        f.file(&f.paths().episodes_usage()),
        format!("{{\"ts\":\"{T1}\",\"op\":\"return\",\"id\":\"violet-46d2ec1194aec0a07\",\"w\":0.5,\"q\":\"固定流程 改动\"}}\n{{\"ts\":\"{T2}\",\"op\":\"clear\",\"id\":\"violet-46d2ec1194aec0a07\"}}\n")
    );
    assert!(!dreams_usage_path(&f.home()).exists());
}

#[test]
fn project_slug_prefers_the_registered_id_and_hashes_the_root_otherwise() {
    let f = Fixture::new();
    assert_eq!(project_slug(Some("stabruriss-kota"), Some(&f.root)).as_deref(), Some("stabruriss-kota"));
    assert_eq!(project_slug(Some("  "), None), None);
    let slug = project_slug(None, Some(&f.root)).unwrap();
    let name = f.root.file_name().unwrap().to_string_lossy().to_string();
    assert_eq!(slug.len(), name.len() + 7, "{slug}");
    assert!(slug.starts_with(&format!("{name}-")), "{slug}");
    assert!(slug[name.len() + 1..].chars().all(|ch| ch.is_ascii_hexdigit()), "{slug}");
    assert_eq!(project_slug(None, Some(&f.root)).unwrap(), slug);
}

#[test]
fn read_jsonl_skips_unparsable_and_torn_lines() {
    let f = Fixture::new();
    let path = f.root.join("ledger.jsonl");
    assert!(read_jsonl::<episodes::Usage>(&path).unwrap().is_empty());
    fs::write(
        &path,
        "{\"ts\":\"a\",\"op\":\"clear\",\"id\":\"x\"}\nnot json\n{\"ts\":\"b\",\"op\":\"clear\",\"id\":\"y\"}\n{\"ts\":\"c\",\"op\":",
    )
    .unwrap();
    let lines = read_jsonl::<episodes::Usage>(&path).unwrap();
    assert_eq!(lines.iter().map(|line| line.id.as_str()).collect::<Vec<_>>(), ["x", "y"]);
}

#[test]
fn locks_serialize_writers_and_release_on_drop() {
    let f = Fixture::new();
    let path = f.paths().soul_lock();
    let first = super::lock(&path).unwrap();
    let second = thread::spawn({
        let path = path.clone();
        move || super::lock(&path).map(|_| ())
    });
    thread::sleep(Duration::from_millis(150));
    assert!(!second.is_finished());
    drop(first);
    second.join().unwrap().unwrap();
}

fn project(f: &Fixture, name: &str) -> PathBuf {
    let root = f.root.join(name);
    for (id, status) in [("agent-one", "active"), ("agent-two", "archived")] {
        fs::create_dir_all(root.join(".agent-workspaces").join(id)).unwrap();
        fs::write(root.join(".agent-workspaces").join(id).join("agent.yaml"), format!("id: {id}\nstatus: {status}\n")).unwrap();
    }
    fs::write(root.join("workspace.json"), "{\"projectId\":\"project-fixture\",\"localRoot\":\"x\"}").unwrap();
    root
}

fn events(root: &Path, day: &str, lines: &[String]) {
    let dir = root.join("project-memory/chathistory/events");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{day}.jsonl")), format!("{}\n", lines.join("\n"))).unwrap();
}

#[allow(clippy::too_many_arguments)]
fn room_event(id: &str, seq: u64, role: &str, agent: &str, shell: &str, kind: &str, text: &str, origin: Option<&str>, targets: &[&str], overflow: Option<&str>) -> String {
    let mut value = serde_json::json!({
        "id": id, "ts": format!("2026-10-14T02:{seq:02}:05.100+00:00"), "violet_seq": seq, "role": role, "agent_id": agent,
        "shell": shell, "kind": kind, "display": true, "agent_visible": true, "text": text, "source": {"session_id": "s"}
    });
    if let Some(origin) = origin {
        value["message_origin"] = origin.into();
    }
    if !targets.is_empty() {
        value["target_agent_ids"] = serde_json::json!(targets);
    }
    if let Some(path) = overflow {
        value["overflow_path"] = path.into();
    }
    value.to_string()
}

#[test]
fn envelopes_and_mentions_do_not_count() {
    assert_eq!(super::export::strip_envelopes("<KOTA_MARKS v=\"1\">a</KOTA_MARKS>\n hello <KOTA_X>unclosed"), "hello <KOTA_X>unclosed");
    assert_eq!(super::export::countable_chars("@agent-one 好的 @x"), 2);
    assert_eq!(super::export::countable_chars(" 以后除非有我的明确指令，禁止修改 "), 16);
}

#[test]
fn strip_envelopes_only_peels_leading_host_envelopes() {
    use super::export::strip_envelopes;
    // Several host envelopes may lead a message, with whitespace around them.
    assert_eq!(
        strip_envelopes("  <KOTA_TEMPORAL_GAP v=\"1\" current_time=\"2026-10-15T20:00:00Z\">\n3 hours passed\n</KOTA_TEMPORAL_GAP>\n<KOTA_HANDOFF id=\"h1\">\nprevious shell\n</KOTA_HANDOFF>\n<KOTA_MESSAGE id=\"m1\" from=\"a\">\nhi\n</KOTA_MESSAGE>\n正文在这里"),
        "正文在这里"
    );
    // An envelope quoted in the body is the author's text and stays whole.
    let quoted = "老无贴了信封原文：\n```\n<KOTA_MARKS v=\"1\">keep</KOTA_MARKS>\n```\n别动它";
    assert_eq!(strip_envelopes(quoted), quoted);
    // A real envelope in front of such a message is still peeled; the quote survives.
    assert_eq!(strip_envelopes(&format!("<KOTA_MARKS v=\"1\">\n- m1\n</KOTA_MARKS>\n{quoted}")), quoted);
    // Unknown, look-alike or unclosed tags at the front are text too.
    assert_eq!(strip_envelopes("<KOTA_X>not ours</KOTA_X> body"), "<KOTA_X>not ours</KOTA_X> body");
    assert_eq!(strip_envelopes("<KOTA_MESSAGES>look-alike</KOTA_MESSAGES> x"), "<KOTA_MESSAGES>look-alike</KOTA_MESSAGES> x");
    assert_eq!(strip_envelopes("<KOTA_MARKS v=\"1\">unclosed body"), "<KOTA_MARKS v=\"1\">unclosed body");
}

#[test]
fn export_skips_empty_messages_and_own_dream_entries_without_taking_prev() {
    let f = Fixture::new();
    let root = project(&f, "project");
    events(&root, "2026-10-14", &[
        room_event("violet-u0", 1, "user", "agent-one", "kota-local", "message", "第一条正常消息，足够长可以索引了。", Some("user"), &[], None),
        // Only a host envelope, only an attachment marker, this incarnation's own Dream entry, or a runtime error: none is an episode.
        room_event("violet-u1", 2, "user", "agent-one", "kota-local", "message", "<KOTA_TEMPORAL_GAP v=\"1\" current_time=\"2026-10-14T02:02:05Z\">\ngap\n</KOTA_TEMPORAL_GAP>\n", Some("user"), &[], None),
        room_event("violet-u2", 3, "user", "agent-one", "kota-local", "message", "[Image #7]\n\n", Some("user"), &[], None),
        room_event("violet-d1", 4, "assistant", "agent-one", "kota-local", "message", "<KOTA_DREAM_ENTRY>\n- 老无派活时喜欢一次只聊一个话题。\n</KOTA_DREAM_ENTRY>", None, &[], None),
        room_event("violet-e1", 5, "assistant", "agent-one", "kota-local", "message", "Kota Local runtime error: Local RPC command timed out; queued deliveries abandoned.", Some("error"), &[], None),
        room_event("violet-a1", 6, "assistant", "agent-one", "kota-local", "message", "收到。", None, &[], None),
        // A user quoting a Dream entry wholesale is still the user's message.
        room_event("violet-u3", 7, "user", "agent-one", "kota-local", "message", "<KOTA_DREAM_ENTRY>用户贴的条目</KOTA_DREAM_ENTRY>", Some("user"), &[], None),
        room_event("violet-u4", 8, "user", "agent-one", "kota-local", "message", "[Image #8]", Some("user"), &[], None),
    ]);
    let report = super::export::export_with(&root, &f.home(), "老无", T1).unwrap();
    assert_eq!((report.agents[0].messages, report.agents[0].old_dreams, report.agents[0].error.as_deref()), (3, 0, None));
    let paths = SoulPaths::new(&root.join(".agent-workspaces/agent-one"));
    let pool = read_jsonl::<episodes::Episode>(&paths.episodes()).unwrap();
    assert_eq!(pool.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["violet-u0", "violet-a1", "violet-u3"]);
    assert_eq!((pool[0].prev.as_deref(), pool[1].prev.as_deref(), pool[2].prev.as_deref()), (None, Some("violet-u0"), Some("violet-a1")));
    assert_eq!(pool[2].text, "<KOTA_DREAM_ENTRY>用户贴的条目</KOTA_DREAM_ENTRY>");
    // The cursor still moves past what was skipped.
    let state = super::export::read_state(&paths).unwrap();
    let cursor = &state.projects["project-fixture"].cursor;
    assert_eq!((cursor.segment.as_deref(), cursor.violet_seq, state.projects["project-fixture"].episodes), (Some("2026-10-14.jsonl"), 8, 3));
    // Next time, a skipped message that is now behind the cursor still does not become anyone's neighbour.
    let day = root.join("project-memory/chathistory/events/2026-10-14.jsonl");
    let mut text = fs::read_to_string(&day).unwrap();
    text.push_str(&room_event("violet-u5", 9, "user", "agent-one", "kota-local", "message", "游标之后的新消息，够长可以索引。", Some("user"), &[], None));
    text.push('\n');
    fs::write(&day, text).unwrap();
    let again = super::export::export_with(&root, &f.home(), "老无", T2).unwrap();
    assert_eq!(again.agents[0].messages, 1);
    let pool = read_jsonl::<episodes::Episode>(&paths.episodes()).unwrap();
    assert_eq!((pool.len(), pool[3].id.as_str(), pool[3].prev.as_deref()), (4, "violet-u5", Some("violet-u3")));
}

#[test]
fn export_moves_only_messages_by_cursor_and_ignores_archived_dreams() {
    let f = Fixture::new();
    let root = project(&f, "project");
    events(&root, "2026-10-14", &[
        room_event("violet-u1", 1, "user", "agent-one", "kota-local", "message", "<KOTA_TEMPORAL_GAP v=\"1\">gap</KOTA_TEMPORAL_GAP>\n以后除非有我的明确指令，禁止修改已经固定的流程 @agent-one", Some("user"), &[], None),
        room_event("violet-a1", 2, "assistant", "agent-one", "kota-local", "message", "收到。固定流程清单记在收据里。", None, &[], None),
        room_event("violet-b1", 3, "assistant", "agent-one", "system", "message", "总线消息，不入池。", Some("a2a"), &["agent-two"], None),
        room_event("violet-u2", 4, "user", "agent-one", "kota-local", "message", "好的", Some("user"), &[], None),
        room_event("violet-u0", 5, "user", "agent-one", "kota-local", "message", "旧消息没有来源标签，不入池。", None, &[], None),
        room_event("violet-t1", 6, "assistant", "laughing-man", "system", "message", "把方案交回给座山雕 review。", Some("telegram"), &["agent-one"], None),
        room_event("violet-c1", 7, "assistant", "agent-one", "kota-local", "commentary", "思考中，不入池。", None, &[], None),
        room_event("violet-o1", 8, "user", "agent-two", "kota-local", "message", "给二号的消息，一号不该收到。", Some("user"), &[], None),
    ]);
    fs::create_dir_all(root.join("project-memory/chathistory/overflow")).unwrap();
    fs::write(root.join("project-memory/chathistory/overflow/violet-a2.md"), "FULL TEXT 全文在溢出文件里，预览只是开头。").unwrap();
    events(&root, "2026-10-15", &[room_event("violet-a2", 9, "assistant", "agent-one", "kota-local", "message", "FULL TEXT…", None, &[], Some("project-memory/chathistory/overflow/violet-a2.md"))]);
    fs::create_dir_all(f.home().join("dreams")).unwrap();
    let old = "# Old Dreams\n\n## 2026-10-09T09:00:00.000+00:00\n- 老无派活时喜欢一次只聊一个话题，聊完清点再开工。\n- \"第二条带引号\"\n\n";
    fs::write(f.home().join("dreams/old_dreams.md"), old).unwrap();

    let report = super::export::export_with(&root, &f.home(), "老无", T1).unwrap();
    assert_eq!(report.agents.len(), 1);
    let row = &report.agents[0];
    assert_eq!((row.agent_id.as_str(), row.messages, row.old_dreams, row.error.as_deref()), ("agent-one", 5, 0, None));
    let paths = SoulPaths::new(&root.join(".agent-workspaces/agent-one"));
    let pool = read_jsonl::<episodes::Episode>(&paths.episodes()).unwrap();
    assert_eq!(pool.iter().map(|e| e.id.as_str()).collect::<Vec<_>>()[..5], ["violet-u1", "violet-a1", "violet-u2", "violet-t1", "violet-a2"]);
    let u1 = &pool[0];
    assert_eq!(
        (u1.role.as_deref(), u1.origin.as_deref(), u1.project.as_deref(), u1.ts.as_str(), u1.from.as_deref(), u1.prev.as_deref(), u1.indexed, u1.w0),
        (Some("user"), Some("user"), Some("project-fixture"), "2026-10-14T02:01:05Z", Some("老无"), None, true, Some(0.5))
    );
    assert_eq!(u1.text, "以后除非有我的明确指令，禁止修改已经固定的流程 @agent-one");
    assert_eq!((pool[1].from.as_deref(), pool[1].origin.as_deref(), pool[1].prev.as_deref()), (Some("self"), Some("end_turn"), Some("violet-u1")));
    assert_eq!((pool[2].indexed, pool[2].w0, pool[2].prev.as_deref()), (false, None, Some("violet-a1")));
    assert_eq!((pool[3].from.as_deref(), pool[3].origin.as_deref(), pool[3].prev.as_deref(), pool[3].role.as_deref()), (Some("老无"), Some("telegram"), Some("violet-u2"), Some("user")));
    assert!(pool[4].text.starts_with("FULL TEXT 全文") && pool[4].prev.is_none(), "{:?}", pool[4]);
    assert_eq!(pool.len(), 5);
    let short = f.file(&paths.episodes()).lines().nth(2).unwrap().to_string();
    assert!(short.ends_with("\"text\":\"好的\",\"indexed\":false}"), "{short}");
    let state = super::export::read_state(&paths).unwrap();
    assert_eq!(state.current_project.as_deref(), Some("project-fixture"));
    let project_state = &state.projects["project-fixture"];
    assert_eq!(
        (project_state.cursor.segment.as_deref(), project_state.cursor.violet_seq, project_state.episodes, project_state.stints.len(), project_state.last_export.as_deref()),
        (Some("2026-10-15.jsonl"), 9, 5, 1, Some(T1))
    );
    assert!(!f.file(&paths.episodes_state()).contains("old_dreams"));
    // Idempotent: nothing moves twice.
    let again = super::export::export_with(&root, &f.home(), "老无", T2).unwrap();
    assert_eq!((again.agents[0].messages, again.agents[0].old_dreams), (0, 0));
    assert_eq!(read_jsonl::<episodes::Episode>(&paths.episodes()).unwrap().len(), 5);
    // Only what came after the cursors moves next time.
    let day2 = root.join("project-memory/chathistory/events/2026-10-15.jsonl");
    let mut text = fs::read_to_string(&day2).unwrap();
    text.push_str(&room_event("violet-u3", 10, "user", "agent-one", "kota-local", "message", "新一条用户消息，游标之后才搬。", Some("puppeteer"), &[], None));
    text.push('\n');
    fs::write(&day2, text).unwrap();
    fs::write(f.home().join("dreams/old_dreams.md"), format!("{old}## 2026-10-15T01:00:00+00:00\n- 第三条新归档。\n")).unwrap();
    let third = super::export::export_with(&root, &f.home(), "老无", T3).unwrap();
    assert_eq!((third.agents[0].messages, third.agents[0].old_dreams), (1, 0));
    let pool = read_jsonl::<episodes::Episode>(&paths.episodes()).unwrap();
    assert_eq!((pool[5].id.as_str(), pool[5].prev.as_deref(), pool[5].origin.as_deref()), ("violet-u3", Some("violet-a2"), Some("puppeteer")));
    assert_eq!(super::export::read_state(&paths).unwrap().projects["project-fixture"].cursor.violet_seq, 10);
}

#[test]
fn legacy_old_dream_cursor_and_replaced_by_remain_readable_without_new_writes() {
    let state: super::export::State = serde_json::from_str(
        r#"{"version":1,"current_project":"project-fixture","projects":{},"old_dreams":{"lines":7}}"#,
    ).unwrap();
    assert_eq!(state.current_project.as_deref(), Some("project-fixture"));
    assert!(!serde_json::to_string(&state).unwrap().contains("old_dreams"));
    let episode: episodes::Episode = serde_json::from_str(POOL[3]).unwrap();
    assert_eq!(episode.replaced_by.as_deref(), Some("m-20261002-9c17"));
    assert!(!serde_json::to_string(&episode).unwrap().contains("replaced_by"));
}

#[test]
fn carry_copies_the_soul_tolerantly_and_leaves_the_index_behind() {
    let f = Fixture::new();
    let from = f.root.join("from");
    let paths = SoulPaths::new(&from);
    f.try_run(false, &["add-mark", "carried mark"], "", T1).unwrap();
    let own = SoulPaths::new(&f.agent());
    fs::create_dir_all(paths.soul_dir()).unwrap();
    fs::copy(own.marks(), paths.marks()).unwrap();
    pushes::record(&own, &f.home(), &delivery(T1), &[pushed("d-1", 0.9)]).unwrap();
    fs::copy(own.pushes(), paths.pushes()).unwrap();
    fs::create_dir_all(paths.episodes_dir()).unwrap();
    fs::write(paths.episodes(), format!("{}\n{{\"id\":\"torn\",\"ki", POOL[0])).unwrap();
    fs::write(paths.episodes_usage(), "{\"ts\":\"2026-10-14T07:02:11Z\",\"op\":\"return\",\"id\":\"violet-u1\",\"w\":0.5,\"q\":\"x\"}\nbroken line\n").unwrap();
    fs::write(paths.episodes_index(), b"cache").unwrap();
    let to = f.root.join("to");
    let project = super::carry::NewProject { slug: "project-two", name: "two", root: Path::new("/tmp/two") };
    let counts = super::carry::carry(&from, &to, Some(project), T2).unwrap();
    assert_eq!(counts, super::carry::SoulCounts { marks: 1, episodes: 1 });
    let dst = SoulPaths::new(&to);
    assert_eq!(fs::read(dst.marks()).unwrap(), fs::read(paths.marks()).unwrap());
    assert_eq!(f.file(&dst.pushes()), f.file(&paths.pushes()));
    assert_eq!(f.file(&dst.episodes()), format!("{}\n", POOL[0]));
    assert_eq!(f.file(&dst.episodes_usage()).lines().count(), 1);
    assert!(!dst.episodes_index().exists());
    let state = super::export::read_state(&dst).unwrap();
    assert_eq!(state.current_project.as_deref(), Some("project-two"));
    assert_eq!(state.projects["project-two"].stints[0].from, T2);
    assert!(super::carry::carry(&from, &to, None, T2).unwrap_err().contains("already has an under-table"));
    // A root without a soul carries nothing but still gets its stint.
    let empty_to = f.root.join("empty-to");
    super::carry::carry(&f.root.join("nothing"), &empty_to, Some(project), T2).unwrap();
    assert!(!SoulPaths::new(&empty_to).marks().exists());
    assert_eq!(super::export::read_state(&SoulPaths::new(&empty_to)).unwrap().projects.len(), 1);
}

#[test]
fn export_writes_state_for_an_incarnation_with_nothing_to_move() {
    let f = Fixture::new();
    let root = project(&f, "fresh");
    let report = super::export::export_with(&root, &f.home(), "老无", T1).unwrap();
    assert_eq!((report.agents[0].messages, report.agents[0].old_dreams, report.agents[0].error.as_deref()), (0, 0, None));
    let paths = SoulPaths::new(&root.join(".agent-workspaces/agent-one"));
    assert!(paths.episodes_state().is_file());
    assert_eq!(super::export::read_state(&paths).unwrap().current_project.as_deref(), Some("project-fixture"));
}

#[test]
fn export_without_chathistory_ignores_dreams_and_isolates_one_incarnation() {
    let f = Fixture::new();
    let root = project(&f, "quiet");
    fs::write(root.join(".agent-workspaces/agent-two/agent.yaml"), "id: agent-two\nstatus: active\n").unwrap();
    fs::create_dir_all(f.home().join("dreams")).unwrap();
    fs::write(f.home().join("dreams/old_dreams.md"), "# Old Dreams\n\n## 2026-10-09T09:00:00Z\n- 一条归档的梦。\n").unwrap();
    // agent-one's pool path is a directory: its export fails; agent-two's goes on.
    fs::create_dir_all(root.join(".agent-workspaces/agent-one/under-table/episodes/episodes.jsonl")).unwrap();
    let report = super::export::export_with(&root, &f.home(), "老无", T1).unwrap();
    let rows = report.agents.iter().map(|r| (r.agent_id.as_str(), r.messages, r.old_dreams, r.error.is_some())).collect::<Vec<_>>();
    assert_eq!(rows, [("agent-one", 0, 0, true), ("agent-two", 0, 0, false)]);
    let two = SoulPaths::new(&root.join(".agent-workspaces/agent-two"));
    assert!(read_jsonl::<episodes::Episode>(&two.episodes()).unwrap().is_empty());
    assert!(super::export::read_state(&two).unwrap().projects["project-fixture"].cursor.segment.is_none());
    assert!(super::export::export_with(&f.root.join("nowhere"), &f.home(), "老无", T1).unwrap().agents.is_empty());
}

#[test]
fn concurrent_exports_of_one_incarnation_take_turns_and_pool_each_message_once() {
    let f = Fixture::new();
    let root = project(&f, "project");
    events(&root, "2026-10-14", &[
        room_event("violet-u1", 1, "user", "agent-one", "kota-local", "message", "倒计时开始前的最后一条用户消息。", Some("user"), &[], None),
        room_event("violet-a1", 2, "assistant", "agent-one", "kota-local", "message", "收到，这条回复也要进往事池。", None, &[], None),
    ]);
    let paths = SoulPaths::new(&root.join(".agent-workspaces/agent-one"));
    // The countdown's export and the one after consolidation both arrive while a third holds the lock.
    let held = super::lock(&paths.export_lock()).unwrap();
    let runs = [T1, T2].map(|now| {
        let (root, home) = (root.clone(), f.home());
        thread::spawn(move || super::export::export_with(&root, &home, "老无", now))
    });
    thread::sleep(Duration::from_millis(150));
    assert!(runs.iter().all(|run| !run.is_finished()));
    assert!(!paths.episodes().exists());
    drop(held);
    let rows = runs.map(|run| run.join().unwrap().unwrap().agents.remove(0));
    assert!(rows.iter().all(|row| row.error.is_none()), "{rows:?}");
    let mut moved = rows.iter().map(|row| row.messages).collect::<Vec<_>>();
    moved.sort();
    assert_eq!(moved, [0, 2]);
    let pool = read_jsonl::<episodes::Episode>(&paths.episodes()).unwrap();
    assert_eq!(pool.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["violet-u1", "violet-a1"]);
}
