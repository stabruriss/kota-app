use serde::Serialize;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthInUse {
    pub agent_id: String,
    pub project_root: String,
}
