use serde::Serialize;
use std::fmt;

/// Errors crossing IPC contain a reason and an identity, never file contents,
/// credential values, provider exceptions or command output.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalError {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
}

impl LocalError {
    pub fn new(code: &str) -> Self {
        Self {
            code: code.into(),
            provider_id: None,
            stage: None,
        }
    }
    pub fn provider(code: &str, provider: &str) -> Self {
        Self {
            code: code.into(),
            provider_id: Some(provider.into()),
            stage: None,
        }
    }
    pub fn at(mut self, stage: &str) -> Self {
        if self.stage.is_none() { self.stage = Some(stage.into()); }
        self
    }
}
impl fmt::Display for LocalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Older Local commands return strings. Keep their error equally safe.
        if let Some(provider) = &self.provider_id {
            write!(f, "{} ({provider})", self.code)
        } else {
            f.write_str(&self.code)
        }
    }
}
impl std::error::Error for LocalError {}
pub(crate) type LocalResult<T> = Result<T, LocalError>;

pub(super) fn identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}
