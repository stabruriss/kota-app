//! Narrow UI DTOs. Construction is explicit; reading a snapshot starts no work.
use super::control::{Member, Membership, Role};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub protocol_version: u32,
    pub device: Device,
    pub worker: Worker,
    pub group: Group,
    pub invitation: Invitation,
    pub invitation_generation: Option<String>,
    pub sync: Progress,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Worker {
    pub configured: bool,
    pub can_create_group: bool,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct Group {
    pub id: Option<String>,
    pub name: Option<String>,
    pub role: Option<Role>,
    pub members: Vec<PublicMember>,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicMember {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub online: bool,
    pub public_key: String,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Invitation {
    #[default]
    None,
    Preparing,
    Ready,
    Error,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Idle,
    Connecting,
    Syncing,
    Partial,
    Failed,
}
/// Presentation facts, not permissions or a second sync state machine. Only
/// Manager can classify a failure's source; the UI must not parse error text.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Indicator {
    #[default]
    Healthy,
    Connecting,
    ReachingService,
    ReachingPeers,
    FetchingSession,
    FinishingSync,
    RetryingFiles,
    CheckingProtocol,
    Reconnecting,
    CloudflareLimit,
    UpdateWorker,
    UpdateKota,
    GroupAccessDenied,
    DeviceIdentityError,
    OtherInstance,
    // Reserved for a confirmed file-access cause; generic transport::Io does
    // not carry that evidence and must not produce this value.
    FileAccessError,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: Phase,
    pub indicator: Indicator,
    pub completed: Option<u64>,
    pub total: Option<u64>,
    pub last_successful_at: Option<String>,
    pub error: Option<String>,
    pub control_recoverable: bool,
    pub service_recoverable: bool,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            protocol_version: 1,
            device: Device {
                id: String::new(),
                name: "This device".into(),
            },
            worker: Worker::default(),
            group: Group::default(),
            invitation: Invitation::None,
            invitation_generation: None,
            sync: Progress::default(),
        }
    }
}
impl Status {
    pub(crate) fn membership(&mut self, membership: Option<&Membership>, members: &[Member]) {
        self.group = match membership {
            None => Group::default(),
            Some(m) => Group {
                id: Some(m.group_id.clone()),
                name: members
                    .iter()
                    .find(|p| p.role == Role::Owner)
                    .map(|p| p.name.clone()),
                role: Some(m.role.clone()),
                members: members
                    .iter()
                    .take(32)
                    .map(|p| PublicMember {
                        id: p.device_id.clone(),
                        name: p.name.clone(),
                        role: p.role.clone(),
                        online: p.online,
                        public_key: p.public_key.clone(),
                    })
                    .collect(),
            },
        };
        if membership.is_none_or(|m| m.role != Role::Owner) {
            self.invitation = Invitation::None;
            self.invitation_generation = None;
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Command {
    pub expected_group_id: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvitationRequest {
    pub expected_group_id: Option<String>,
    pub refresh: bool,
}
// No Debug on raw invitation inputs.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JoinRequest {
    pub expected_group_id: Option<String>,
    pub invitation: String,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameRequest {
    pub expected_group_id: Option<String>,
    pub name: String,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoveRequest {
    pub expected_group_id: Option<String>,
    pub device_id: String,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvatarRequest {
    pub sha256: String,
    pub ext: String,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Error {
    pub code: &'static str,
}
impl Error {
    pub(crate) fn from_code(code: &str) -> Self {
        Self {
            code: match code {
                "stale_signature" => "stale_signature",
                "group_context_changed" => "group_context_changed",
                "control_in_use" | "control_busy" | "sync_busy" => "sync_busy",
                "not_joined" => "not_joined",
                "worker_not_paired" => "worker_not_paired",
                "worker_update_required" => "worker_update_required",
                "cloudflare_resource_limit" => "cloudflare_resource_limit",
                "relay_session_lost" => "relay_session_lost",
                "owner_required" => "owner_required",
                _ => "sync_unavailable",
            },
        }
    }
}
pub(crate) fn display_error(code: &str) -> String {
    match code {
        "cloudflare_resource_limit" => {
            "Cloudflare resource limit reached. Retry later; if it continues, ask the group owner to check Cloudflare."
        }
        "relay_session_lost" => {
            "The sync relay session was interrupted. Keep Kota running on the other devices and retry."
        }
        "stale_signature" => {
            "Request expired; check this device’s clock and the other devices’ clocks, then retry."
        }
        "worker_update_required" => {
            "Worker is outdated; update it from the Laughing Man card and retry."
        }
        "incomplete_sync_identity" | "missing_device_identity" => {
            "Device identity is incomplete; report it on GitHub Discussions."
        }
        "control_in_use" => {
            "Another Kota app is using sync; quit it and click Retry."
        }
        "control_busy" | "sync_busy" => "Sync is busy; try again shortly.",
        "worker_unreachable" => "Cannot reach the sync service; check your connection and retry.",
        "removed" | "unauthorized" => {
            "Group access was revoked; ask the owner for a new invitation."
        }
        "sync_timeout" | "sync_connection_closed" => {
            "Connection to another device failed; keep Kota running on the other devices and retry."
        }
        "sync_protocol_error" => {
            "Sync protocol error; retry or report it on GitHub Discussions."
        }
        "protocol_mismatch" => {
            "Sync protocol error; update Kota to the latest version on all devices."
        }
        "sync_integrity_error" => {
            "A received file failed verification; click Retry to download it again."
        }
        "sync_file_io_failed" => {
            "Cannot read or save sync files; check disk space and permissions, then retry."
        }
        _ => "Sync could not finish; retry or report it on GitHub Discussions.",
    }
    .into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Failure {
    pub side: String,
    pub code: String,
    pub thread_id: Option<String>,
    pub post_id: Option<String>,
}
impl Failure {
    pub(crate) fn checked(
        side: &str,
        code: &str,
        thread: Option<&str>,
        post: Option<&str>,
    ) -> Self {
        let code = if !code.is_empty()
            && code.len() <= 80
            && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        {
            code
        } else {
            "sync_item_failed"
        };
        Self {
            side: if side == "peer" { "peer" } else { "local" }.into(),
            code: code.into(),
            thread_id: thread
                .filter(|v| super::safe_id(v).is_ok())
                .map(str::to_owned),
            post_id: post
                .filter(|v| super::safe_id(v).is_ok())
                .map(str::to_owned),
        }
    }
}
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Diagnostics {
    pub protocol_version: u32,
    pub connections: Vec<ConnectionInfo>,
    pub failures: Vec<Failure>,
    pub omitted_failures: u64,
    pub last_error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionInfo {
    pub device_id: String,
    pub state: String,
    pub local_candidate_type: Option<String>,
    pub remote_candidate_type: Option<String>,
    pub basis: Option<String>,
    pub remote_address: Option<String>,
    pub sent_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indicator_contract_is_closed_and_busy_does_not_imply_another_instance() {
        use Indicator::*;
        let indicators = [Healthy, Connecting, ReachingService, ReachingPeers,
            FetchingSession, FinishingSync, RetryingFiles, CheckingProtocol,
            Reconnecting, CloudflareLimit, UpdateWorker, UpdateKota,
            GroupAccessDenied, DeviceIdentityError, OtherInstance, FileAccessError];
        assert_eq!(serde_json::to_value(indicators).unwrap(), serde_json::json!([
            "healthy", "connecting", "reaching_service", "reaching_peers",
            "fetching_session", "finishing_sync", "retrying_files", "checking_protocol",
            "reconnecting", "cloudflare_limit", "update_worker", "update_kota",
            "group_access_denied", "device_identity_error", "other_instance", "file_access_error"
        ]));
        assert_eq!(serde_json::to_value(Status::default()).unwrap()["sync"]["indicator"], "healthy");
        for code in ["control_busy", "sync_busy"] {
            assert_eq!(Error::from_code(code).code, "sync_busy");
            assert_eq!(display_error(code), "Sync is busy; try again shortly.");
        }
        assert_eq!(Error::from_code("control_in_use").code, "sync_busy");
        assert!(display_error("control_in_use").contains("Another Kota app"));
    }
    #[test]
    fn relay_errors_are_fixed_details_and_never_infer_daily_quota_or_recovery() {
        let codes = ["relay_session_lost", "cloudflare_resource_limit"];
        let errors: Vec<_> = codes.iter().map(|code| Error::from_code(code)).collect();
        assert_eq!(
            serde_json::to_value(&errors).unwrap(),
            serde_json::json!([
            {"code":"relay_session_lost"}, {"code":"cloudflare_resource_limit"}])
        );
        let details = codes.map(|code| display_error(code));
        assert_eq!(details[0], "The sync relay session was interrupted. Keep Kota running on the other devices and retry.");
        assert_eq!(details[1], "Cloudflare resource limit reached. Retry later; if it continues, ask the group owner to check Cloudflare.");
        // The UI reserves a daily-quota detail, but this backend has no sample
        // proof and cannot emit that public action code or its reset promise.
        assert_eq!(
            Error::from_code("cloudflare_quota_exceeded").code,
            "sync_unavailable"
        );
        for detail in details
            .into_iter()
            .chain([display_error("cloudflare_quota_exceeded")])
        {
            assert!(
                !detail.contains("00:00")
                    && !detail.contains("update")
                    && !detail.contains("Sync error:")
            );
        }
        assert!(!Status::default().sync.service_recoverable);
        println!(
            "KOTA_BBS_RELAY_ERROR_FIXTURE={}",
            serde_json::json!({
            "errors":errors,"details":codes.map(|code|display_error(code)),"serviceRecoverable":false})
        );
    }
    #[test]
    fn only_confirmed_version_mismatch_recommends_updating() {
        let details = ["sync_protocol_error", "protocol_mismatch", "sync_timeout", "sync_connection_closed"]
            .map(|code| (code, display_error(code)))
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert!(!details["sync_protocol_error"].contains("update Kota"));
        assert!(details["protocol_mismatch"].contains("update Kota"));
        assert!(!details["sync_timeout"].contains("protocol"));
        println!("KOTA_BBS_PROTOCOL_ERRORS_FIXTURE={}", serde_json::to_string(&details).unwrap());
    }
    #[test]
    fn public_contract_fixtures_have_only_public_fields_and_exact_action_shapes() {
        let initial = Status::default();
        let mut owner = initial.clone();
        owner.device.id = "a".repeat(64);
        owner.worker = Worker {
            configured: true,
            can_create_group: true,
        };
        owner.group = Group {
            id: Some("group-one".into()),
            name: Some("Mac".into()),
            role: Some(Role::Owner),
            members: vec![PublicMember {
                id: owner.device.id.clone(),
                name: "Mac".into(),
                role: Role::Owner,
                online: true,
                public_key: "public-key".into(),
            }],
        };
        owner.invitation = Invitation::Ready;
        owner.invitation_generation = Some("2".into());
        let mut member = owner.clone();
        member.group.role = Some(Role::Member);
        member.invitation = Invitation::None;
        member.invitation_generation = None;
        let mut connecting = member.clone();
        connecting.sync.phase = Phase::Connecting;
        connecting.sync.indicator = Indicator::Connecting;
        let mut partial = member.clone();
        partial.sync = Progress {
            phase: Phase::Partial,
            indicator: Indicator::FinishingSync,
            completed: Some(2),
            total: Some(3),
            last_successful_at: Some("2026-09-12T10:00:00Z".into()),
            error: Some(display_error("sync_file_io_failed")),
            control_recoverable: false,
            service_recoverable: false,
        };
        assert!(!partial.sync.control_recoverable || !partial.sync.service_recoverable);
        let mut clock = member.clone();
        clock.sync.phase = Phase::Failed;
        clock.sync.indicator = Indicator::Connecting;
        clock.sync.error = Some(display_error("stale_signature"));
        use crate::bbs::sync::AvatarView;
        let avatars = [
            AvatarView::Builtin { id: "codex".into() },
            AvatarView::Image {
                sha256: "a".repeat(64),
                ext: "png".into(),
                local_path: Some("/isolated-fixture/bbs/avatars/verified.png".into()),
                available: true,
            },
            AvatarView::Image {
                sha256: "b".repeat(64),
                ext: "webp".into(),
                local_path: None,
                available: false,
            },
            AvatarView::None,
        ];
        let fixtures = serde_json::json!({"statuses":[initial,owner,member,connecting,partial,clock],"avatars":avatars,"unit":(),"invitation":super::super::control::InvitationResult{group_id:"group-one".into(),generation:"2".into(),invitation:format!("kota-bbs://worker.example/join#{}","a".repeat(64))},"error":Error::from_code("stale_signature")});
        let bytes = serde_json::to_string(&fixtures).unwrap();
        assert!(!bytes.contains("privateKey"));
        assert!(!bytes.contains("hasCode"));
        assert!(!bytes.contains("\"signature\":"));
        let action: RemoveRequest = serde_json::from_value(
            serde_json::json!({"expectedGroupId":"group-one","deviceId":"peer"}),
        )
        .unwrap();
        assert_eq!(action.expected_group_id.as_deref(), Some("group-one"));
        assert_eq!(
            Error::from_code("token=SECRET"),
            Error {
                code: "sync_unavailable"
            }
        );
        println!("KOTA_BBS_SYNC_IPC_FIXTURE={bytes}");
    }
}
