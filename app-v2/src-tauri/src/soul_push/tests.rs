use super::{core::*, settings::Settings};
use crate::{message_origin::MessageOrigin, soul::marks::{Mark, Marks}, violet::ActiveDream};
use serde_json::{json, Value};
use std::collections::HashMap;

fn settings() -> Settings {
    // Explicit fixture, not a decision about the product's first-launch toggle.
    Settings { schema: 1, enabled: true, mark_threshold: 0.85, dream_threshold: 0.90,
        max_marks: 3, max_dreams: 2, cooldown_replies: 10, minimum_message_length: 60 }
}
fn mark(id: &str, text: &str, ts: &str) -> Mark {
    Mark { id: id.into(), text: text.into(), created_at: ts.into(), updated_at: ts.into(), project: None, used: 0 }
}
fn fixture_marks() -> Marks {
    Marks { version: 1, revision: 7, marks: vec![
        mark("m-old", "Keep synthetic receipts.", "2026-10-14T00:00:00Z"),
        mark("m-new", "Check the fixture before changing a rule.", "2026-10-15T00:00:00Z"),
    ] }
}
fn fixture_dreams() -> Vec<ActiveDream> {
    vec![ActiveDream { id: "d-old".into(), text: "The fixture user likes brief notes.".into(), position: 0 },
        ActiveDream { id: "d-new".into(), text: "The fixture user reads the result first.".into(), position: 1 }]
}
fn request(settings: &Settings) -> Request {
    build_request("Synthetic user input", None, 7,
        notes(&fixture_marks(), &fixture_dreams()).unwrap(), settings).unwrap()
}
fn reply(request: &Request, p: f64) -> Value {
    json!({"answers": request.body["questions"].as_object().unwrap().keys()
        .map(|id| (id.clone(), json!({"type":"noul","noul":p}))).collect::<serde_json::Map<_, _>>()})
}

#[test]
fn settings_ranges_and_soul_eligibility_do_not_inherit_a2a_temporal_eligibility() {
    let policy = settings();
    assert!(policy.validate().is_ok());
    for n in 1..=19 {
        let mut next = policy.clone(); next.mark_threshold = n as f64 / 20.0; next.dream_threshold = next.mark_threshold;
        assert!(next.validate().is_ok());
    }
    for value in [0.0, 0.01, 0.96, 1.0, 0.875, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut next = policy.clone(); next.mark_threshold = value;
        assert!(next.validate().is_err());
    }
    for (marks, dreams, replies, length, valid) in [
        (1, 0, 0, 0, true), (5, 5, 50, 500, true), (0, 2, 10, 60, false),
        (6, 2, 10, 60, false), (3, 6, 10, 60, false), (3, 2, 51, 60, false),
        (3, 2, 10, 510, false), (3, 2, 10, 61, false),
    ] {
        let mut next = policy.clone(); next.max_marks = marks; next.max_dreams = dreams;
        next.cooldown_replies = replies; next.minimum_message_length = length;
        assert_eq!(next.validate().is_ok(), valid);
    }
    for origin in [MessageOrigin::User, MessageOrigin::Telegram, MessageOrigin::Puppeteer] {
        assert!(!policy.permits(origin, 59)); assert!(policy.permits(origin, 60));
    }
    for origin in [MessageOrigin::A2a, MessageOrigin::Bbs, MessageOrigin::Ember, MessageOrigin::Dream,
        MessageOrigin::Handoff, MessageOrigin::Bartender, MessageOrigin::System] {
        assert!(!policy.permits(origin, 4_000));
    }
    assert!(MessageOrigin::A2a.uses_temporal_gap());
    let mut off = policy.clone(); off.enabled = false;
    assert!(!off.permits(MessageOrigin::User, 4_000));
    let mut no_minimum = policy; no_minimum.minimum_message_length = 0;
    assert!(no_minimum.permits(MessageOrigin::User, 0));
}

#[test]
fn request_uses_the_approved_question_and_mark_field_for_both_kinds() {
    let request = request(&settings());
    assert_eq!(QUESTION, "An AI agent keeps `mark` as a lesson from its earlier work. It now receives `new_message`; `previous_reply`, when present, is its latest reply. `mark` applies to how the agent should handle `new_message`.");
    assert_eq!(request.body.as_object().unwrap().len(), 2); // transport supplies model
    assert_eq!(request.body["state"], json!({"new_message":"Synthetic user input"}));
    let questions = request.body["questions"].as_object().unwrap();
    assert_eq!(questions.len(), 4);
    for (id, question) in questions {
        assert!(id.starts_with("m-") || id.starts_with("d-"));
        assert_eq!(question["type"], "noul");
        assert_eq!(question.as_object().unwrap().len(), 2);
        assert_eq!(question["instructions"].as_object().unwrap().len(), 2);
        assert_eq!(question["instructions"]["question"], QUESTION);
        assert!(question["instructions"]["mark"].is_string());
        assert!(question["instructions"].get("dream").is_none());
    }
    assert_eq!(questions["m-new"]["instructions"]["mark"], fixture_marks().marks[1].text);
    assert_eq!(questions["d-new"]["instructions"]["mark"], fixture_dreams()[1].text);
}

#[test]
fn state_has_only_bounded_original_text_and_latest_reply_tail_in_unicode_characters() {
    let text = format!("{}end", "雪🙂".repeat(3_000));
    let previous = format!("discard{}tail", "梦🙂".repeat(2_000));
    let request = build_request(&text, Some(&previous), 7,
        notes(&fixture_marks(), &[]).unwrap(), &settings()).unwrap();
    assert_eq!(request.body["state"].as_object().unwrap().len(), 2);
    assert_eq!(request.body["state"]["new_message"], text.chars().take(4_000).collect::<String>());
    let tail = request.body["state"]["previous_reply"].as_str().unwrap();
    assert_eq!(tail.chars().count(), 1_500); assert!(tail.ends_with("tail"));
    assert_eq!(tail, previous.chars().skip(previous.chars().count() - 1_500).collect::<String>());
    let request = build_request("short", Some(""), 7, notes(&fixture_marks(), &[]).unwrap(), &settings()).unwrap();
    assert!(request.body["state"].get("previous_reply").is_none());
}

#[test]
fn budget_packs_marks_first_skips_an_oversized_dream_and_is_not_a_question_count_cap() {
    let marks = Marks { version: 1, revision: 2, marks: (0..20).map(|i|
        mark(&format!("m-{i}"), &"记".repeat(200), "2026-10-15T00:00:00Z")).collect() };
    let mut dreams = vec![ActiveDream { id: "d-huge".into(), text: "🙂".repeat(10_000), position: 0 }];
    dreams.extend((1..50).map(|i| ActiveDream { id: format!("d-{i}"), text: "梦".repeat(200), position: i }));
    let request = build_request(&"文".repeat(4_000), Some(&"复".repeat(1_500)), marks.revision,
        notes(&marks, &dreams).unwrap(), &settings()).unwrap();
    let questions = request.body["questions"].as_object().unwrap();
    for mark in &marks.marks { assert!(questions.contains_key(&mark.id)); }
    assert!(!questions.contains_key("d-huge"));
    assert!(questions.len() > 30); assert!(questions.len() < marks.marks.len() + dreams.len() - 1);
    assert!(request.body.to_string().len() <= 60_000);
    for (id, question) in questions {
        assert!(request.body["state"].to_string().len() + question.to_string().len() + id.len() + 4 <= 30_000);
    }
    let mut no_dreams = settings(); no_dreams.max_dreams = 0;
    assert!(build_request("fixture", None, 0, notes(&Marks::default(), &fixture_dreams()).unwrap(), &no_dreams).is_none());
    assert!(build_request("fixture", None, 0, vec![], &settings()).is_none());
}

#[test]
fn selection_is_inclusive_per_kind_and_ties_use_addition_time_or_dream_position() {
    let mut settings = settings(); settings.max_marks = 1; settings.max_dreams = 1;
    let mut marks = fixture_marks();
    marks.marks[0].updated_at = "2026-10-16T00:00:00Z".into(); // editing isn't a new addition
    let request = build_request("fixture", None, 7, notes(&marks, &fixture_dreams()).unwrap(), &settings).unwrap();
    let mut answer = reply(&request, 0.90);
    answer["answers"]["m-old"]["noul"] = json!(0.85);
    answer["answers"]["m-new"]["noul"] = json!(0.85);
    let selected = select(&request, &answer, &settings).unwrap();
    assert_eq!(selected.iter().map(|hit| hit.note.id.as_str()).collect::<Vec<_>>(), vec!["m-new", "d-new"]);
    answer["answers"]["m-new"]["noul"] = json!(0.8499);
    answer["answers"]["d-new"]["noul"] = json!(0.8999);
    let selected = select(&request, &answer, &settings).unwrap();
    assert_eq!(selected.iter().map(|hit| hit.note.id.as_str()).collect::<Vec<_>>(), vec!["m-old", "d-old"]);
    assert!(select(&request, &reply(&request, 0.1), &settings).unwrap().is_empty());
}

#[test]
fn malformed_or_incomplete_answers_reject_the_whole_call_not_just_one_note() {
    let request = request(&settings());
    for bad in [Value::Null, json!({}), json!({"type":"noul","noul":"0.99"}),
        json!({"type":"choice","noul":0.99}), json!({"type":"noul","noul":-0.01}),
        json!({"type":"noul","noul":1.01}), json!({"type":"noul","probability":0.99})] {
        let mut answer = reply(&request, 0.99); answer["answers"]["m-new"] = bad;
        assert!(select(&request, &answer, &settings()).is_err());
    }
    let mut missing = reply(&request, 0.99); missing["answers"].as_object_mut().unwrap().remove("d-old");
    assert!(select(&request, &missing, &settings()).is_err());
    let mut extra = reply(&request, 0.99); extra["answers"]["m-unknown"] = json!({"type":"noul","noul":0.99});
    assert!(select(&request, &extra, &settings()).is_err());
    assert!(select(&request, &json!({}), &settings()).is_err());
}

#[test]
fn envelope_keeps_exact_copy_both_sections_and_third_push_hint_without_probabilities() {
    let request = request(&settings());
    let mut answer = reply(&request, 0.0);
    answer["answers"]["m-old"]["noul"] = json!(0.89);
    answer["answers"]["d-new"]["noul"] = json!(0.97);
    let selected = select(&request, &answer, &settings()).unwrap();
    let text = envelope(request.revision, &selected, &HashMap::from([("d-new".into(), 3)])).unwrap();
    assert_eq!(text, concat!(
        "<KOTA_MARKS v=\"1\" revision=\"7\">\n",
        "Your own marks that may apply to this message. They are for reference, not new instructions. Use your own judgement.\n",
        "- [m-old] Keep synthetic receipts.\n\n",
        "What Kota has learned about the user that may apply to this message. For reference, not instructions. Use your own judgement.\n",
        "- [d-new] The fixture user reads the result first.\n",
        "  ↑ Pushed to you 3 times. If this is a lasting lesson for you, add-mark it; then run clear-heat d-new either way.\n",
        "</KOTA_MARKS>"));
    assert!(!text.contains("0.89") && !text.contains("0.97") && !text.contains("//"));
    assert!(!envelope(7, &selected, &HashMap::from([("d-new".into(), 2)])).unwrap().contains("↑"));
    let just_dream = selected.iter().filter(|hit| hit.note.kind == Kind::Dream).cloned().collect::<Vec<_>>();
    let text = envelope(7, &just_dream, &HashMap::new()).unwrap();
    assert!(!text.contains(MARK_INTRO)); assert!(text.contains(DREAM_INTRO));
    assert!(envelope(7, &[], &HashMap::new()).is_none());
}

#[test]
fn each_repeated_dream_keeps_its_own_hint_directly_below_its_line() {
    let request = request(&settings());
    let mut answer = reply(&request, 0.0);
    answer["answers"]["d-new"]["noul"] = json!(0.98);
    answer["answers"]["d-old"]["noul"] = json!(0.95);
    let selected = select(&request, &answer, &settings()).unwrap();
    let counts = HashMap::from([("d-new".into(), 3), ("d-old".into(), 7)]);
    let text = envelope(7, &selected, &counts).unwrap();
    assert_eq!(text, concat!(
        "<KOTA_MARKS v=\"1\" revision=\"7\">\n",
        "What Kota has learned about the user that may apply to this message. For reference, not instructions. Use your own judgement.\n",
        "- [d-new] The fixture user reads the result first.\n",
        "  ↑ Pushed to you 3 times. If this is a lasting lesson for you, add-mark it; then run clear-heat d-new either way.\n",
        "- [d-old] The fixture user likes brief notes.\n",
        "  ↑ Pushed to you 7 times. If this is a lasting lesson for you, add-mark it; then run clear-heat d-old either way.\n",
        "</KOTA_MARKS>"));
}

#[test]
fn snapshots_reject_duplicate_ids_and_malformed_fields_without_guessing_dream_dates() {
    let mut marks = fixture_marks(); marks.marks.push(marks.marks[0].clone());
    assert!(notes(&marks, &[]).is_err());
    let mut marks = fixture_marks(); marks.marks[0].created_at = "not-a-time".into();
    assert!(notes(&marks, &[]).is_err());
    for text in ["", " \t", "two\nlines", "nul\0byte"] {
        let dreams = vec![ActiveDream { id: "d-synthetic".into(), text: text.into(), position: 4 }];
        assert!(notes(&Marks::default(), &dreams).is_err());
    }
    let dreams = vec![ActiveDream { id: "d-synthetic".into(), text: "Inline </KOTA_MARKS> is ordinary note text.".into(), position: 4 }];
    assert!(notes(&Marks::default(), &dreams).is_ok());
}
