//! Explicit BBS destinations. Text is display-only; routing never interprets @
//! words in Markdown and never opens a control client or an Agent Bus here.
use super::*;
use crate::bbs_sync::{self, roster};

pub const PRIVATE_REPLY_ERROR: &str = "This agent is on another device, but this thread is not shared. No reply was posted. Start a new thread to @ this agent.";
pub const UNJOINED_NEW_ERROR: &str = "This agent is on another device, but this device is not in a sync group. No thread was created.";

/// Existing publication/attachment errors retain their string shape. Only the
/// three agreed mention failures become public fixed, single-field codes.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum PublishError {
    Code { code: &'static str },
    Message(String),
}
impl From<anyhow::Error> for PublishError {
    fn from(error: anyhow::Error) -> Self {
        let code = match error.to_string().as_str() {
            PRIVATE_REPLY_ERROR => Some("mention_thread_not_shared"),
            UNJOINED_NEW_ERROR => Some("mention_requires_group"),
            "agent_roster_not_synced" => Some("agent_roster_not_synced"),
            _ => None,
        };
        match code {
            Some(code) => Self::Code { code },
            None => Self::Message(format!("{error:#}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mention {
    pub device_id: String,
    pub project_id: String,
    pub agent_id: String,
}
impl Mention {
    pub fn validate(&self) -> Result<()> {
        if self.device_id != "local" {
            bbs_sync::valid_hash(&self.device_id).map_err(anyhow::Error::msg)?;
        }
        bbs_sync::safe_id(&self.project_id).map_err(anyhow::Error::msg)?;
        bbs_sync::safe_id(&self.agent_id).map_err(anyhow::Error::msg)
    }
    pub fn parse(target: &str) -> Result<Self> {
        let parts = target.split('/').collect::<Vec<_>>();
        if parts.len() != 3 {
            bail!("--at requires <deviceId|local>/<projectId>/<agentId>");
        }
        let mention = Self {
            device_id: parts[0].into(),
            project_id: parts[1].into(),
            agent_id: parts[2].into(),
        };
        mention.validate()?;
        Ok(mention)
    }
    /// A remote literal `local` is never rebound to the receiving machine.
    pub(crate) fn received_here(&self, device_id: &str) -> bool {
        self.device_id != "local" && self.device_id == device_id
    }
}

pub(crate) fn validate(mentions: &[Mention]) -> Result<()> {
    for mention in mentions {
        mention.validate()?;
    }
    Ok(())
}

pub(super) struct Prepared {
    targets: Vec<(Mention, String)>,
    membership: Option<(String, String)>,
}
impl Prepared {
    pub(super) fn load(
        content: &sync::ContentStore,
        targets: &[Mention],
        new_root: bool,
    ) -> Result<Self> {
        if targets.is_empty() {
            return Ok(Self {
                targets: Vec::new(),
                membership: None,
            });
        }
        validate(targets)?;
        let ctx = roster::context(&content.state)?;
        let local = if targets
            .iter()
            .any(|t| t.device_id == "local" || ctx.device_id.as_deref() == Some(&t.device_id))
        {
            crate::agent_directory::collect(content.account_dir())?.complete()?
        } else {
            Vec::new()
        };
        let mut peers = BTreeMap::new();
        let mut result = Vec::new();
        let mut seen = BTreeSet::new();
        for target in targets {
            let mut target = target.clone();
            let is_local =
                target.device_id == "local" || ctx.device_id.as_deref() == Some(&target.device_id);
            if is_local {
                target.device_id = ctx.device_id.clone().unwrap_or_else(|| "local".into());
            }
            if !seen.insert(target.clone()) {
                continue;
            }
            let name = if is_local {
                local
                    .iter()
                    .find(|p| p.id == target.project_id)
                    .and_then(|p| p.agents.iter().find(|a| a.id == target.agent_id))
                    .map(|a| a.name.clone())
            } else {
                if ctx.membership.is_none() {
                    bail!(
                        "{}",
                        if new_root {
                            UNJOINED_NEW_ERROR
                        } else {
                            PRIVATE_REPLY_ERROR
                        }
                    );
                }
                if !peers.contains_key(&target.device_id) {
                    let roster = roster::read_peer(&content.state, &ctx, &target.device_id)?
                        .ok_or_else(|| anyhow!("agent_roster_not_synced"))?;
                    peers.insert(target.device_id.clone(), roster.projects);
                }
                peers[&target.device_id]
                    .iter()
                    .find(|p| p.project_id == target.project_id)
                    .and_then(|p| p.agents.iter().find(|a| a.agent_id == target.agent_id))
                    .map(|a| a.name.clone())
            };
            let name = name.unwrap_or_else(|| target.agent_id.clone());
            result.push((target, name));
        }
        Ok(Self {
            targets: result,
            membership: ctx.membership.map(|m| (m.group_id, m.membership_id)),
        })
    }

    /// Runs before any thread/post creation, with the publication/deletion lock
    /// held. Potentially large directory reads happened before taking the lock.
    pub(super) fn finalize(
        self,
        content: &sync::ContentStore,
        lock: &BbsWriteLock,
        thread: &str,
        new_root: bool,
        body: String,
    ) -> Result<(Vec<Mention>, String)> {
        if self.targets.is_empty() {
            return Ok((Vec::new(), body));
        }
        let current = roster::context(&content.state)?;
        let has_remote = self.targets.iter().any(|(t, _)| {
            t.device_id != "local" && current.device_id.as_deref() != Some(&t.device_id)
        });
        let shared_here = if !new_root && has_remote {
            let state = content.state_locked(lock)?;
            current.membership.as_ref().is_some_and(|member| {
                state
                    .groups
                    .get(&member.group_id)
                    .is_some_and(|g| g.shared_threads.contains(thread))
            })
        } else {
            false
        };
        let mut seen = BTreeSet::new();
        let mut normalized = Vec::new();
        let mut names = Vec::new();
        for (mut target, name) in self.targets {
            if target.device_id == "local" {
                target.device_id = current.device_id.clone().unwrap_or_else(|| "local".into());
            }
            let remote = target.device_id != "local"
                && current.device_id.as_deref() != Some(&target.device_id);
            if remote {
                let member = current.membership.as_ref().ok_or_else(|| {
                    anyhow!(if new_root {
                        UNJOINED_NEW_ERROR
                    } else {
                        PRIVATE_REPLY_ERROR
                    })
                })?;
                if self.membership.as_ref()
                    != Some(&(member.group_id.clone(), member.membership_id.clone()))
                {
                    bail!("group_context_changed");
                }
                if !current
                    .members
                    .iter()
                    .any(|m| m.device_id == target.device_id)
                {
                    bail!("mention_target_unavailable");
                }
                if !new_root && !shared_here {
                    bail!("{PRIVATE_REPLY_ERROR}");
                }
            }
            if seen.insert(target.clone()) {
                normalized.push(target);
                names.push(name);
            }
        }
        // Match the existing human @ prefix, but all callers share one helper.
        let prefix = names
            .iter()
            .map(|name| format!("@{name}"))
            .collect::<Vec<_>>()
            .join(" ");
        Ok((normalized, format!("{prefix}\n\n{}", body.trim())))
    }
}

#[cfg(test)]
mod tests;
