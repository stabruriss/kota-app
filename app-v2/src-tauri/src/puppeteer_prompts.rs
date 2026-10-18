//! The account's editable Jev questions. Invalid files are preserved and fall back as a whole.
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub(crate) const FILE: &str = "puppeteer-prompts.json";
pub(crate) const INVALID: &str = "Puppeteer prompt file is invalid. Using bundled defaults.";
pub(crate) const UNREADABLE: &str =
    "Could not read the Puppeteer prompt file. Using bundled defaults.";
pub(crate) const BUNDLED: &str = include_str!("../../kota-ext/puppeteer-prompts.json");
const MAX_BYTES: usize = 1024 * 1024;

pub(crate) fn builtin() -> &'static Value {
    static VALUE: OnceLock<Value> = OnceLock::new();
    VALUE.get_or_init(|| serde_json::from_str(BUNDLED).expect("checked-in Puppeteer prompts"))
}
fn keys(value: &Value, expected: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|o| o.len() == expected.len() && expected.iter().all(|k| o.contains_key(*k)))
}
fn text(value: &Value) -> bool {
    value.as_str().is_some_and(|s| !s.contains('\0'))
}
pub(crate) fn valid(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if value["schema"].as_f64() != Some(1.0)
        || object.keys().any(|k| {
            ![
                "schema",
                "defaultIntents",
                "stringFirst",
                "stringLater",
                "danceContext",
                "danceRule",
            ]
            .contains(&k.as_str())
        })
    {
        return false;
    }
    if let Some(intents) = object.get("defaultIntents") {
        if intents.as_array().is_none_or(|a| !a.iter().all(text)) {
            return false;
        }
    }
    for (name, question, criterion) in [
        ("stringFirst", "model", "<model_id>"),
        ("stringLater", "model", "<model_id>"),
        ("danceContext", "target", "<agent_id>"),
        ("danceRule", "target", "<agent_id>"),
    ] {
        let part = &value[name];
        let template = &part["questions"][question];
        if !keys(part, &["questions"])
            || !keys(&part["questions"], &[question])
            || !keys(template, &["type", "instructions", "criteria"])
            || template["type"] != "choice"
            || !text(&template["instructions"])
            || template["instructions"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty() || s.chars().count() > 512)
            || !keys(&template["criteria"], &[criterion])
            || !text(&template["criteria"][criterion])
        {
            return false;
        }
    }
    true
}
pub(crate) fn file_content(path: &Path) -> Result<String, &'static str> {
    let stat = fs::symlink_metadata(path).map_err(|_| UNREADABLE)?;
    if !stat.is_file() || stat.file_type().is_symlink() || stat.len() > MAX_BYTES as u64 {
        return Err(INVALID);
    }
    let bytes = fs::read(path).map_err(|_| UNREADABLE)?;
    if bytes.len() > MAX_BYTES {
        return Err(INVALID);
    }
    String::from_utf8(bytes).map_err(|_| UNREADABLE)
}
/// Used by the existing ensure path so malformed files are never overwritten, even at startup.
pub(crate) fn valid_file(path: &Path) -> bool {
    file_content(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .is_some_and(|v| valid(&v))
}
pub(crate) struct Loaded {
    pub path: PathBuf,
    pub content: String,
    pub value: Value,
    pub warning: Option<&'static str>,
}
fn load(path: PathBuf) -> Loaded {
    let parsed = file_content(&path).and_then(|content| {
        let value: Value = serde_json::from_str(&content).map_err(|_| INVALID)?;
        if !valid(&value) {
            return Err(INVALID);
        }
        Ok((content.trim_end().to_owned(), value))
    });
    match parsed {
        Ok((content, value)) => Loaded {
            path,
            content,
            value,
            warning: None,
        },
        Err(warning) => Loaded {
            path,
            content: BUNDLED.trim_end().to_owned(),
            value: builtin().clone(),
            warning: Some(warning),
        },
    }
}
/// Every fresh classification/preview takes one effective snapshot, never a cached user file.
pub(crate) fn read() -> Loaded {
    let template =
        crate::system_prompt_template_by_file_name(FILE).expect("registered Puppeteer template");
    let path = crate::system_prompt_account_path(template);
    if fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        let _ = crate::ensure_system_prompt_template(template, false);
    }
    // Existing user files are read once, not first reread by ensure/upgrade logic.
    load(path)
}

#[cfg(test)]
#[path = "puppeteer_prompts_tests.rs"]
mod tests;
