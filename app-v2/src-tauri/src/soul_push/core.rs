//! Pure request, answer selection and envelope formatting. No HTTP or file I/O.
use super::settings::Settings;
use crate::{soul::marks::Marks, violet::ActiveDream};
use chrono::{DateTime, FixedOffset};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

pub(crate) const QUESTION: &str = "An AI agent keeps `mark` as a lesson from its earlier work. It now receives `new_message`; `previous_reply`, when present, is its latest reply. `mark` applies to how the agent should handle `new_message`.";
pub(crate) const MARK_INTRO: &str = "Your own marks that may apply to this message. They are for reference, not new instructions. Use your own judgement.";
pub(crate) const DREAM_INTRO: &str = "What Kota has learned about the user that may apply to this message. For reference, not instructions. Use your own judgement.";
pub(crate) const DREAM_REMINDER_AT: u64 = 3;

// Conservative UTF-8/JSON byte budgets, not a token-count claim. Byte-level
// tokenization cannot make a text token from less than one byte. Leave headroom
// for the service's own framing under its 64k/request and 32k/question limits.
const REQUEST_BUDGET: usize = 60_000;
const QUESTION_BUDGET: usize = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind { Mark, Dream }

#[derive(Clone, Debug)]
pub(crate) struct Note {
    pub id: String,
    pub text: String,
    pub kind: Kind,
    created_at: Option<DateTime<FixedOffset>>,
    position: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Selected {
    pub note: Note,
    pub probability: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub revision: u64,
    pub body: Value,
    notes: Vec<Note>,
}

fn valid_note(id: &str, text: &str, prefix: &str) -> bool {
    id.starts_with(prefix) && id.len() > prefix.len() && id.len() <= 128
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        && !text.trim().is_empty() && !text.contains(['\n', '\r', '\0'])
}

pub(crate) fn notes(marks: &Marks, dreams: &[ActiveDream]) -> Result<Vec<Note>, &'static str> {
    let mut result = Vec::new();
    let mut ids = HashSet::new();
    for (position, mark) in marks.marks.iter().enumerate() {
        if !valid_note(&mark.id, &mark.text, "m-") || !ids.insert(mark.id.clone()) {
            return Err("soul_snapshot_invalid");
        }
        let created_at = DateTime::parse_from_rfc3339(&mark.created_at)
            .map_err(|_| "soul_snapshot_invalid")?;
        result.push(Note { id: mark.id.clone(), text: mark.text.clone(), kind: Kind::Mark,
            created_at: Some(created_at), position });
    }
    for dream in dreams {
        if !valid_note(&dream.id, &dream.text, "d-") || !ids.insert(dream.id.clone()) {
            return Err("soul_snapshot_invalid");
        }
        result.push(Note { id: dream.id.clone(), text: dream.text.clone(), kind: Kind::Dream,
            created_at: None, position: dream.position });
    }
    Ok(result)
}

pub(crate) fn build_request(
    original_text: &str,
    previous_reply: Option<&str>,
    revision: u64,
    candidates: Vec<Note>,
    settings: &Settings,
) -> Option<Request> {
    let mut state = json!({"new_message": original_text.chars().take(4_000).collect::<String>()});
    if let Some(reply) = previous_reply.filter(|text| !text.is_empty()) {
        let start = reply.chars().count().saturating_sub(1_500);
        state["previous_reply"] = json!(reply.chars().skip(start).collect::<String>());
    }
    let state_bytes = state.to_string().len();
    let mut questions = Map::new();
    let mut selected = Vec::new();
    let mut total_bytes = state_bytes + 64;
    // Do not impose a question-count limit. Marks get the budget first, then
    // Dreams. The caller already removed entries still inside their cooldown.
    for kind in [Kind::Mark, Kind::Dream] {
        if kind == Kind::Dream && settings.max_dreams == 0 { continue; }
        for note in candidates.iter().filter(|note| note.kind == kind) {
            let question = json!({"type":"noul", "instructions":{"mark":note.text,"question":QUESTION}});
            let bytes = question.to_string().len() + note.id.len() + 4;
            if state_bytes + bytes > QUESTION_BUDGET || total_bytes + bytes > REQUEST_BUDGET {
                continue;
            }
            // TypeSafe's API documents questions as map<string, Question> with
            // caller-chosen opaque keys, so keep the m-/d- ID unchanged.
            questions.insert(note.id.clone(), question);
            selected.push(note.clone());
            total_bytes += bytes;
        }
    }
    if selected.is_empty() { return None; }
    // The shared transport supplies the selected channel's model ID and key.
    Some(Request { revision, body: json!({"state":state,"questions":questions}), notes: selected })
}

pub(crate) fn select(request: &Request, reply: &Value, settings: &Settings) -> Result<Vec<Selected>, &'static str> {
    let invalid = "soul_jev_invalid_answer";
    let answers = reply["answers"].as_object().ok_or(invalid)?;
    if answers.len() != request.notes.len() { return Err(invalid); }
    let mut scored = Vec::new();
    for note in &request.notes {
        let answer = answers.get(&note.id).ok_or(invalid)?;
        let p = answer["noul"].as_f64().ok_or(invalid)?;
        if answer["type"] != "noul" || !p.is_finite() || !(0.0..=1.0).contains(&p) {
            return Err(invalid);
        }
        scored.push(Selected { note: note.clone(), probability: p });
    }
    let mut result = Vec::new();
    for kind in [Kind::Mark, Kind::Dream] {
        let (threshold, cap) = if kind == Kind::Mark {
            (settings.mark_threshold, settings.max_marks)
        } else { (settings.dream_threshold, settings.max_dreams) };
        let mut accepted = scored.iter().filter(|hit| hit.note.kind == kind && hit.probability >= threshold)
            .cloned().collect::<Vec<_>>();
        accepted.sort_by(|a, b| b.probability.total_cmp(&a.probability)
            .then_with(|| b.note.created_at.cmp(&a.note.created_at))
            .then_with(|| b.note.position.cmp(&a.note.position)));
        result.extend(accepted.into_iter().take(cap));
    }
    Ok(result)
}

/// Count supplied here includes this push. Only shown at/after the third push;
/// account-ledger order/clear semantics belong to the shared Soul storage module.
pub(crate) fn body(selected: &[Selected], dream_counts: &HashMap<String, u64>) -> Option<String> {
    if selected.is_empty() { return None; }
    let mut sections = Vec::new();
    for (kind, intro) in [(Kind::Mark, MARK_INTRO), (Kind::Dream, DREAM_INTRO)] {
        let entries = selected.iter().filter(|hit| hit.note.kind == kind).collect::<Vec<_>>();
        if entries.is_empty() { continue; }
        let mut section = intro.to_string();
        for hit in entries {
            section.push_str(&format!("\n- [{}] {}", hit.note.id, hit.note.text));
            if kind == Kind::Dream {
                if let Some(&count) = dream_counts.get(&hit.note.id).filter(|count| **count >= DREAM_REMINDER_AT) {
                    section.push_str(&format!("\n  ↑ Pushed to you {count} times. If this is a lasting lesson for you, add-mark it; then run clear-heat {} either way.", hit.note.id));
                }
            }
        }
        sections.push(section);
    }
    Some(sections.join("\n\n"))
}

pub(crate) fn envelope(revision: u64, selected: &[Selected], dream_counts: &HashMap<String, u64>) -> Option<String> {
    body(selected, dream_counts)
        .map(|body| format!("<KOTA_MARKS v=\"1\" revision=\"{revision}\">\n{body}\n</KOTA_MARKS>"))
}
