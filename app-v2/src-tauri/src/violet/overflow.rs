//! Long native replies: bounded history/IPC, with one complete Markdown file.
//! Identity always uses the old 10k transformation; existing history is frozen.
use super::*;

const LEGACY_TEXT_CHARS: usize = 10_000;
// A defensive file limit, not a promise about provider JSONL cold-scan limits.
const MAX_OVERFLOW_CHARS: usize = 8_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct TextIdentity {
    pub message_id: String,
    pub native_dedupe_hash: String,
    pub room_dedupe_hash: String,
}

#[derive(Clone)]
pub(super) struct PendingText {
    legacy: String,
    full: Option<Arc<str>>,
    oversized: bool,
}

// Never print a full transient response, including on failed test assertions.
impl std::fmt::Debug for PendingText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingText").finish_non_exhaustive()
    }
}

pub(super) fn prepare_text(text: String, role: &str, kind: &str) -> (String, Option<PendingText>) {
    if text.chars().nth(LEGACY_TEXT_CHARS).is_none() {
        return (text, None);
    }
    let legacy = truncate_chars(&text, LEGACY_TEXT_CHARS);
    let long = text.chars().nth(MAX_EVENT_TEXT_CHARS).is_some();
    let preview = text.chars().take(MAX_EVENT_TEXT_CHARS).collect();
    let candidate = long && role == "assistant" && kind == "message";
    let oversized = candidate && text.chars().nth(MAX_OVERFLOW_CHARS).is_some();
    let full = (candidate && !oversized).then(|| Arc::<str>::from(text));
    (
        preview,
        Some(PendingText {
            legacy,
            full,
            oversized,
        }),
    )
}

pub(super) fn dedupe_hash(text: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(one_line(text).to_lowercase().as_bytes())
    )
}

pub(super) fn native_dedupe_hash(event: &NativeEvent) -> String {
    event
        .text_identity
        .as_ref()
        .map(|identity| identity.native_dedupe_hash.clone())
        .unwrap_or_else(|| {
            dedupe_hash(
                event
                    .pending_text
                    .as_ref()
                    .map_or(&event.text, |pending| &pending.legacy),
            )
        })
}

/// Call after provider classification and before serializing parsed events.
/// Taking the pending field first guarantees no exit keeps the full response.
pub(super) fn finalize_events(
    project_root: &Path,
    events: &mut [NativeEvent],
    spans: &[PrivacySpan],
) {
    for event in events {
        let Some(pending) = event.pending_text.take() else {
            continue;
        };
        let legacy_room_text = clean_shared_log_text(project_root, &pending.legacy);
        let id = stable_message_id(
            &event.session_id,
            &event.agent_id,
            &event.timestamp,
            &legacy_room_text,
        );
        event.text_identity = Some(TextIdentity {
            message_id: id.clone(),
            native_dedupe_hash: dedupe_hash(&pending.legacy),
            room_dedupe_hash: dedupe_hash(&legacy_room_text),
        });
        if event.role != "assistant"
            || event.kind != "message"
            || is_private_event(event, spans)
            || is_bootstrap_noise(event)
            || is_internal_agent_bus_envelope_event(event)
        {
            continue;
        }
        if pending.oversized {
            eprintln!("Kota Violet overflow skipped: response exceeds file limit ({id})");
            continue;
        }
        let Some(full) = pending.full else { continue };
        // Check full envelopes too: their closing tag may be beyond the preview.
        if is_bootstrap_noise_text(&full)
            || is_agent_bus_envelope_text(&full)
            || is_harness_envelope_text(&full)
        {
            continue;
        }
        let full = clean_shared_log_text(project_root, &full);
        // Count the visible response after the existing secret/ANSI cleanup.
        // Identity above deliberately keeps the old truncate-then-clean order.
        event.text = full.chars().take(MAX_EVENT_TEXT_CHARS).collect();
        if room_event_for(event, project_root).is_none() {
            continue;
        }
        if full.chars().nth(MAX_EVENT_TEXT_CHARS).is_none() {
            continue;
        }
        match persist_new_response(project_root, event, &id, &full) {
            Ok(path) => event.overflow_path = path,
            Err(error) => eprintln!("Kota Violet overflow write failed ({id}): {error}"),
        }
    }
}

fn persist_new_response(
    project_root: &Path,
    event: &NativeEvent,
    id: &str,
    full: &str,
) -> Result<Option<String>, String> {
    // The same lock as history protects check-and-publish across two App instances.
    with_chathistory_write_lock(project_root, || {
        let day_path = chathistory_events_dir(project_root)
            .join(format!("{}.jsonl", chathistory_day_key(&event.timestamp)));
        if let Some(previous) = read_chathistory_event_file(&day_path)?
            .into_iter()
            .find(|old| old.id == id)
        {
            // Forward-only: never create an artifact for a frozen old preview.
            return Ok(previous.overflow_path);
        }
        let relative = format!("project-memory/chathistory/overflow/{id}.md");
        let path = project_root.join(&relative);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_file() && meta.len() > 0 => return Ok(Some(relative)),
            Ok(_) => return Err("overflow destination is not a regular nonempty file".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("stat overflow: {error}")),
        }
        fs::create_dir_all(
            path.parent()
                .expect("generated overflow filename has parent"),
        )
        .map_err(|error| format!("create overflow directory: {error}"))?;
        // Existing atomic temp + rename implementation; no read/modify of old files.
        write_if_changed(&path, full.as_bytes())?;
        Ok(Some(relative))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "kota-overflow-{}-{}",
                std::process::id(),
                WRITE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn event(&self, text: String) -> NativeEvent {
            event(
                &ProjectAgent {
                    agent_id: "agent-fixture-overflow".into(),
                    shell: "codex".into(),
                    cwd: self.0.clone(),
                    session_id: None,
                },
                &NativeSource {
                    kind: "codex-jsonl".into(),
                    session_id: "fixture-overflow-session".into(),
                    path: self.0.join("rollout.jsonl"),
                    aux_path: None,
                },
                "assistant",
                "message",
                "2026-10-09T05:00:00Z",
                "fixture-event",
                text,
            )
        }
        fn finalize(&self, event: &mut NativeEvent) {
            finalize_events(&self.0, std::slice::from_mut(event), &[]);
        }
        fn message(&self, event: NativeEvent) -> VioletChatMessage {
            event_to_message(room_event_for(&event, &self.0).unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn unicode_boundary_stores_only_over_20000_and_keeps_exact_full_response() {
        for length in [19_999, 20_000, 20_001] {
            let f = Fixture::new();
            let text = "🦉".repeat(length);
            let mut event = f.event(text.clone());
            f.finalize(&mut event);
            assert_eq!(event.text.chars().count(), length.min(20_000));
            assert!(event.pending_text.is_none());
            if length > 20_000 {
                let path = f.0.join(event.overflow_path.as_ref().unwrap());
                assert_eq!(fs::read_to_string(path).unwrap(), text);
            } else {
                assert!(event.overflow_path.is_none());
                assert!(!f.0.join("project-memory/chathistory/overflow").exists());
            }
        }
    }

    #[test]
    fn legacy_ids_and_both_dedupe_keys_match_10k_algorithm_after_cleaning() {
        for length in [15_000, 30_000] {
            let f = Fixture::new();
            let text = format!("\u{1b}[31m{}\u{1b}[0m", "文".repeat(length));
            let mut old = f.event(text.clone());
            old.text = truncate_chars(&text, 10_000);
            old.pending_text = None;
            let old_native_key = native_event_dedupe_key(&old);
            let old_message = f.message(old);
            let mut new = f.event(text);
            f.finalize(&mut new);
            assert_eq!(native_event_dedupe_key(&new), old_native_key);
            let new_message = f.message(new);
            assert_eq!(new_message.id, old_message.id);
            assert_eq!(new_message.id, "violet-dfa417dc03d5f52b6");
            assert_eq!(
                room_message_dedupe_key(&new_message),
                room_message_dedupe_key(&old_message)
            );
        }
    }

    #[test]
    fn private_filtered_and_reclassified_events_never_create_artifacts() {
        for (role, kind, prefix, private) in [
            ("assistant", "message", "", true),
            ("assistant", "commentary", "", false),
            ("assistant", "thinking", "", false),
            ("assistant", "tool", "", false),
            ("user", "message", "", false),
            ("assistant", "message", "<permissions instructions>", false),
        ] {
            let f = Fixture::new();
            let mut event = f.event(format!("{prefix}{}FULL_BODY_SENTINEL", "x".repeat(21_000)));
            event.role = role.into();
            event.kind = kind.into(); // Provider can reclassify after event().
            let spans = if private {
                vec![PrivacySpan {
                    agent_id: event.agent_id.clone(),
                    started_at: "2026-10-09T04:59:00Z".into(),
                    ended_at: None,
                }]
            } else {
                vec![]
            };
            finalize_events(&f.0, std::slice::from_mut(&mut event), &spans);
            assert!(event.pending_text.is_none());
            assert!(event.overflow_path.is_none());
            assert!(!f.0.join("project-memory/chathistory/overflow").exists());
            assert!(!serde_json::to_string(&event)
                .unwrap()
                .contains("FULL_BODY_SENTINEL"));
        }
    }

    #[test]
    fn frozen_old_history_does_not_get_backfilled_or_replaced_on_reparse() {
        let f = Fixture::new();
        let full = "x".repeat(30_000);
        let mut old = f.event(full.clone());
        old.text = truncate_chars(&full, 10_000);
        old.pending_text = None;
        let message = f.message(old);
        write_chathistory_messages(&f.0, &[message.clone()]).unwrap();
        let path = chathistory_events_dir(&f.0).join("2026-10-09.jsonl");
        let before = fs::read(&path).unwrap();
        let mut new = f.event(full);
        f.finalize(&mut new);
        assert!(new.overflow_path.is_none());
        assert!(!f.0.join("project-memory/chathistory/overflow").exists());
        write_chathistory_messages(&f.0, &[f.message(new)]).unwrap();
        assert_eq!(fs::read(path).unwrap(), before);
        assert_eq!(
            read_chathistory_event_file(&chathistory_events_dir(&f.0).join("2026-10-09.jsonl"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn repeat_ingestion_reuses_file_and_all_persisted_text_is_bounded() {
        let f = Fixture::new();
        let full = format!("{}FULL_BODY_SENTINEL", "x".repeat(30_000));
        let mut first = f.event(full.clone());
        f.finalize(&mut first);
        let path = f.0.join(first.overflow_path.as_ref().unwrap());
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        write_chathistory_messages(&f.0, &[f.message(first.clone())]).unwrap();
        let mut again = f.event(full.clone());
        f.finalize(&mut again);
        assert_eq!(again.overflow_path, first.overflow_path);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        assert_eq!(fs::read_to_string(path).unwrap(), full);
        let cache_path = f.0.join("cache.jsonl");
        write_cached_native_events(&cache_path, &[again]).unwrap();
        let cache = fs::read_to_string(&cache_path).unwrap();
        assert!(!cache.contains("FULL_BODY_SENTINEL"));
        assert!(!cache.contains("pending_text"));
        let cached = read_cached_native_events(&cache_path).unwrap();
        assert_eq!(cached[0].text.chars().count(), 20_000);
        let message = f.message(cached[0].clone());
        let json = serde_json::to_value(&message).unwrap();
        assert!(json.get("overflowPath").is_some());
        assert!(json.get("legacyDedupeHash").is_none());
        let history = chathistory_event_from_message(&message, None);
        assert_eq!(history.text.chars().count(), 20_000);
        let restored = message_from_chathistory_event(history);
        assert_eq!(restored.overflow_path, message.overflow_path);
        assert_eq!(
            room_message_dedupe_key(&restored),
            room_message_dedupe_key(&message)
        );
    }

    #[test]
    fn write_failure_keeps_preview_and_omits_link_without_holding_full_text() {
        let f = Fixture::new();
        ensure_chathistory_dirs(&f.0).unwrap();
        let blocked = chathistory_dir(&f.0).join("overflow");
        fs::write(&blocked, b"keep-existing-file").unwrap();
        let mut event = f.event("x".repeat(30_000));
        f.finalize(&mut event);
        assert_eq!(event.text.chars().count(), 20_000);
        assert!(event.overflow_path.is_none());
        assert!(event.pending_text.is_none());
        assert_eq!(fs::read(blocked).unwrap(), b"keep-existing-file");
    }

    #[test]
    fn full_text_is_not_in_debug_and_is_not_serialized_even_before_finalize() {
        let f = Fixture::new();
        let event = f.event(format!("{}UNIQUE_FULL_BODY", "x".repeat(21_000)));
        assert!(!format!("{event:?}").contains("UNIQUE_FULL_BODY"));
        assert!(!serde_json::to_string(&event)
            .unwrap()
            .contains("UNIQUE_FULL_BODY"));
    }

    #[test]
    fn normalization_never_discards_visible_text_or_writes_hidden_envelopes() {
        let f = Fixture::new();
        let text = format!("{}visible tail", "\u{1b}[31m\u{1b}[0m".repeat(3_000));
        let mut event = f.event(text);
        f.finalize(&mut event);
        assert_eq!(event.text, "visible tail");
        assert!(event.overflow_path.is_none());
        let mut envelope = f.event(format!(
            "<system-reminder>{}</system-reminder>",
            "x".repeat(21_000)
        ));
        f.finalize(&mut envelope);
        assert!(envelope.overflow_path.is_none());
        assert!(envelope.pending_text.is_none());
        assert!(!f.0.join("project-memory/chathistory/overflow").exists());
    }

    #[test]
    fn oversized_response_never_claims_a_truncated_file_is_complete() {
        let f = Fixture::new();
        let mut event = f.event("x".repeat(MAX_OVERFLOW_CHARS + 1));
        f.finalize(&mut event);
        assert_eq!(event.text.chars().count(), MAX_EVENT_TEXT_CHARS);
        assert!(event.overflow_path.is_none());
        assert!(event.pending_text.is_none());
        assert!(!f.0.join("project-memory/chathistory/overflow").exists());
    }

    #[test]
    fn split_handoff_keeps_its_own_identity_when_the_prompt_is_long() {
        let f = Fixture::new();
        let mut prompt = f.event("x".repeat(21_000));
        prompt.role = "user".into();
        prompt.shell_handoff = Some(crate::shell_switch::ProjectedHandoff {
            id: "fixture-handoff".into(),
            text: "Shell changed.".into(),
        });
        f.finalize(&mut prompt);
        let expanded = expand_shell_handoffs(vec![prompt]);
        let whisper = &expanded[0];
        assert!(whisper.text_identity.is_none());
        assert!(whisper.pending_text.is_none());
        assert!(whisper.overflow_path.is_none());
        let mut expected = f.event("Shell changed.".into());
        expected.role = "system".into();
        assert_eq!(
            room_message_dedupe_key(&f.message(whisper.clone())),
            room_message_dedupe_key(&f.message(expected))
        );
        assert!(expanded[1].text_identity.is_some());
    }

    #[test]
    fn cold_tail_limit_is_explicit_but_incremental_large_endturn_is_complete() {
        let full = "x".repeat(JSONL_TAIL_BYTES as usize + 1_000);
        let line = serde_json::json!({"type":"event_msg","timestamp":"2026-10-09T05:00:00Z","payload":{"type":"agent_message","message":full}});
        for warm in [false, true] {
            let f = Fixture::new();
            let agent = ProjectAgent {
                agent_id: "agent-fixture-overflow".into(),
                shell: "codex".into(),
                cwd: f.0.clone(),
                session_id: None,
            };
            let source = NativeSource {
                kind: "codex-jsonl".into(),
                session_id: "fixture-overflow-session".into(),
                path: f.0.join("rollout.jsonl"),
                aux_path: None,
            };
            fs::write(&source.path, "{}\n").unwrap();
            if warm {
                assert!(
                    parse_jsonl_source_incremental(&f.0, &agent, &source, parse_codex_line)
                        .unwrap()
                        .is_empty()
                );
            }
            append_text(&source.path, &format!("{line}\n")).unwrap();
            let parsed =
                parse_jsonl_source_incremental(&f.0, &agent, &source, parse_codex_line).unwrap();
            if warm {
                assert_eq!(parsed.len(), 1);
                assert_eq!(
                    fs::read_to_string(f.0.join(parsed[0].overflow_path.as_ref().unwrap()))
                        .unwrap(),
                    full
                );
                assert_eq!(parsed[0].text.chars().count(), MAX_EVENT_TEXT_CHARS);
            } else {
                // Existing default 2 MiB cold-tail reader skips a partial first line.
                assert!(parsed.is_empty());
                assert!(!f.0.join("project-memory/chathistory/overflow").exists());
            }
        }
    }

    #[test]
    fn incremental_parser_finalizes_before_cache_and_leaves_old_cache_untouched() {
        let f = Fixture::new();
        let agent = ProjectAgent {
            agent_id: "agent-fixture-overflow".into(),
            shell: "codex".into(),
            cwd: f.0.clone(),
            session_id: None,
        };
        let source = NativeSource {
            kind: "codex-jsonl".into(),
            session_id: "fixture-overflow-session".into(),
            path: f.0.join("rollout.jsonl"),
            aux_path: None,
        };
        let full = format!("{}FULL_BODY_SENTINEL", "x".repeat(21_000));
        let line = serde_json::json!({"type":"event_msg","timestamp":"2026-10-09T05:00:00Z","payload":{"type":"agent_message","message":full}});
        fs::write(&source.path, format!("{line}\n")).unwrap();
        let parsed =
            parse_jsonl_source_incremental(&f.0, &agent, &source, parse_codex_line).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].overflow_path.is_some());
        assert!(parsed[0].pending_text.is_none());
        let path = parsed_event_cache_path(&f.0, &source_cache_key(&source)).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&before).contains("FULL_BODY_SENTINEL"));
        assert_eq!(parsed[0].text.chars().count(), 20_000);
        let repeated =
            parse_jsonl_source_incremental(&f.0, &agent, &source, parse_codex_line).unwrap();
        assert_eq!(repeated.len(), 1);
        assert_eq!(fs::read(path).unwrap(), before);
        assert_eq!(PARSED_EVENT_CACHE_VERSION, "8");
    }
}
