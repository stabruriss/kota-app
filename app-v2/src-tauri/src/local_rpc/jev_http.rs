//! Dance's direct classifier transport. No Pi process, model turn, transcript, or provider auth.
//! One pooled Agent is reused; the key is a request header only and errors never contain it.
use super::{
    error::{LocalError, LocalResult},
    jev,
    owned::OwnedDirectory,
};
use serde_json::{json, Value};
use std::{
    error::Error,
    io::{self, Read},
    sync::OnceLock,
    time::{Duration, Instant},
};

pub(crate) const DEADLINE: Duration = Duration::from_secs(3);
const MAX_REPLY: u64 = 1024 * 1024;
pub(crate) fn base_url(provider: &str) -> Option<&'static str> {
    match provider {
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        "vercel-ai-gateway" => Some("https://ai-gateway.vercel.sh/typesafe/v1"),
        "opencode" => Some("https://opencode.ai/zen/v1"),
        "typesafe" => Some("https://api.typesafe.ai/v1"),
        _ => None,
    }
}
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout(DEADLINE)
            .redirects(0)
            .build()
    })
}
fn timed_out(error: &(dyn Error + 'static)) -> bool {
    let mut cause = Some(error);
    while let Some(e) = cause {
        if e.downcast_ref::<io::Error>().is_some_and(|e| {
            matches!(
                e.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            )
        }) {
            return true;
        }
        cause = e.source();
    }
    false
}
pub(crate) fn classify(owned: &OwnedDirectory, body: Value) -> LocalResult<Value> {
    jev::gate(owned)?;
    let credential = jev::current(owned)?.ok_or_else(|| LocalError::new("jev_auth"))?;
    request(
        agent(),
        owned,
        &credential,
        base_url(&credential.provider).unwrap(),
        body,
        DEADLINE,
    )
}
/// Endpoint/deadline injection is private to the module's loopback tests, never an IPC parameter.
fn request(
    agent: &ureq::Agent,
    owned: &OwnedDirectory,
    credential: &jev::JevCredential,
    base: &str,
    mut body: Value,
    deadline: Duration,
) -> LocalResult<Value> {
    body["model"] = json!(credential.model_id);
    let started = Instant::now();
    let response = agent
        .post(&format!("{}/systemone", base.trim_end_matches('/')))
        .timeout(deadline)
        .set("Authorization", &format!("Bearer {}", credential.key))
        .send_json(body);
    let result = match response {
        Err(ureq::Error::Status(401 | 403, _)) => {
            let _ = jev::mark_rejected(owned, credential);
            Err(LocalError::new("jev_auth"))
        }
        Err(ureq::Error::Transport(e)) if timed_out(&e) || started.elapsed() >= deadline => {
            Err(LocalError::new("jev_timeout"))
        }
        Err(_) => Err(LocalError::new("jev_unreachable")),
        Ok(response) if !(200..300).contains(&response.status()) => {
            Err(LocalError::new("jev_unreachable"))
        }
        Ok(response) => {
            let mut bytes = Vec::new();
            match response
                .into_reader()
                .take(MAX_REPLY + 1)
                .read_to_end(&mut bytes)
            {
                Err(e) if timed_out(&e) || started.elapsed() >= deadline => {
                    Err(LocalError::new("jev_timeout"))
                }
                Err(_) => Err(LocalError::new("jev_unreachable")),
                Ok(_) if bytes.len() as u64 > MAX_REPLY => Err(LocalError::new("jev_unreachable")),
                Ok(_) => {
                    serde_json::from_slice(&bytes).map_err(|_| LocalError::new("jev_unreachable"))
                }
            }
        }
    };
    crate::kota_debug_log(&format!(
        "[puppeteer] jev {}ms {}",
        started.elapsed().as_millis(),
        result
            .as_ref()
            .map(|_| "ok")
            .unwrap_or_else(|e| e.code.as_str())
    ));
    result
}

#[cfg(test)]
#[path = "jev_http_tests.rs"]
mod tests;
