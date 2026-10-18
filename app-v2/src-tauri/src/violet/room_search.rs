//! Read-only search and context windows over the existing daily room segments.

use super::*;

const SEARCH_LIMIT_DEFAULT: usize = 50;
const TOTAL_BUDGET: StdDuration = StdDuration::from_millis(300);

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletRoomSearchRequest {
    pub project_root: String,
    pub query: String,
    #[serde(default)]
    pub human_only: bool,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletRoomSearchResult {
    pub hits: Vec<VioletChatMessage>,
    pub match_terms: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VioletRoomAround {
    pub id: String,
    pub before: usize,
    pub after: usize,
}

struct SearchTerms {
    display: Vec<String>,
    lower: Vec<String>,
    raw: Vec<memchr::memmem::Finder<'static>>,
    ascii_letters: Vec<bool>,
    slow_query: bool,
}

impl SearchTerms {
    fn parse(query: &str) -> Self {
        let mut terms = Vec::new();
        let mut term = String::new();
        let mut quoted = false;
        for ch in query.chars() {
            if ch == '"' || (!quoted && ch.is_whitespace()) {
                if !term.trim().is_empty() {
                    terms.push(term.trim().to_owned());
                }
                term.clear();
                if ch == '"' {
                    quoted = !quoted;
                }
            } else {
                term.push(ch);
            }
        }
        // An unfinished quote is still a literal phrase, never a parse error.
        if !term.trim().is_empty() {
            terms.push(term.trim().to_owned());
        }
        let mut seen = HashSet::new();
        terms.retain(|term| seen.insert(term.to_lowercase()));
        let lower: Vec<String> = terms.iter().map(|term| term.to_lowercase()).collect();
        let raw = lower
            .iter()
            .map(|term| memchr::memmem::Finder::new(term).into_owned())
            .collect();
        let ascii_letters = lower
            .iter()
            .map(|term| term.bytes().any(|ch| ch.is_ascii_alphabetic()))
            .collect();
        let slow_query = terms.iter().any(|term| {
            term.chars().any(|ch| {
                matches!(ch, '"' | '\\')
                    || ch.is_control()
                    || (!ch.is_ascii()
                        && ch.to_lowercase().to_string() != ch.to_uppercase().to_string())
            })
        });
        Self {
            display: terms,
            lower,
            raw,
            ascii_letters,
            slow_query,
        }
    }

    fn candidate(&self, line: &[u8]) -> bool {
        // Ordinary CJK/ASCII queries need no allocated lowercase copy of every
        // JSON line. Unusual encodings/case mappings conservatively reach serde.
        if self.slow_query || has_alternate_json_escapes(line) || has_ascii_case_expansion(line) {
            return true;
        }
        self.raw.iter().enumerate().all(|(index, finder)| {
            if self.ascii_letters[index] {
                ascii_contains(line, self.lower[index].as_bytes())
            } else {
                finder.find(line).is_some()
            }
        })
    }

    fn matches(&self, text: &str) -> bool {
        let lower = text.to_lowercase();
        self.lower.iter().all(|term| lower.contains(term))
    }
}

fn json_fragment(text: &str) -> String {
    let encoded = serde_json::to_string(text).expect("serialize a string");
    encoded[1..encoded.len() - 1].to_owned()
}

fn has_alternate_json_escapes(line: &[u8]) -> bool {
    memchr::memmem::find(line, b"\\u").is_some() || memchr::memmem::find(line, b"\\/").is_some()
}

// These are the non-ASCII scalars whose Rust Unicode lowercase mapping can
// introduce ASCII. The exhaustive test guards this assumption across upgrades.
const ASCII_CASE_EXPANSIONS: &[char] = &['\u{212a}', '\u{130}'];

fn has_ascii_case_expansion(line: &[u8]) -> bool {
    ASCII_CASE_EXPANSIONS.iter().any(|ch| {
        let mut bytes = [0; 4];
        memchr::memmem::find(line, ch.encode_utf8(&mut bytes).as_bytes()).is_some()
    })
}

fn ascii_contains(line: &[u8], term: &[u8]) -> bool {
    memchr::memchr2_iter(
        term[0].to_ascii_lowercase(),
        term[0].to_ascii_uppercase(),
        line,
    )
    .any(|offset| {
        line.get(offset..offset + term.len())
            .is_some_and(|slice| slice.eq_ignore_ascii_case(term))
    })
}

fn segment_lines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut start = 0;
    let mut lines = Vec::new();
    for end in memchr::memchr_iter(b'\n', bytes) {
        lines.push(&bytes[start..end]);
        start = end + 1;
    }
    if start < bytes.len() {
        lines.push(&bytes[start..]);
    }
    lines
}

struct Segment {
    day: String,
    path: PathBuf,
}

fn valid_day(day: &str) -> bool {
    day.len() == 10
        && chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
            .is_ok_and(|date| date.format("%Y-%m-%d").to_string() == day)
}

fn segments(project_root: &Path) -> Result<Vec<Segment>, String> {
    let dir = chathistory_events_dir(project_root);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("read {}: {err}", dir.display())),
    };
    let mut segments = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("read {}: {err}", dir.display()))?;
        let name = entry.file_name();
        let Some(day) = name.to_str().and_then(|name| name.strip_suffix(".jsonl")) else {
            continue;
        };
        if valid_day(day) && entry.path().is_file() {
            segments.push(Segment {
                day: day.to_owned(),
                path: entry.path(),
            });
        }
    }
    segments.sort_by(|a, b| b.day.cmp(&a.day));
    Ok(segments)
}

fn read_segment(segment: &Segment) -> Result<Vec<u8>, String> {
    fs::read(&segment.path).map_err(|err| format!("read {}: {err}", segment.path.display()))
}

struct SearchCursor {
    day: String,
    line: usize,
}

impl SearchCursor {
    fn parse(cursor: &str) -> Result<Self, String> {
        let invalid = || "Invalid room search cursor".to_owned();
        let (day, line) = cursor.split_once(':').ok_or_else(invalid)?;
        if !valid_day(day) || line.is_empty() || !line.bytes().all(|ch| ch.is_ascii_digit()) {
            return Err(invalid());
        }
        let line = line.parse::<usize>().map_err(|_| invalid())?;
        if line == 0 {
            return Err(invalid());
        }
        Ok(Self {
            day: day.to_owned(),
            line,
        })
    }
}

pub fn search_room(
    project_root: &Path,
    request: VioletRoomSearchRequest,
) -> Result<VioletRoomSearchResult, String> {
    search_with_budget(project_root, request, TOTAL_BUDGET)
}

fn search_with_budget(
    project_root: &Path,
    request: VioletRoomSearchRequest,
    budget: StdDuration,
) -> Result<VioletRoomSearchResult, String> {
    let started = Instant::now();
    let terms = SearchTerms::parse(&request.query);
    let cursor = request
        .cursor
        .as_deref()
        .map(SearchCursor::parse)
        .transpose()?;
    let first_page = cursor.is_none();
    let limit = request.limit.unwrap_or(SEARCH_LIMIT_DEFAULT).max(1);
    let mut result = VioletRoomSearchResult {
        hits: Vec::new(),
        match_terms: terms.display.clone(),
        total: first_page.then_some(0),
        next_cursor: None,
        truncated: false,
    };
    if terms.lower.is_empty() {
        return Ok(result);
    }
    let mut total = 0;
    let mut hit_positions = HashMap::new();
    // A daily segment is already chronologically sorted by the room writer.
    // Reverse both dates and physical lines, keeping same-timestamp ties stable.
    for segment in segments(project_root)? {
        if cursor
            .as_ref()
            .is_some_and(|cursor| segment.day > cursor.day)
        {
            continue;
        }
        let bytes = read_segment(&segment)?;
        let lines = segment_lines(&bytes);
        for (index, line) in lines.iter().enumerate().rev() {
            if cursor
                .as_ref()
                .is_some_and(|cursor| segment.day == cursor.day && index + 1 >= cursor.line)
            {
                continue;
            }
            // The budget only limits counting, never the accuracy/fullness of hits.
            if first_page && result.hits.len() == limit && started.elapsed() >= budget {
                result.total = None;
                result.truncated = true;
                return Ok(finish_search_page(result, &hit_positions));
            }
            if !terms.candidate(line) {
                continue;
            }
            let Ok(event) = serde_json::from_slice::<ChathistoryEvent>(line) else {
                continue;
            };
            if !searchable(&event, request.human_only) || !terms.matches(&event.text) {
                continue;
            }
            total += 1;
            if result.hits.len() < limit {
                // Counting may continue, but pagination always resumes at the last hit returned.
                let position = format!("{}:{}", segment.day, index + 1);
                hit_positions.insert(event.id.clone(), (result.hits.len(), position.clone()));
                result.next_cursor = Some(position);
                result.hits.push(message_from_chathistory_event(event));
            }
            if !first_page && result.hits.len() == limit {
                return Ok(finish_search_page(result, &hit_positions));
            }
        }
    }
    if first_page {
        result.total = Some(total);
    }
    if total <= result.hits.len() {
        result.next_cursor = None;
    }
    Ok(finish_search_page(result, &hit_positions))
}

fn finish_search_page(
    mut result: VioletRoomSearchResult,
    positions: &HashMap<String, (usize, String)>,
) -> VioletRoomSearchResult {
    // Match the room's visibility within this page; total still counts raw events.
    // Keep the segment writer's descending order, including mixed timestamp
    // precision ties. Only returned IDs own a cursor; no state survives the page.
    result.hits = dedupe_room_messages(result.hits);
    result.hits.sort_by_key(|message| positions[&message.id].0);
    if result.next_cursor.is_some() {
        result.next_cursor = result
            .hits
            .last()
            .map(|message| positions[&message.id].1.clone());
    }
    result
}

fn searchable(event: &ChathistoryEvent, human_only: bool) -> bool {
    if !event.display
        || event.kind != "message"
        || event.role == "system"
        || event.message_origin.as_deref() == Some("shell_handoff")
        || event.agent_id == "violet"
        || event.agent_id == "bartender"
        || is_project_agent_lifecycle_notification(event)
        || is_ignorable_codex_internal_context_event(event)
    {
        return false;
    }
    let intent = event
        .actor_intent
        .as_deref()
        .filter(|value| !value.is_empty());
    if matches!(intent, Some("delivery-skipped" | "bbs-thread" | "dream"))
        || (event.agent_id == "laughing-man" && intent != Some("telegram"))
        || (event.agent_id == "ember" && intent != Some("reminder"))
        || event
            .source
            .native_event_id
            .as_deref()
            .is_some_and(|id| id.ends_with(":skipped"))
    {
        return false;
    }
    let text = strip_leading_provider_attachment_markers(&event.text);
    if is_agent_bus_envelope_text(text)
        || (event.role == "user" && event.agent_id != "user"
            && crate::temporal_context::is_temporal_gap_prompt(text))
        || is_bootstrap_noise_text(text)
        || is_codex_turn_aborted_wrapper_text(text)
        || text.starts_with("You are replying to a Kota BBS thread")
        || text.starts_with("You are creating a Kota BBS thread")
        || text.starts_with("It's time to dream.")
        || extract_ember_dream_entry_text(text).is_some()
    {
        return false;
    }
    let human = event.role == "user" || intent == Some("telegram");
    if human_only || human {
        return human;
    }
    if event.role != "assistant" {
        return false;
    }
    intent == Some("reminder")
        || matches!(
            event.shell.as_str(),
            "claude" | "codex" | "antigravity" | "opencode" | "kimi" | "pi" | "kota-local"
        )
        // agent_provider is a mutable identity snapshot, not the record's source.
        || (event.shell == "system"
            && event.source.native_event_id.as_deref().is_some_and(|id| id.starts_with("agentbus-"))
            && (intent.is_none() || matches!(intent, Some("handoff" | "review" | "status" | "task"))))
}

fn previewable(event: &ChathistoryEvent) -> bool {
    searchable(event, false)
        || (event.display
            && (matches!(event.kind.as_str(), "compaction" | "interrupt")
                || is_codex_turn_aborted_wrapper_text(&event.text)))
}

pub(super) fn read_around(
    project_root: &Path,
    around: &VioletRoomAround,
) -> Result<(Vec<VioletChatMessage>, String), String> {
    let segments = segments(project_root)?;
    let encoded_id = json_fragment(&around.id);
    for (segment_index, segment) in segments.iter().enumerate() {
        let bytes = read_segment(segment)?;
        let lines = segment_lines(&bytes);
        for (line_index, line) in lines.iter().enumerate().rev() {
            if memchr::memmem::find(line, encoded_id.as_bytes()).is_none()
                && !has_alternate_json_escapes(line)
            {
                continue;
            }
            let Ok(event) = serde_json::from_slice::<ChathistoryEvent>(line) else {
                continue;
            };
            if event.id != around.id {
                continue;
            }
            if !previewable(&event) {
                return Err("Room preview anchor is not in the preview collection".into());
            }
            let mut messages = preview_neighbors(
                &segments,
                segment_index,
                &lines,
                line_index,
                around.before,
                true,
            )?;
            messages.reverse();
            let target = message_from_chathistory_event(event);
            let target_key = room_message_dedupe_key(&target);
            messages.push(target);
            messages.extend(preview_neighbors(
                &segments,
                segment_index,
                &lines,
                line_index,
                around.after,
                false,
            )?);
            let messages = dedupe_room_messages(messages);
            let resolved_target_id = messages
                .iter()
                .find(|message| room_message_dedupe_key(message) == target_key)
                .expect("room dedupe retains one representative of the anchor group")
                .id
                .clone();
            return Ok((messages, resolved_target_id));
        }
    }
    Err("Room preview anchor was not found".into())
}

fn preview_neighbors(
    segments: &[Segment],
    anchor_segment: usize,
    anchor_lines: &[&[u8]],
    anchor_line: usize,
    count: usize,
    before: bool,
) -> Result<Vec<VioletChatMessage>, String> {
    let mut messages = Vec::new();
    if count == 0 {
        return Ok(messages);
    }
    let indices: Vec<usize> = if before {
        (anchor_segment..segments.len()).collect()
    } else {
        (0..=anchor_segment).rev().collect()
    };
    for index in indices {
        let anchor = index == anchor_segment;
        // Keep the anchor segment's original read; a concurrent append/rewrite must
        // not change the meaning of its line number between locating and slicing.
        let bytes;
        let other_lines;
        let lines = if anchor {
            anchor_lines
        } else {
            bytes = read_segment(&segments[index])?;
            other_lines = segment_lines(&bytes);
            &other_lines
        };
        let line_indices: Box<dyn Iterator<Item = usize>> = if before {
            Box::new((0..if anchor { anchor_line } else { lines.len() }).rev())
        } else {
            Box::new(if anchor { anchor_line + 1 } else { 0 }..lines.len())
        };
        for line in line_indices {
            let Ok(event) = serde_json::from_slice::<ChathistoryEvent>(lines[line]) else {
                continue;
            };
            if previewable(&event) {
                messages.push(message_from_chathistory_event(event));
                if messages.len() == count {
                    return Ok(messages);
                }
            }
        }
    }
    Ok(messages)
}

#[cfg(test)]
#[path = "room_search_tests.rs"]
mod tests;
