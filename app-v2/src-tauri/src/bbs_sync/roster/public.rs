//! The IPC only slices this immutable memory projection. Reading a page does
//! not initialize the directory, an identity, or any background operation.
use super::{DeviceView, DirectoryView, PublicAvatar};
use serde::{Deserialize, Serialize};

pub(crate) const PAGE_BYTES: usize = 16_375;
pub(crate) const PAGE_ITEMS: usize = 64;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Error {
    pub code: &'static str,
}
impl Error {
    pub(crate) fn invalid() -> Self {
        Self {
            code: "invalid_roster_request",
        }
    }
    pub(crate) fn unavailable() -> Self {
        Self {
            code: "roster_unavailable",
        }
    }
    pub(crate) fn changed() -> Self {
        Self {
            code: "roster_changed",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadRequest {
    pub version: Option<String>,
    pub after: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvatarRequest {
    pub device_id: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Row {
    Device {
        device_id: Option<String>,
        name: String,
        local: bool,
        online: bool,
        roster_status: &'static str,
        received_at: Option<String>,
    },
    Project {
        device_id: Option<String>,
        project_id: String,
        name: String,
    },
    Agent {
        device_id: Option<String>,
        project_id: String,
        agent_id: String,
        name: String,
        target_ref: String,
        avatar: PublicAvatar,
    },
}
#[derive(Clone, Debug, Serialize)]
pub struct Page {
    pub version: String,
    pub items: Vec<Row>,
    pub next: Option<String>,
}

pub(crate) fn offset(value: &str) -> Option<usize> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}

pub(crate) struct Snapshot {
    pub(crate) version: String,
    rows: Vec<Row>,
    sizes: Vec<usize>,
}
impl Snapshot {
    pub(crate) fn new(view: DirectoryView) -> anyhow::Result<Self> {
        let mut rows = Vec::new();
        for DeviceView {
            device_id,
            name,
            local,
            online,
            roster_status,
            received_at,
            projects,
        } in view.devices
        {
            rows.push(Row::Device {
                device_id: device_id.clone(),
                name,
                local,
                online,
                roster_status,
                received_at,
            });
            for project in projects.unwrap_or_default() {
                rows.push(Row::Project {
                    device_id: device_id.clone(),
                    project_id: project.project_id.clone(),
                    name: project.name,
                });
                for agent in project.agents {
                    rows.push(Row::Agent {
                        device_id: device_id.clone(),
                        project_id: project.project_id.clone(),
                        agent_id: agent.agent_id,
                        name: agent.name,
                        target_ref: agent.target_ref,
                        avatar: agent.avatar,
                    });
                }
            }
        }
        // Stream into the digest; there is no second whole-JSON allocation.
        let version = super::json_hash(&rows)?;
        let sizes = rows
            .iter()
            .map(|row| serde_json::to_vec(row).map(|b| b.len()))
            .collect::<Result<Vec<_>, _>>()?;
        // Reserve the largest possible offset plus the actual response wrapper.
        let shell = Page {
            version: version.clone(),
            items: vec![],
            next: Some(usize::MAX.to_string()),
        };
        let overhead = serde_json::to_vec(&shell)?.len();
        if sizes.iter().any(|n| n + overhead > PAGE_BYTES) {
            anyhow::bail!("roster_item_too_large");
        }
        Ok(Self {
            version,
            rows,
            sizes,
        })
    }
    pub(crate) fn page(&self, request: ReadRequest) -> Result<Page, Error> {
        let start = match (request.version, request.after) {
            (None, None) => 0,
            (Some(version), Some(after)) => {
                super::valid_hash(&version).map_err(|_| Error::invalid())?;
                if version != self.version {
                    return Err(Error::changed());
                }
                offset(&after)
                    .filter(|n| *n < self.rows.len())
                    .ok_or_else(Error::invalid)?
            }
            _ => return Err(Error::invalid()),
        };
        let mut page = Page {
            version: self.version.clone(),
            items: Vec::new(),
            next: None,
        };
        let overhead = serde_json::to_vec(&Page {
            version: self.version.clone(),
            items: vec![],
            next: Some(usize::MAX.to_string()),
        })
        .map_err(|_| Error::unavailable())?
        .len();
        let mut used = overhead;
        let mut end = start;
        while end < self.rows.len() && page.items.len() < PAGE_ITEMS {
            let added = self.sizes[end] + usize::from(!page.items.is_empty());
            if used + added > PAGE_BYTES {
                break;
            }
            used += added;
            page.items.push(self.rows[end].clone());
            end += 1;
        }
        if end < self.rows.len() {
            page.next = Some(end.to_string());
        }
        Ok(page)
    }
}
