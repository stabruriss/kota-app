//! `search-episode`: an FTS5 index over `episodes.jsonl` (rebuilt whenever the pool
//! changed; deletable), BM25 relevance over CJK 2-grams, heat A = ln Σ w·t^-0.5 from
//! `usage.jsonl` ordering only inside the relevant set, five hits with one neighbour
//! line each, and the §K.3 reminder (SOUL-PLAN v4 §2/§9/§18.2; TEXTS §D/§K.3).
use super::{
    date_of,
    episodes::{self, Episode, Usage},
    lock, read_jsonl, SoulPaths,
};
use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension};
use std::{collections::HashSet, fs, time::Duration};

const SCHEMA_VERSION: &str = "1";
/// The relevant set: the best BM25 matches heat may reorder.
const RELEVANT: usize = 20;
/// How much heat may add on top of relevance (座山雕 2026-10-15): bounded, so a weakly
/// relevant hot episode never passes the most relevant cold one.
const HEAT_BONUS: f64 = 0.3;
pub(crate) const HITS: usize = 5;
const SCORED_HITS: usize = 4;
pub(crate) const RETURN_W: f64 = 0.5;
pub(crate) const EXPAND_W: f64 = 1.0;
pub(crate) const REMIND_EXPANDED: u64 = 2;
pub(crate) const REMIND_RETURNED: u64 = 4;
const FIRST_SLICE_CHARS: usize = 120;
const WINDOW_CHARS: usize = 60;
const NEIGHBOUR_CHARS: usize = 80;

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xAC00..=0xD7AF | 0x20000..=0x2FFFF
    )
}

fn flush_word(word: &mut String, out: &mut Vec<String>) {
    if !word.is_empty() {
        out.push(word.to_lowercase());
        word.clear();
    }
}

fn flush_cjk(run: &mut Vec<char>, out: &mut Vec<String>) {
    if run.len() == 1 {
        out.push(run[0].to_string());
    }
    for pair in run.windows(2) {
        out.push(pair.iter().collect());
    }
    run.clear();
}

/// Lower-cased words for scripts with spaces, 2-grams for CJK runs (a lone CJK character
/// stays itself). The same function feeds the index and the query, so BM25 sees one
/// vocabulary without a custom tokenizer.
pub(crate) fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut run = Vec::new();
    for ch in text.chars() {
        if is_cjk(ch) {
            flush_word(&mut word, &mut out);
            run.push(ch);
        } else if ch.is_alphanumeric() || ch == '_' {
            flush_cjk(&mut run, &mut out);
            word.push(ch);
        } else {
            flush_word(&mut word, &mut out);
            flush_cjk(&mut run, &mut out);
        }
    }
    flush_word(&mut word, &mut out);
    flush_cjk(&mut run, &mut out);
    out
}

fn sql(err: rusqlite::Error) -> String {
    format!("episodes index: {err}")
}

fn open(paths: &SoulPaths) -> Result<Connection, String> {
    let path = paths.episodes_index();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let conn = Connection::open(&path).map_err(sql)?;
    conn.busy_timeout(Duration::from_millis(250)).map_err(sql)?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL)").map_err(sql)?;
    Ok(conn)
}

/// Size and mtime of the pool: the index is rebuilt whenever they moved.
fn source_stamp(paths: &SoulPaths) -> String {
    match fs::metadata(paths.episodes()) {
        Ok(meta) => {
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("{}:{modified}", meta.len())
        }
        Err(_) => "0:0".into(),
    }
}

fn meta(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |row| row.get(0))
        .optional()
        .map_err(sql)
}

/// Opens the index, rebuilding it from the pool when missing, stale or of another
/// schema. The index is a cache: deleting the file is always safe.
pub(crate) fn open_current(paths: &SoulPaths) -> Result<Connection, String> {
    let stamp = source_stamp(paths);
    let conn = open(paths)?;
    if meta(&conn, "schema")?.as_deref() == Some(SCHEMA_VERSION) && meta(&conn, "stamp")?.as_deref() == Some(&stamp) {
        return Ok(conn);
    }
    let _lock = lock(&paths.episodes_lock())?;
    let stamp = source_stamp(paths);
    rebuild(&conn, paths, &stamp)?;
    Ok(conn)
}

fn rebuild(conn: &Connection, paths: &SoulPaths, stamp: &str) -> Result<(), String> {
    let pool: Vec<Episode> = read_jsonl(&paths.episodes())?;
    conn.execute_batch(
        "BEGIN;
         DROP TABLE IF EXISTS episodes;
         DROP TABLE IF EXISTS fts;
         CREATE TABLE episodes(id TEXT PRIMARY KEY, kind TEXT NOT NULL, project TEXT, ts TEXT NOT NULL,
             from_name TEXT, prev TEXT, text TEXT NOT NULL, indexed INTEGER NOT NULL, w0 REAL);
         CREATE VIRTUAL TABLE fts USING fts5(id UNINDEXED, body);",
    )
    .map_err(sql)?;
    let result = (|| {
        let mut rows = conn
            .prepare("INSERT INTO episodes(id, kind, project, ts, from_name, prev, text, indexed, w0) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)")
            .map_err(sql)?;
        let mut bodies = conn.prepare("INSERT INTO fts(id, body) VALUES (?1, ?2)").map_err(sql)?;
        let mut seen = HashSet::new();
        for episode in &pool {
            // A retried write may repeat a line; the first copy of an id stands.
            if !seen.insert(episode.id.clone()) {
                continue;
            }
            rows.execute(params![
                episode.id,
                episode.kind,
                episode.project,
                episode.ts,
                episode.from,
                episode.prev,
                episode.text,
                episode.indexed as i64,
                episode.w0
            ])
            .map_err(sql)?;
            if episode.indexed {
                bodies.execute(params![episode.id, tokens(&episode.text).join(" ")]).map_err(sql)?;
            }
        }
        for (key, value) in [("schema", SCHEMA_VERSION), ("stamp", stamp)] {
            conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)", params![key, value]).map_err(sql)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => conn.execute_batch("COMMIT").map_err(sql),
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

/// One stored episode, as the hit list and the neighbour line need it.
#[derive(Clone, Debug)]
pub(crate) struct Stored {
    pub id: String,
    pub kind: String,
    pub project: Option<String>,
    pub ts: String,
    pub from: Option<String>,
    pub prev: Option<String>,
    pub text: String,
    pub w0: Option<f64>,
}

pub(crate) fn stored(conn: &Connection, id: &str) -> Result<Option<Stored>, String> {
    conn.query_row(
        "SELECT id, kind, project, ts, from_name, prev, text, w0 FROM episodes WHERE id = ?1",
        params![id],
        |row| {
            Ok(Stored {
                id: row.get(0)?,
                kind: row.get(1)?,
                project: row.get(2)?,
                ts: row.get(3)?,
                from: row.get(4)?,
                prev: row.get(5)?,
                text: row.get(6)?,
                w0: row.get(7)?,
            })
        },
    )
    .optional()
    .map_err(sql)
}

fn match_expression(terms: &[String], any: bool) -> String {
    let quoted = terms.iter().map(|term| format!("\"{}\"", term.replace('"', "\"\""))).collect::<Vec<_>>();
    quoted.join(if any { " OR " } else { " " })
}

/// The relevant set: up to twenty ids by BM25 (every term, else any term), best first.
fn relevant(conn: &Connection, terms: &[String], project: Option<&str>) -> Result<Vec<(String, f64)>, String> {
    for any in [false, true] {
        let mut statement = conn
            .prepare(
                "SELECT fts.id, bm25(fts) FROM fts JOIN episodes ON episodes.id = fts.id
                 WHERE fts MATCH ?1 AND (?2 IS NULL OR episodes.project = ?2)
                 ORDER BY bm25(fts), episodes.ts DESC LIMIT ?3",
            )
            .map_err(sql)?;
        let rows = statement
            .query_map(params![match_expression(terms, any), project, RELEVANT as i64], |row| {
                Ok((row.get::<_, String>(0)?, -row.get::<_, f64>(1)?))
            })
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        if !rows.is_empty() || terms.len() < 2 {
            return Ok(rows);
        }
    }
    Ok(Vec::new())
}

fn day(ts: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(date_of(ts), "%Y-%m-%d").ok()
}

fn decay(event_ts: &str, today: NaiveDate) -> f64 {
    let days = day(event_ts).map_or(0, |d| (today - d).num_days().max(0)) as f64;
    (days + 1.0).powf(-0.5)
}

/// Events of one id after its last clear-heat line, by ledger order (never by the
/// second-precision timestamps).
fn since_clear<'a>(usage: &'a [Usage], id: &str) -> (bool, Vec<&'a Usage>) {
    let clear_at = usage.iter().rposition(|u| u.id == id && u.op == "clear");
    let events = usage
        .iter()
        .enumerate()
        .filter(|(index, u)| u.id == id && u.op != "clear" && clear_at.map_or(true, |at| *index > at))
        .map(|(_, u)| u)
        .collect();
    (clear_at.is_some(), events)
}

/// A = ln Σ w·t^-0.5 over the episode's birth weight (gone after a clear) and its usage
/// since the last clear; t is the natural-day distance plus one. Nothing summed is -∞.
pub(crate) fn heat(usage: &[Usage], episode: &Stored, today: NaiveDate) -> f64 {
    let (cleared, events) = since_clear(usage, &episode.id);
    let mut sum = 0.0;
    if !cleared {
        sum += episode.w0.unwrap_or(0.0) * decay(&episode.ts, today);
    }
    for event in events {
        sum += event.w.unwrap_or(0.0) * decay(&event.ts, today);
    }
    sum.ln()
}

/// Returned/expanded counts since the last clear: the §K.3 reminder fires at ≥4 / ≥2.
pub(crate) fn counts_since_clear(usage: &[Usage], id: &str) -> (u64, u64) {
    let (_, events) = since_clear(usage, id);
    let returned = events.iter().filter(|u| u.op == "return").count() as u64;
    let expanded = events.iter().filter(|u| u.op == "expand").count() as u64;
    (returned, expanded)
}

#[derive(Clone, Debug)]
pub(crate) struct Hit {
    pub episode: Stored,
    /// The previous whitelisted message, for the `↳ before:` line.
    pub neighbour: Option<Stored>,
    /// `-bm25()`: larger is more relevant.
    pub relevance: f64,
    /// A from `heat`; -∞ when nothing is summed.
    pub heat: f64,
}

/// score = bm25_norm + 0.3 × heat_norm inside the relevant set: bm25_norm is the raw
/// BM25 ratio to the set's best, heat_norm the ratio of A (negative A counts as 0) to
/// the set's largest A, or 0 for every episode when none is above 0. Ties go to heat,
/// then to the newer episode. Four by score, then the newest relevant episode not yet
/// chosen as the fifth; fewer than five relevant episodes are all returned.
pub(crate) fn rank(mut hits: Vec<Hit>) -> Vec<Hit> {
    let max_relevance = hits.iter().map(|hit| hit.relevance).fold(0.0_f64, f64::max);
    let max_heat = hits.iter().map(|hit| hit.heat.max(0.0)).fold(0.0_f64, f64::max);
    let score = |hit: &Hit| {
        let relevance = if max_relevance > 0.0 { hit.relevance / max_relevance } else { 0.0 };
        let heat = if max_heat > 0.0 { hit.heat.max(0.0) / max_heat } else { 0.0 };
        relevance + HEAT_BONUS * heat
    };
    hits.sort_by(|a, b| {
        score(b)
            .total_cmp(&score(a))
            .then_with(|| b.heat.total_cmp(&a.heat))
            .then_with(|| b.episode.ts.cmp(&a.episode.ts))
    });
    if hits.len() <= HITS {
        return hits;
    }
    let rest = hits.split_off(SCORED_HITS);
    if let Some(newest) = rest.into_iter().max_by(|a, b| a.episode.ts.cmp(&b.episode.ts)) {
        hits.push(newest);
    }
    hits
}

/// Runs one query; the caller records the returns and renders.
pub(crate) fn search(paths: &SoulPaths, query: &str, project: Option<&str>, ts: &str) -> Result<(Vec<Hit>, Vec<Usage>), String> {
    let terms = tokens(query);
    let usage = episodes::read_usage(paths)?;
    if terms.is_empty() {
        return Ok((Vec::new(), usage));
    }
    let conn = open_current(paths)?;
    let today = day(ts).unwrap_or_default();
    let mut hits = Vec::new();
    for (id, relevance) in relevant(&conn, &terms, project)? {
        if let Some(episode) = stored(&conn, &id)? {
            let heat = heat(&usage, &episode, today);
            let neighbour = match episode.prev.as_deref() {
                Some(prev) => stored(&conn, prev)?,
                None => None,
            };
            hits.push(Hit { episode, neighbour, relevance, heat });
        }
    }
    Ok((rank(hits), usage))
}

fn chars_prefix(text: &str, limit: usize) -> (String, bool) {
    let mut out = text.chars().take(limit).collect::<String>();
    let cut = text.chars().nth(limit).is_some();
    if cut {
        out.push('…');
    }
    (out, cut)
}

/// The first sentence, at most 120 characters.
pub(crate) fn first_slice(text: &str) -> String {
    let text = text.trim();
    let end = text
        .char_indices()
        .find(|(_, ch)| matches!(ch, '。' | '！' | '？' | '!' | '?' | '\n'))
        .map(|(index, ch)| index + ch.len_utf8());
    let sentence = end.map_or(text, |end| &text[..end]).trim();
    chars_prefix(sentence, FIRST_SLICE_CHARS).0.replace('\n', " ")
}

/// About 60 characters either side of the first term that occurs, unless that is
/// already inside the first slice.
pub(crate) fn match_window(text: &str, terms: &[String]) -> Option<String> {
    let chars = text.chars().collect::<Vec<_>>();
    let lowered = text.to_lowercase().chars().collect::<Vec<_>>();
    let mut best: Option<usize> = None;
    for term in terms {
        let needle = term.chars().collect::<Vec<_>>();
        if needle.is_empty() || needle.len() > lowered.len() {
            continue;
        }
        if let Some(at) = lowered.windows(needle.len()).position(|window| window == needle.as_slice()) {
            best = Some(best.map_or(at, |b| b.min(at)));
        }
    }
    let at = best?;
    if at < FIRST_SLICE_CHARS {
        return None;
    }
    let start = at.saturating_sub(WINDOW_CHARS);
    let end = (at + WINDOW_CHARS).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(chars[start..end].iter());
    if end < chars.len() {
        out.push('…');
    }
    Some(out.replace('\n', " "))
}

pub(crate) fn neighbour_slice(text: &str) -> String {
    chars_prefix(text.trim(), NEIGHBOUR_CHARS).0.replace('\n', " ")
}

/// `you` for the incarnation's own words, the sender's name for a user message, and
/// `old dream` for an archived Dream.
pub(crate) fn who(episode: &Stored) -> String {
    if episode.kind == "old_dream" {
        return "old dream".into();
    }
    match episode.from.as_deref() {
        Some("self") | None | Some("") => "you".into(),
        Some(name) => name.into(),
    }
}
