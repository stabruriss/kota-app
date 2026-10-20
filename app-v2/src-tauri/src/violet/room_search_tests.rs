use super::*;

struct RoomFixture(PathBuf);

impl RoomFixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("fable-search-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn write(&self, events: Vec<ChathistoryEvent>) {
        let dir = chathistory_events_dir(&self.0);
        fs::create_dir_all(&dir).unwrap();
        let mut days: BTreeMap<String, Vec<ChathistoryEvent>> = BTreeMap::new();
        for event in events {
            days.entry(chathistory_day_key(&event.ts))
                .or_default()
                .push(event);
        }
        for (day, events) in days {
            fs::write(
                dir.join(format!("{day}.jsonl")),
                render_chathistory_events(&dedupe_chathistory_events(events)).unwrap(),
            )
            .unwrap();
        }
    }

    fn search(&self, query: &str, human_only: bool) -> VioletRoomSearchResult {
        let mut request = request(query);
        request.human_only = human_only;
        search_room(&self.0, request).unwrap()
    }

    fn around(&self, id: &str, before: usize, after: usize) -> Result<VioletRoomState, String> {
        // Exercise the actual read-cache dispatcher, not only its helper.
        read_cache(
            &self.0,
            serde_json::from_value(serde_json::json!({
                "around": { "id": id, "before": before, "after": after }
            }))
            .unwrap(),
        )
    }
}

impl Drop for RoomFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn event(id: &str, text: &str) -> ChathistoryEvent {
    serde_json::from_value(serde_json::json!({
        "id": id, "ts": "2026-04-12T10:00:00Z", "violet_seq": 1,
        "role": "assistant", "agent_id": "agent-fable73", "shell": "codex",
        "kind": "message", "display": true, "agent_visible": false, "text": text,
        "source": { "session_id": "session-fable73", "path": "/synthetic/fable/rollout.jsonl",
            "native_event_id": format!("native-{id}"), "line_start": 1, "line_end": 1 },
        "agent_display_name": "Fable", "agent_avatar_id": "fable-avatar",
        "agent_provider": "codex", "model": "fable-model", "effort": "high",
        "target_agent_ids": ["agent-fable74"], "message_origin": "native",
        "agent_status": "active"
    }))
    .unwrap()
}

fn request(query: &str) -> VioletRoomSearchRequest {
    serde_json::from_value(serde_json::json!({ "projectRoot": "/synthetic/fable", "query": query }))
        .unwrap()
}

#[test]
fn ember_dream_acknowledgements_stay_out_of_search_when_shown_in_room() {
    let fixture = RoomFixture::new();
    fixture.write(vec![
        event("dream-ack", "<KOTA_DREAM_ENTRY>needle</KOTA_DREAM_ENTRY>"),
        event("ordinary", "needle in a normal reply"),
    ]);
    assert_eq!(ids(&fixture.search("needle", false).hits), ["ordinary"]);
    assert!(fixture.around("dream-ack", 1, 1).is_err());
    let request: VioletRoomRequest = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(read_chathistory_messages(&fixture.0, &request).unwrap().len(), 2);
}

#[test]
fn temporal_composer_search_and_preview_use_the_saved_user_message() {
    let fixture = RoomFixture::new();
    let prompt = "<KOTA_TEMPORAL_GAP v=\"1\" current_time=\"2026-04-12T10:00:00Z\">\nIt has been over 24 hours since your last completed response in this room.\n</KOTA_TEMPORAL_GAP>\n";
    let mut saved = event("fable-composer", "amber original");
    saved.role = "user".into();
    saved.agent_id = "user".into();
    saved.shell = "composer".into();
    saved.temporal_gap = Some(crate::temporal_context::ComposerTemporalGap {
        current_time: "2026-04-12T10:00:00Z".into(),
        prompt: prompt.into(),
        target_agent_ids: vec!["agent-fable73".into()],
        elapsed_days_by_target: BTreeMap::from([("agent-fable73".into(), 2)]),
    });
    let mut echo = event("fable-echo", &format!("[Image #1]{prompt}amber original"));
    echo.role = "user".into();
    fixture.write(vec![saved, echo]);
    let results = fixture.search("amber", true);
    assert_eq!(ids(&results.hits), vec!["fable-composer"]);
    assert!(results.hits[0].temporal_gap.is_some());
    assert!(fixture.search("completed response", false).hits.is_empty());
    let preview = fixture.around("fable-composer", 15, 15).unwrap();
    assert_eq!(ids(&preview.messages), vec!["fable-composer"]);
    assert!(fixture.around("fable-echo", 15, 15).is_err());
}

fn ids(messages: &[VioletChatMessage]) -> Vec<&str> {
    messages.iter().map(|message| message.id.as_str()).collect()
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if path.is_dir() {
                out.insert(relative, Vec::new());
                visit(root, &path, out);
            } else {
                out.insert(relative, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

#[test]
fn search_supports_and_phrases_case_unicode_and_single_chinese_character() {
    let fixture = RoomFixture::new();
    fixture.write(vec![
        event("a", "Alpha RED fox 茶点 Éclair"),
        event("b", "ALPHA fox red 茶杯"),
        event("c", "red fox only"),
    ]);
    for (query, expected) in [
        ("alpha fox", vec!["b", "a"]),
        ("alpha \"red fox\"", vec!["a"]),
        ("\"RED FOX\"", vec!["c", "a"]),
        ("茶", vec!["b", "a"]),
        ("éCLAIR", vec!["a"]),
        ("  alpha\talpha  \"red fox\" ", vec!["a"]),
        ("\"red fox", vec!["c", "a"]),
    ] {
        let result = fixture.search(query, false);
        assert_eq!(ids(&result.hits), expected, "{query}");
        assert_eq!(result.total, Some(expected.len()));
        assert!(!result.truncated);
        assert!(result.next_cursor.is_none());
    }
    assert_eq!(
        fixture
            .search(" Alpha alpha \"red fox\" ", false)
            .match_terms,
        ["Alpha", "red fox"]
    );
    let empty = fixture.search("  \"\"  ", false);
    assert!(empty.hits.is_empty() && empty.match_terms.is_empty());
    assert_eq!(empty.total, Some(0));
}

#[test]
fn search_prefilter_handles_json_escapes_and_only_matches_decoded_message_text() {
    let fixture = RoomFixture::new();
    let plain = event("plain", "茶 /path say \"yes\"\nnext line \\file");
    fixture.write(vec![plain.clone()]);
    let mut escaped = plain;
    escaped.id = "escaped".into();
    escaped.violet_seq = Some(2);
    escaped.text.push_str(" alternate");
    let escaped = serde_json::to_string(&escaped)
        .unwrap()
        .replace("茶", "\\u8336")
        .replace("/path", "\\/path");
    let file = chathistory_events_dir(&fixture.0).join("2026-04-12.jsonl");
    let mut bytes = fs::read(&file).unwrap();
    bytes.extend_from_slice(format!("{escaped}\n{{ broken json\n").as_bytes());
    bytes.extend_from_slice(&[0xff, b'\n']);
    fs::write(file, bytes).unwrap();
    for query in ["茶", "/path", "yes", "\\file", "\"yes\" \"next line\""] {
        assert_eq!(
            ids(&fixture.search(query, false).hits),
            ["escaped", "plain"],
            "{query}"
        );
    }
    assert!(fixture.search("fable-model", false).hits.is_empty());
    assert!(fixture.search("/synthetic/fable", false).hits.is_empty());
}

#[test]
fn byte_prefilter_preserves_unicode_case_and_guards_ascii_expansion_fallbacks() {
    let expansions: Vec<char> = (128..=0x10ffff)
        .filter_map(char::from_u32)
        .filter(|ch| ch.to_lowercase().any(|lower| lower.is_ascii()))
        .collect();
    let mut expected = ASCII_CASE_EXPANSIONS.to_vec();
    expected.sort_unstable();
    assert_eq!(expansions, expected);
    let fixture = RoomFixture::new();
    fixture.write(vec![event("unicode", "Éclair Kelvin İstanbul ΑΛΦΑ 中文")]);
    for query in ["éCLAIR", "kelvin", "istan", "i", "αΛΦΑ", "中"] {
        let result = fixture.search(query, false);
        if query == "istan" {
            // Unicode lowercase is i + combining dot, not accent normalization.
            assert!(result.hits.is_empty());
        } else {
            assert_eq!(ids(&result.hits), ["unicode"], "{query}");
        }
    }
}

#[test]
fn searchable_set_and_human_only_are_shared_backend_policy_without_rewriting_dtos() {
    let fixture = RoomFixture::new();
    let mut allowed = Vec::new();
    let mut human = event(
        "human",
        "needle <KOTA_QUOTE_REF>literal quote</KOTA_QUOTE_REF>",
    );
    human.role = "user".into();
    allowed.push(human);
    let mut telegram = event("telegram", "needle telegram body");
    telegram.shell = "system".into();
    telegram.agent_provider = Some("system".into());
    telegram.agent_id = "laughing-man".into();
    telegram.actor_intent = Some("telegram".into());
    allowed.push(telegram.clone());
    for shell in ["claude", "codex", "antigravity", "opencode", "kimi", "pi"] {
        let mut end_turn = event(shell, &format!("needle end turn {shell}"));
        end_turn.shell = shell.into();
        allowed.push(end_turn);
    }
    for intent in [
        Some("handoff"),
        Some("review"),
        Some("status"),
        Some("task"),
        None,
    ] {
        let mut bus = event(
            intent.unwrap_or("legacy-bus"),
            &format!("needle bus body {}", intent.unwrap_or("legacy")),
        );
        bus.shell = "system".into();
        bus.agent_provider = Some("system".into());
        bus.actor_intent = intent.map(str::to_owned);
        bus.source.native_event_id = Some(format!("agentbus-fable-{}", bus.id));
        if intent.is_none() {
            bus.agent_provider = Some("opencode".into());
        }
        allowed.push(bus);
    }
    let mut reminder = event("reminder", "needle reminder body");
    reminder.shell = "system".into();
    reminder.agent_id = "ember".into();
    reminder.actor_intent = Some("reminder".into());
    allowed.push(reminder);
    fixture.write(allowed.clone());
    let result = fixture.search("needle", false);
    assert_eq!(result.hits.len(), allowed.len());
    assert!(result
        .hits
        .iter()
        .any(|hit| hit.id == "legacy-bus" && hit.agent_provider.as_deref() == Some("opencode")));
    for expected in allowed {
        let actual = result
            .hits
            .iter()
            .find(|hit| hit.id == expected.id)
            .unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(message_from_chathistory_event(expected)).unwrap()
        );
    }
    let result = fixture.search("needle", true);
    assert_eq!(ids(&result.hits), ["telegram", "human"]);
    assert_eq!(result.hits[0].role, "assistant");
    assert_eq!(result.hits[0].agent_id, "laughing-man");
    assert_eq!(result.hits[0].actor_intent.as_deref(), Some("telegram"));
}

#[test]
fn search_excludes_process_messages_wrappers_dreams_system_actors_and_hidden_entries() {
    let fixture = RoomFixture::new();
    let mut excluded = Vec::new();
    for kind in [
        "commentary",
        "tool",
        "thinking",
        "progress",
        "control",
        "compaction",
        "interrupt",
        "artifact",
    ] {
        let mut row = event(kind, "needle");
        row.kind = kind.into();
        excluded.push(row);
    }
    for (id, patch) in [
        ("hidden", serde_json::json!({ "display": false })),
        ("system", serde_json::json!({ "role": "system" })),
        (
            "handoff",
            serde_json::json!({ "role": "user", "message_origin": "shell_handoff" }),
        ),
        (
            "telegram-wrapper",
            serde_json::json!({ "agent_id": "laughing-man" }),
        ),
        (
            "ember-without-intent",
            serde_json::json!({ "agent_id": "ember" }),
        ),
        ("bbs", serde_json::json!({ "actor_intent": "bbs-thread" })),
        (
            "skipped",
            serde_json::json!({ "actor_intent": "delivery-skipped" }),
        ),
        (
            "old-skipped",
            serde_json::json!({ "source": { "session_id": "fable", "native_event_id": "agentbus-fable:skipped" } }),
        ),
        ("summary", serde_json::json!({ "agent_id": "violet" })),
        ("conflict", serde_json::json!({ "agent_id": "bartender" })),
        (
            "unknown-shell",
            serde_json::json!({ "shell": "fable-unknown" }),
        ),
        (
            "non-bus-system",
            serde_json::json!({ "shell": "system", "agent_provider": "system" }),
        ),
    ] {
        let mut value = serde_json::to_value(event(id, "needle")).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        excluded.push(serde_json::from_value(value).unwrap());
    }
    for (i, text) in [
        "<KOTA_MESSAGE id=\"agentbus-fable\">needle</KOTA_MESSAGE>",
        "[Image #1]\n<KOTA_MESSAGE>needle</KOTA_MESSAGE>",
        "You are replying to a Kota BBS thread. needle",
        "You are creating a Kota BBS thread. needle",
        "It's time to dream. needle",
        "<KOTA_DREAM_ENTRY>needle</KOTA_DREAM_ENTRY>",
        "<system-reminder>needle</system-reminder>",
        "<permissions instructions>needle</permissions instructions>",
        "<codex_internal_context>needle</codex_internal_context>",
        "<turn_aborted>needle interrupted the previous turn</turn_aborted>",
    ]
    .iter()
    .enumerate()
    {
        let mut row = event(&format!("wrapper-{i}"), text);
        row.role = "user".into();
        excluded.push(row);
    }
    fixture.write(excluded);
    for human_only in [false, true] {
        let result = fixture.search("needle", human_only);
        assert!(
            result.hits.is_empty(),
            "unexpected IDs: {:?}",
            ids(&result.hits)
        );
        assert_eq!(result.total, Some(0));
    }
}

#[test]
fn search_cursor_pages_reverse_dates_and_lines_without_skipping_counted_hits() {
    let fixture = RoomFixture::new();
    let mut rows = Vec::new();
    for i in 0..7 {
        let mut row = event(&format!("hit-{i}"), &format!("needle {i}"));
        row.ts = format!("2026-04-{}T10:00:00Z", if i < 3 { "11" } else { "12" });
        row.violet_seq = Some(i + 1);
        rows.push(row);
    }
    fixture.write(rows);
    let before = snapshot(&fixture.0);
    let mut req = request("needle");
    req.limit = Some(2);
    let first = search_room(&fixture.0, req.clone()).unwrap();
    assert_eq!(ids(&first.hits), ["hit-6", "hit-5"]);
    assert_eq!(first.total, Some(7));
    assert_eq!(first.next_cursor.as_deref(), Some("2026-04-12:3"));
    let mut all = first.hits;
    req.cursor = first.next_cursor;
    while req.cursor.is_some() {
        let next = search_room(&fixture.0, req.clone()).unwrap();
        assert!(next.total.is_none());
        assert!(!next.truncated);
        all.extend(next.hits);
        req.cursor = next.next_cursor;
    }
    assert_eq!(
        ids(&all),
        ["hit-6", "hit-5", "hit-4", "hit-3", "hit-2", "hit-1", "hit-0"]
    );
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn counting_budget_never_truncates_hits_and_total_is_omitted_when_inexact() {
    let fixture = RoomFixture::new();
    fixture.write(
        (0..5)
            .map(|i| event(&format!("hit-{i}"), &format!("needle {i}")))
            .collect(),
    );
    let mut req = request("needle");
    req.limit = Some(2);
    let first = search_with_budget(&fixture.0, req.clone(), StdDuration::ZERO).unwrap();
    assert_eq!(ids(&first.hits), ["hit-4", "hit-3"]);
    assert!(first.truncated && first.total.is_none());
    let json = serde_json::to_value(&first).unwrap();
    assert!(json.get("total").is_none());
    req.cursor = first.next_cursor;
    let next = search_with_budget(&fixture.0, req, StdDuration::ZERO).unwrap();
    assert_eq!(ids(&next.hits), ["hit-2", "hit-1"]);
    assert!(!next.truncated);
    let mut sparse = request("needle");
    sparse.limit = Some(10);
    let sparse = search_with_budget(&fixture.0, sparse, StdDuration::ZERO).unwrap();
    assert_eq!(sparse.hits.len(), 5);
    assert_eq!(sparse.total, Some(5));
    assert!(!sparse.truncated);
}

#[test]
fn preview_uses_id_windows_and_its_own_collection_without_changing_ordinary_before() {
    let fixture = RoomFixture::new();
    let mut rows = Vec::new();
    for (i, kind) in [
        "message",
        "compaction",
        "commentary",
        "message",
        "interrupt",
        "tool",
        "message",
    ]
    .iter()
    .enumerate()
    {
        let mut row = event(&format!("row-{i}"), &format!("needle {i}"));
        row.kind = (*kind).into();
        if matches!(*kind, "compaction" | "interrupt") {
            row.role = "system".into();
        }
        row.ts = format!("2026-04-{}T10:00:00Z", if i < 3 { "11" } else { "12" });
        row.violet_seq = Some(i as u64 + 1);
        rows.push(row);
    }
    fixture.write(rows);
    let before = snapshot(&fixture.0);
    assert_eq!(
        ids(&fixture.around("row-3", 1, 1).unwrap().messages),
        ["row-1", "row-3", "row-4"]
    );
    assert_eq!(
        ids(&fixture.around("row-1", 15, 0).unwrap().messages),
        ["row-0", "row-1"]
    );
    assert_eq!(
        ids(&fixture.around("row-4", 0, 15).unwrap().messages),
        ["row-4", "row-6"]
    );
    assert_eq!(
        ids(&fixture.around("row-0", 15, 15).unwrap().messages),
        ["row-0", "row-1", "row-3", "row-4", "row-6"]
    );
    assert_eq!(
        ids(&fixture.around("row-6", 0, 0).unwrap().messages),
        ["row-6"]
    );
    assert!(fixture
        .around("missing", 15, 15)
        .unwrap_err()
        .contains("not found"));
    assert!(fixture
        .around("row-2", 15, 15)
        .unwrap_err()
        .contains("not in the preview collection"));
    let normal = read_cache(
        &fixture.0,
        serde_json::from_value(serde_json::json!({
            "before": "2026-04-12T10:00:00Z", "limit": 50
        }))
        .unwrap(),
    )
    .unwrap();
    // Ordinary room loading still includes commentary and keeps its timestamp boundary.
    assert_eq!(ids(&normal.messages), ["row-0", "row-1", "row-2"]);
    assert!(normal.resolved_target_id.is_none());
    assert!(serde_json::to_value(&normal)
        .unwrap()
        .get("resolvedTargetId")
        .is_none());
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn search_deduplicates_each_page_but_counts_raw_events_and_keeps_its_cursor() {
    let fixture = RoomFixture::new();
    let mut rows = Vec::new();
    for (id, text, ts) in [
        ("earliest", "needle other", "2026-04-12T09:58:00Z"),
        ("kept", "Needle repeated\nbody", "2026-04-12T10:00:00Z"),
        ("hidden", "needle REPEATED body", "2026-04-12T10:00:01Z"),
        ("latest", "needle fresh", "2026-04-12T10:01:00Z"),
    ] {
        let mut row = event(id, text);
        row.ts = ts.into();
        rows.push(row);
    }
    fixture.write(rows);
    let mut req = request("needle");
    req.limit = Some(3);
    let first = search_room(&fixture.0, req.clone()).unwrap();
    assert_eq!(ids(&first.hits), ["latest", "kept"]);
    assert_eq!(first.hits[1].text, "Needle repeated\nbody");
    assert_eq!(first.total, Some(4)); // Includes the record hidden by room deduplication.
    assert_eq!(first.next_cursor.as_deref(), Some("2026-04-12:2"));
    let budgeted = search_with_budget(&fixture.0, req.clone(), StdDuration::ZERO).unwrap();
    assert_eq!(ids(&budgeted.hits), ["latest", "kept"]);
    assert!(budgeted.truncated && budgeted.total.is_none());
    assert_eq!(budgeted.next_cursor, first.next_cursor);
    req.cursor = first.next_cursor;
    let next = search_room(&fixture.0, req).unwrap();
    assert_eq!(ids(&next.hits), ["earliest"]);
    assert!(next.next_cursor.is_none());
}

#[test]
fn preview_resolves_a_deduplicated_anchor_to_the_rooms_retained_id() {
    let fixture = RoomFixture::new();
    let older = event("kept", "Needle repeated\nbody");
    let mut newer = event("hidden", "needle REPEATED body");
    newer.ts = "2026-04-12T10:00:01Z".into();
    fixture.write(vec![older, newer]);
    let before = snapshot(&fixture.0);
    let preview = fixture.around("hidden", 15, 15).unwrap();
    assert_eq!(ids(&preview.messages), ["kept"]);
    assert_eq!(preview.resolved_target_id.as_deref(), Some("kept"));
    assert_eq!(
        serde_json::to_value(&preview).unwrap()["resolvedTargetId"],
        "kept"
    );
    let normal = read_cache(
        &fixture.0,
        serde_json::from_value(serde_json::json!({})).unwrap(),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&preview.messages).unwrap(),
        serde_json::to_value(&normal.messages).unwrap()
    );
    assert_eq!(
        fixture
            .around("kept", 0, 15)
            .unwrap()
            .resolved_target_id
            .as_deref(),
        Some("kept")
    );
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn absent_history_invalid_cursors_and_preview_errors_do_not_write_anything() {
    let fixture = RoomFixture::new();
    let before = snapshot(&fixture.0);
    assert_eq!(fixture.search("needle", false).total, Some(0));
    assert!(fixture.around("absent", 0, 0).is_err());
    for cursor in [
        "../elsewhere:1",
        "2026-02-31:1",
        "2026-04-12:0",
        "2026-04-12:-1",
        "2026-04-12:abc",
        "2026-04-12:1:2",
    ] {
        let mut req = request("needle");
        req.cursor = Some(cursor.into());
        assert!(search_room(&fixture.0, req)
            .unwrap_err()
            .contains("Invalid room search cursor"));
    }
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
#[ignore = "manual read-only benchmark; requires KOTA_ROOM_SEARCH_BENCH_ROOT"]
fn room_search_benchmark() {
    let root = PathBuf::from(std::env::var("KOTA_ROOM_SEARCH_BENCH_ROOT").unwrap());
    let files = segments(&root).unwrap();
    let bytes: u64 = files
        .iter()
        .map(|file| fs::metadata(&file.path).unwrap().len())
        .sum();
    println!("segments={} bytes={bytes}", files.len());
    for query in ["同步", "a", "\"local test\"", "fable-search-absent-token"] {
        for pass in 0..4 {
            let start = Instant::now();
            let result = search_room(&root, request(query)).unwrap();
            println!(
                "query={query:?} pass={pass} ms={:.3} hits={} total={:?} truncated={}",
                start.elapsed().as_secs_f64() * 1000.0,
                result.hits.len(),
                result.total,
                result.truncated
            );
        }
    }
}
