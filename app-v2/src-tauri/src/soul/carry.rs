//! Carrying a soul between incarnations (SOUL-PLAN v4 §17 转世): copy marks, pushes,
//! episodes and usage from one under-table to another, re-writing ledgers through the
//! tolerant reader (a torn last line from a writer mid-append is dropped), leaving the
//! rebuildable index and lock files behind, and giving `state.json` a stint in the new
//! project. Also the counts a recruit confirmation can show. A root is an agent CWD or a
//! Tavern hero directory: both keep the soul at `<root>/under-table/`.
use super::{
    append_lines,
    episodes::{Episode, Usage},
    export::{self, ProjectState, State, Stint},
    marks,
    pushes::Push,
    read_jsonl, SoulPaths,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{fs, path::Path};

/// The project an incarnation lands in.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NewProject<'a> {
    pub slug: &'a str,
    pub name: &'a str,
    pub root: &'a Path,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SoulCounts {
    pub marks: usize,
    pub episodes: usize,
}

pub(crate) fn has_soul(root: &Path) -> bool {
    SoulPaths::new(root).under_table().is_dir()
}

/// Marks and episodes under a root; unreadable or absent files count zero.
pub(crate) fn counts(root: &Path) -> SoulCounts {
    let paths = SoulPaths::new(root);
    SoulCounts {
        marks: marks::read(&paths).map(|marks| marks.marks.len()).unwrap_or(0),
        episodes: read_jsonl::<Episode>(&paths.episodes()).map(|pool| pool.len()).unwrap_or(0),
    }
}

/// Copies `from`'s under-table into `to`, which must not have one yet. marks.json goes
/// as the document it is; the ledgers are read tolerantly and written whole; the index
/// is not copied. With a project, state.json closes every open stint and opens one
/// there. Fails as a whole; the caller removes `to`.
pub(crate) fn carry(from: &Path, to: &Path, project: Option<NewProject<'_>>, now: &str) -> Result<SoulCounts, String> {
    let src = SoulPaths::new(from);
    let dst = SoulPaths::new(to);
    if dst.under_table().exists() {
        return Err(format!("{} already has an under-table", to.display()));
    }
    if src.marks().is_file() {
        let bytes = fs::read(src.marks()).map_err(|err| format!("read {}: {err}", src.marks().display()))?;
        serde_json::from_slice::<marks::Marks>(&bytes).map_err(|err| format!("parse {}: {err}", src.marks().display()))?;
        fs::create_dir_all(dst.soul_dir()).map_err(|err| format!("create {}: {err}", dst.soul_dir().display()))?;
        crate::adapter_sync::atomic_replace(&dst.marks(), &bytes)?;
    }
    copy_jsonl::<Push>(&src.pushes(), &dst.pushes())?;
    copy_jsonl::<Episode>(&src.episodes(), &dst.episodes())?;
    copy_jsonl::<Usage>(&src.episodes_usage(), &dst.episodes_usage())?;
    let had_state = src.episodes_state().is_file();
    let mut state = export::read_state(&src)?;
    let has_project = project.is_some();
    if let Some(project) = project {
        stint(&mut state, project, now);
    }
    if had_state || has_project {
        export::write_state(&dst, &state)?;
    }
    Ok(counts(to))
}

fn copy_jsonl<T: DeserializeOwned + Serialize>(from: &Path, to: &Path) -> Result<(), String> {
    if !from.is_file() {
        return Ok(());
    }
    let rows: Vec<T> = read_jsonl(from)?;
    let lines = rows
        .iter()
        .map(|row| serde_json::to_string(row).map_err(|err| err.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    append_lines(to, &lines)
}

/// Closes every open stint and opens one in the new project (sample §6: a soul back in
/// a project it knew gets another stint, not a new entry).
pub(crate) fn stint(state: &mut State, project: NewProject<'_>, now: &str) {
    for entry in state.projects.values_mut() {
        for stint in &mut entry.stints {
            if stint.to.is_none() {
                stint.to = Some(now.to_string());
            }
        }
    }
    let entry = state.projects.entry(project.slug.to_string()).or_insert_with(|| ProjectState {
        name: project.name.to_string(),
        root: project.root.display().to_string(),
        ..ProjectState::default()
    });
    entry.stints.push(Stint { from: now.to_string(), to: None });
    state.current_project = Some(project.slug.to_string());
}
