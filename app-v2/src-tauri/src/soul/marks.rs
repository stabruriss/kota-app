//! `under-table/soul/marks.json`: at most 20 marks of at most 200 characters, a
//! revision every write bumps, and `used` (times pushed) kept by the push recorder.
use super::{episodes, lock, write_json_atomic, SoulPaths};
use serde::{Deserialize, Serialize};
use std::{fs, io};

pub(crate) const MARK_LIMIT: usize = 20;
pub(crate) const MARK_TEXT_CHARS: usize = 200;
const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Mark {
    pub id: String,
    pub text: String,
    pub created_at: String,
    pub updated_at: String,
    pub project: Option<String>,
    #[serde(default)]
    pub used: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Marks {
    pub version: u32,
    pub revision: u64,
    pub marks: Vec<Mark>,
}

impl Default for Marks {
    fn default() -> Self {
        Self { version: VERSION, revision: 0, marks: Vec::new() }
    }
}

impl Marks {
    pub(crate) fn find(&self, id: &str) -> Option<&Mark> {
        self.marks.iter().find(|mark| mark.id == id)
    }
}

pub(crate) enum Text {
    Ok(String),
    Empty,
    TooLong(usize),
}

/// A mark is one line: whitespace runs (newlines included) collapse to one space. The
/// limit counts Unicode characters, not bytes.
pub(crate) fn check_text(text: &str) -> Text {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Text::Empty;
    }
    let chars = text.chars().count();
    if chars > MARK_TEXT_CHARS {
        return Text::TooLong(chars);
    }
    Text::Ok(text)
}

pub(crate) enum Added {
    Added { id: String, marks: Marks },
    Limit,
    Empty,
    TooLong(usize),
}

pub(crate) enum Edited {
    Updated(Marks),
    NotFound,
    Empty,
    TooLong(usize),
}

pub(crate) enum Deleted {
    Deleted(Marks),
    NotFound,
}

/// The current file, read under the soul lock: a concurrent write or `used` bump is
/// never half seen (the file is replaced atomically either way).
pub(crate) fn snapshot(paths: &SoulPaths) -> Result<Marks, String> {
    let _lock = lock(&paths.soul_lock())?;
    read(paths)
}

/// Reads without taking the lock, for callers that already hold it.
pub(crate) fn read(paths: &SoulPaths) -> Result<Marks, String> {
    let path = paths.marks();
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Marks::default()),
        Err(err) => return Err(format!("read {}: {err}", path.display())),
    };
    let marks: Marks =
        serde_json::from_slice(&bytes).map_err(|err| format!("parse {}: {err}", path.display()))?;
    if marks.version != VERSION {
        return Err(format!("{} has unsupported version {}", path.display(), marks.version));
    }
    Ok(marks)
}

fn write(paths: &SoulPaths, marks: &Marks) -> Result<(), String> {
    write_json_atomic(&paths.marks(), marks)
}

pub(crate) fn add(paths: &SoulPaths, text: &str, project: Option<String>, ts: &str) -> Result<Added, String> {
    let text = match check_text(text) {
        Text::Ok(text) => text,
        Text::Empty => return Ok(Added::Empty),
        Text::TooLong(chars) => return Ok(Added::TooLong(chars)),
    };
    let _lock = lock(&paths.soul_lock())?;
    let mut marks = read(paths)?;
    if marks.marks.len() >= MARK_LIMIT {
        return Ok(Added::Limit);
    }
    let id = mint_id(&marks, ts);
    marks.marks.push(Mark {
        id: id.clone(),
        text,
        created_at: ts.into(),
        updated_at: ts.into(),
        project,
        used: 0,
    });
    marks.revision += 1;
    write(paths, &marks)?;
    Ok(Added::Added { id, marks })
}

/// The mark keeps its id, usage and cooldown. Replaced text does not enter episodes.
pub(crate) fn edit(paths: &SoulPaths, id: &str, text: &str, ts: &str) -> Result<Edited, String> {
    let text = match check_text(text) {
        Text::Ok(text) => text,
        Text::Empty => return Ok(Edited::Empty),
        Text::TooLong(chars) => return Ok(Edited::TooLong(chars)),
    };
    let _lock = lock(&paths.soul_lock())?;
    let mut marks = read(paths)?;
    let Some(index) = marks.marks.iter().position(|mark| mark.id == id) else {
        return Ok(Edited::NotFound);
    };
    marks.marks[index].text = text;
    marks.marks[index].updated_at = ts.into();
    marks.revision += 1;
    write(paths, &marks)?;
    Ok(Edited::Updated(marks))
}

/// A deleted mark retires under its own id.
pub(crate) fn delete(paths: &SoulPaths, id: &str, ts: &str) -> Result<Deleted, String> {
    let _lock = lock(&paths.soul_lock())?;
    let mut marks = read(paths)?;
    let Some(index) = marks.marks.iter().position(|mark| mark.id == id) else {
        return Ok(Deleted::NotFound);
    };
    let mark = marks.marks[index].clone();
    episodes::append(
        paths,
        &[episodes::Episode::retired_mark(mark.id, mark.project, ts, mark.text)],
    )?;
    marks.marks.remove(index);
    marks.revision += 1;
    write(paths, &marks)?;
    Ok(Deleted::Deleted(marks))
}

/// Tallies pushes on the marks' own ids. The caller holds the soul lock; the revision
/// stays, since a tally is not a change the agent made.
pub(crate) fn bump_used_locked(paths: &SoulPaths, ids: &[&str]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut marks = read(paths)?;
    let mut changed = false;
    for mark in &mut marks.marks {
        let hits = ids.iter().filter(|id| **id == mark.id).count() as u64;
        if hits > 0 {
            mark.used += hits;
            changed = true;
        }
    }
    if changed {
        write(paths, &marks)?;
    }
    Ok(())
}

/// `m-YYYYMMDD-xxxx`: the day the mark was made and four hex digits, unique in the file.
fn mint_id(marks: &Marks, ts: &str) -> String {
    let day = ts.chars().take(10).filter(char::is_ascii_digit).collect::<String>();
    loop {
        let random = uuid::Uuid::new_v4().simple().to_string();
        let id = format!("m-{day}-{}", &random[..4]);
        if marks.find(&id).is_none() {
            return id;
        }
    }
}
