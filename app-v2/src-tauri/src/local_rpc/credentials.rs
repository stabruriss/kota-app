use super::{
    error::{identifier, LocalError, LocalResult},
    owned::{read_json, OwnedDirectory},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuthType {
    ApiKey,
    Oauth,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Capabilities {
    pub provider_id: String,
    pub provider_label: String,
    pub can_api_key: bool,
    #[serde(rename = "canOAuth")]
    pub can_o_auth: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeySyntax {
    Literal,
    Command,
    Environment,
}

/// Pi 1.3.0 core/resolve-config-value.js:18-69. Classify only: never trim,
/// interpolate, execute, or include the input in an error. The pin regression
/// compares this with Pi's parser, including malformed templates and escapes.
pub(crate) fn key_syntax(key: &str) -> KeySyntax {
    if key.starts_with('!') {
        return KeySyntax::Command;
    }
    let bytes = key.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }
        match bytes.get(i + 1).copied() {
            Some(b'$' | b'!') => i += 2,
            Some(b'{') => {
                if let Some(end) = bytes[i + 2..]
                    .iter()
                    .position(|b| *b == b'}')
                    .map(|n| n + i + 2)
                {
                    if variable(&bytes[i + 2..end]) {
                        return KeySyntax::Environment;
                    }
                    i = end + 1;
                } else {
                    i += 1;
                }
            }
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => return KeySyntax::Environment,
            _ => i += 1,
        }
    }
    KeySyntax::Literal
}
fn variable(bytes: &[u8]) -> bool {
    bytes
        .first()
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}
pub(crate) fn validate_literal_key(key: &str, provider: &str) -> LocalResult<()> {
    if key.is_empty() || key.len() > 64 * 1024 || key_syntax(key) != KeySyntax::Literal {
        return Err(LocalError::provider("credential_invalid", provider));
    }
    Ok(())
}

/// Deliberately not Debug/Serialize. Only sanitized identities and types leave
/// this module. Every read is fresh, including after another App changes Link.
pub(crate) struct AuthStore {
    entries: Map<String, Value>,
}
impl AuthStore {
    pub fn load(directory: &OwnedDirectory) -> LocalResult<Self> {
        directory.validate_existing()?;
        let value = read_json(&directory.auth_path(), "auth_store_read_failed")?
            .unwrap_or_else(|| Value::Object(Map::new()));
        let entries = value
            .as_object()
            .ok_or_else(|| LocalError::new("auth_store_invalid"))?
            .clone();
        if entries.len() > 256 || entries.keys().any(|id| !identifier(id, 128)) {
            return Err(LocalError::new("auth_store_invalid"));
        }
        Ok(Self { entries })
    }
    /// Must run before creating ModelRuntime, even when the desired model is
    /// another provider: Pi may inspect every credential during availability.
    pub fn preflight(&self) -> LocalResult<()> {
        for (provider, value) in &self.entries {
            let object = value
                .as_object()
                .ok_or_else(|| LocalError::provider("credential_invalid", provider))?;
            match object.get("type").and_then(Value::as_str) {
                Some("api_key") => {
                    if let Some(key) = object.get("key") {
                        let key = key
                            .as_str()
                            .ok_or_else(|| LocalError::provider("credential_invalid", provider))?;
                        if key_syntax(key) != KeySyntax::Literal {
                            return Err(LocalError::provider("credential_invalid", provider));
                        }
                    }
                }
                Some("oauth") => {} // OAuth tokens do not use Pi's key parser.
                _ => return Err(LocalError::provider("credential_invalid", provider)),
            }
        }
        Ok(())
    }
    pub fn shape(&self, provider: &str) -> LocalResult<AuthType> {
        let value = self
            .entries
            .get(provider)
            .ok_or_else(|| LocalError::provider("provider_not_signed_in", provider))?;
        let object = value
            .as_object()
            .ok_or_else(|| LocalError::provider("credential_invalid", provider))?;
        let invalid = || LocalError::provider("credential_invalid", provider);
        match object.get("type").and_then(Value::as_str) {
            Some("oauth") => {
                if !object
                    .get("access")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
                    || object.get("refresh").and_then(Value::as_str).is_none()
                    || !object
                        .get("expires")
                        .and_then(Value::as_f64)
                        .is_some_and(f64::is_finite)
                {
                    return Err(invalid());
                }
                // OpenRouter's native OAuth stores refresh="". Native extras
                // (Codex accountId / OpenAI clientId, scopes) stay untouched.
                Ok(AuthType::Oauth)
            }
            Some("api_key") => {
                let env = match object.get("env") {
                    None => None,
                    Some(value) => {
                        let env = value.as_object().ok_or_else(invalid)?;
                        if env.values().any(|v| v.as_str().is_none()) {
                            return Err(invalid());
                        }
                        Some(env)
                    }
                };
                if let Some(key) = object.get("key") {
                    validate_literal_key(key.as_str().ok_or_else(invalid)?, provider)?;
                } else {
                    match provider {
                        "amazon-bedrock" => {
                            // Pi native credential-chain is exactly a keyless
                            // record; profile, when present, must be nonempty.
                            if env.is_some_and(|e| {
                                e.contains_key("AWS_PROFILE") && !nonempty(e, "AWS_PROFILE")
                            }) {
                                return Err(invalid());
                            }
                        }
                        "google-vertex" => {
                            let env = env.ok_or_else(invalid)?;
                            if !nonempty(env, "GOOGLE_CLOUD_PROJECT")
                                || !nonempty(env, "GOOGLE_CLOUD_LOCATION")
                                || (env.contains_key("GOOGLE_APPLICATION_CREDENTIALS")
                                    && !nonempty(env, "GOOGLE_APPLICATION_CREDENTIALS"))
                            {
                                return Err(invalid());
                            }
                        }
                        _ => return Err(invalid()),
                    }
                }
                if provider == "azure" {
                    let address = env.is_some_and(|e| {
                        nonempty(e, "AZURE_OPENAI_RESOURCE_NAME")
                            || e.get("AZURE_OPENAI_BASE_URL")
                                .and_then(Value::as_str)
                                .is_some_and(valid_resource_url)
                    });
                    if !address {
                        return Err(LocalError::provider("azure_resource_missing", provider));
                    }
                }
                Ok(AuthType::ApiKey)
            }
            _ => Err(invalid()),
        }
    }
    /// A fresh, owned literal key for a gateway request. Never exposed in an IPC DTO.
    pub fn api_key(&self, provider: &str) -> LocalResult<Option<&str>> {
        if !self.entries.contains_key(provider) { return Ok(None); }
        if self.shape(provider)? != AuthType::ApiKey { return Err(LocalError::provider("credential_invalid", provider)); }
        self.entries[provider]["key"].as_str().map(Some)
            .ok_or_else(|| LocalError::provider("credential_invalid", provider))
    }
    pub fn gate(&self, provider: &str, capabilities: &[Capabilities]) -> LocalResult<AuthType> {
        if !identifier(provider, 128) {
            return Err(LocalError::new("invalid_request"));
        }
        let kind = self.shape(provider)?;
        let handler = capabilities
            .iter()
            .find(|p| p.provider_id == provider)
            .ok_or_else(|| LocalError::provider("auth_handler_unavailable", provider))?;
        if provider == "llama.cpp"
            || !match kind {
                AuthType::ApiKey => handler.can_api_key,
                AuthType::Oauth => handler.can_o_auth,
            }
        {
            return Err(LocalError::provider("auth_handler_unavailable", provider));
        }
        Ok(kind)
    }
}
fn nonempty(object: &Map<String, Value>, name: &str) -> bool {
    object
        .get(name)
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
}
pub(crate) fn valid_resource_url(value: &str) -> bool {
    if value.len() > 4096 || value.chars().any(char::is_control) {
        return false;
    }
    url::Url::parse(value).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}
