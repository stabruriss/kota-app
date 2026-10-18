use super::{
    auth::Backend,
    auth_protocol::THINKING_LEVELS,
    credentials::{AuthStore, AuthType, Capabilities},
    error::{identifier, LocalError, LocalResult},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ModelSelection {
    pub provider_id: String,
    pub model_id: String,
}
// Pi 1.3.0 core/defaults.js and core/sdk.js use medium before
// the model's native clamp. Local never consults personal Pi/defaults files.
pub(crate) const INITIAL_THINKING_LEVEL: &str = "medium";
const RETIRED_AZURE_PROVIDER: &str = "azure-openai-responses";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecruitSelection {
    pub schema: u8,
    pub provider_id: String,
    pub model_id: String,
    pub thinking_level: Option<String>,
    #[serde(default)]
    pub auto_route: Option<super::auto_route::Group>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResolvedRecruit {
    pub schema: u8,
    pub provider_id: String,
    pub model_id: String,
    pub model_arg: String,
    pub thinking_level: String,
    /// The agent's own model group when the model is `kota/auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_route: Option<super::auto_route::Group>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeResolved {
    provider_id: String,
    model_id: String,
    requested_thinking_level: String,
    thinking_level: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CatalogModel {
    pub model_id: String,
    pub name: String,
    pub thinking_levels: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeProvider {
    #[serde(flatten)]
    capabilities: Capabilities,
    models: Vec<CatalogModel>,
}
#[derive(Deserialize)]
struct NativeCatalog {
    providers: Vec<NativeProvider>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogProvider {
    pub provider_id: String,
    pub linked: bool,
    pub models: Vec<CatalogModel>,
}
#[derive(Serialize)]
pub(crate) struct Catalog {
    pub schema: u8,
    pub providers: Vec<CatalogProvider>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AvailabilityRow {
    pub provider_id: String,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_type: Option<AuthType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}
#[derive(Serialize)]
pub(crate) struct Availability {
    pub schema: u8,
    pub providers: Vec<AvailabilityRow>,
    pub summary: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LocalError>,
}

pub(crate) fn selected_model(value: Option<&str>) -> LocalResult<ModelSelection> {
    let value = value.map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("default"))
        .ok_or_else(|| LocalError::new("model_required"))?;
    let (provider, id) = value.split_once('/')
        .ok_or_else(|| LocalError::new("model_not_found"))?;
    let model = ModelSelection { provider_id: provider.into(), model_id: id.into() };
    validate_model(&model)?;
    Ok(model)
}

pub(crate) fn validate_saved_selection(model: &str, thinking: Option<&str>) -> LocalResult<()> {
    selected_model(Some(model))?;
    if !thinking.is_some_and(|value| THINKING_LEVELS.contains(&value)) {
        return Err(LocalError::new("thinking_invalid"));
    }
    Ok(())
}

pub(crate) fn saved_thinking(value: Option<&str>) -> &str {
    value.map(str::trim)
        .filter(|value| THINKING_LEVELS.contains(value))
        .unwrap_or(INITIAL_THINKING_LEVEL)
}

fn validate_model(model: &ModelSelection) -> LocalResult<()> {
    // Pi 1.1 renamed the Azure provider, but not its API. Leave old owned
    // credentials/SHELL files intact; selecting the retired ID needs a new
    // explicit model selection and sign-in, never a secret migration.
    if model.provider_id == RETIRED_AZURE_PROVIDER {
        return Err(LocalError::provider("model_not_found", &model.provider_id));
    }
    if !identifier(&model.provider_id, 128)
        || model.provider_id.contains('/')
        || !identifier(&model.model_id, 1024)
    {
        return Err(LocalError::new("model_not_found"));
    }
    Ok(())
}

fn native_catalog(backend: &Backend) -> LocalResult<NativeCatalog> {
    native_catalog_observed(backend, &super::query::Query::standard())
}
fn native_catalog_observed(backend: &Backend, query: &super::query::Query) -> LocalResult<NativeCatalog> {
    let result = super::metadata_cache::query(backend, json!({"operation":"catalog"}), query,
        |result| parse_catalog(result).map(|_| ()))?;
    parse_catalog(&result)
}
fn parse_catalog(result: &serde_json::Value) -> LocalResult<NativeCatalog> {
    let catalog: NativeCatalog = serde_json::from_value(result["catalog"].clone())
        .map_err(|_| LocalError::new("auth_helper_failed"))?;
    if catalog.providers.len() > 256 {
        return Err(LocalError::new("auth_helper_failed"));
    }
    let mut identities = std::collections::BTreeSet::new();
    for provider in &catalog.providers {
        if !identifier(&provider.capabilities.provider_id, 128)
            || !identities.insert(&provider.capabilities.provider_id)
            || provider.models.len() > 10_000
            || provider.models.iter().any(|m| {
                !identifier(&m.model_id, 1024)
                    || m.name.len() > 1024
                    || m.thinking_levels.is_empty()
                    || m.thinking_levels
                        .iter()
                        .any(|s| !THINKING_LEVELS.contains(&s.as_str()))
            })
        {
            return Err(LocalError::new("auth_helper_failed"));
        }
    }
    Ok(catalog)
}
pub(crate) fn capabilities(backend: &Backend) -> LocalResult<Vec<Capabilities>> {
    Ok(native_catalog(backend)?.providers.into_iter().map(|p| p.capabilities).collect())
}
pub(crate) fn availability(backend: &Backend) -> Availability {
    availability_retry(|| availability_once(backend))
}
pub(crate) fn availability_for_app(app: &tauri::AppHandle) -> Availability {
    availability_retry(|| availability_once(&Backend::for_app(app)?))
}
pub(super) fn availability_retry(
    mut attempt: impl FnMut() -> LocalResult<Availability>,
) -> Availability {
    let mut last = None;
    for _ in 0..2 {
        match attempt() {
            Ok(value) => return value,
            Err(error) => {
                let timeout = matches!(error.code.as_str(), "auth_timeout" | "local_config_busy" | "auth_store_busy");
                last = Some(error);
                if timeout {
                    break;
                }
            }
        }
    }
    let error = last.unwrap();
    // A failed global read/probe cannot establish that missing rows are signed
    // out. The UI shows one Retry, rather than inventing an empty auth store.
    Availability {
        schema: 1,
        providers: Vec::new(),
        summary: "error",
        error: Some(error),
    }
}
fn availability_once(backend: &Backend) -> LocalResult<Availability> {
    // A malformed/command-bearing store is never handed to Pi. Its reason is
    // a trustworthy invalid row, rather than guessing that everyone signed out.
    let store = AuthStore::load(&backend.owned)?;
    store.preflight()?;
    let capabilities = capabilities(backend)?;
    let store = AuthStore::load(&backend.owned)?;
    let providers = capabilities
        .iter()
        .filter(|p| p.provider_id != "llama.cpp" && p.provider_id != RETIRED_AZURE_PROVIDER)
        .map(|provider| {
            let id = &provider.provider_id;
            match store.gate(id, &capabilities) {
                Ok(kind) => AvailabilityRow {
                    provider_id: id.clone(),
                    status: "signed_in",
                    auth_type: Some(kind),
                    error_code: None,
                },
                Err(e) => AvailabilityRow {
                    provider_id: id.clone(),
                    status: match e.code.as_str() {
                        "provider_not_signed_in" => "not_signed_in",
                        "auth_handler_unavailable" => "error",
                        _ => "invalid",
                    },
                    auth_type: None,
                    error_code: (e.code != "provider_not_signed_in").then_some(e.code),
                },
            }
        })
        .collect::<Vec<_>>();
    let summary = if providers.iter().any(|p| p.status == "signed_in") {
        "ready"
    } else if providers.iter().any(|p| p.status == "error") {
        "error"
    } else {
        "not_ready"
    };
    Ok(Availability {
        schema: 1,
        providers,
        summary,
        error: None,
    })
}
pub(crate) fn model_catalog(backend: &Backend) -> LocalResult<Catalog> {
    let native = native_catalog(backend)?;
    let capabilities: Vec<_> = native
        .providers
        .iter()
        .map(|p| p.capabilities.clone())
        .collect();
    let store = AuthStore::load(&backend.owned)?;
    store.preflight()?;
    // Catalog is display metadata. Keep unlinked model names so a saved
    // default still has a name after sign-out; gates use fresh owned auth.
    Ok(Catalog {
        schema: 1,
        providers: native
            .providers
            .into_iter()
            .filter(|p| p.capabilities.provider_id != "llama.cpp"
                && p.capabilities.provider_id != RETIRED_AZURE_PROVIDER
                && p.capabilities.provider_id != super::auto_route::PROVIDER)
            .map(|p| CatalogProvider {
                linked: store
                    .gate(&p.capabilities.provider_id, &capabilities)
                    .is_ok(),
                provider_id: p.capabilities.provider_id,
                models: p.models,
            })
            .collect(),
    })
}

pub(crate) fn resolve_model(
    backend: &Backend,
    model: &ModelSelection,
    thinking: &str,
) -> LocalResult<ResolvedRecruit> {
    resolve_model_observed(backend, model, thinking, &super::query::Query::standard())
}
pub(crate) fn resolve_model_observed(
    backend: &Backend, model: &ModelSelection, thinking: &str, query: &super::query::Query,
) -> LocalResult<ResolvedRecruit> {
    validate_model(model)?;
    if !THINKING_LEVELS.contains(&thinking) {
        return Err(LocalError::new("thinking_invalid"));
    }
    query.stage("credential_status")?;
    let before = AuthStore::load(&backend.owned)?;
    before.preflight()?;
    before.shape(&model.provider_id)?;
    let capabilities = native_catalog_observed(backend, query)?.providers.into_iter().map(|p| p.capabilities).collect::<Vec<_>>();
    query.renewed().stage("credential_status")?;
    AuthStore::load(&backend.owned)?.gate(&model.provider_id, &capabilities)?;
    let query = query.renewed();
    query.stage("resolve_model")?;
    let parse = |result: &serde_json::Value| -> LocalResult<NativeResolved> {
        let native: NativeResolved = serde_json::from_value(result["resolved"].clone())
            .map_err(|_| LocalError::new("auth_helper_failed"))?;
        if native.provider_id != model.provider_id || native.model_id != model.model_id
            || native.requested_thinking_level != thinking || !THINKING_LEVELS.contains(&native.thinking_level.as_str()) {
            return Err(LocalError::new("auth_helper_failed"));
        }
        Ok(native)
    };
    let result = super::metadata_cache::query(backend, json!({"operation":"resolve","providerId":model.provider_id,"modelId":model.model_id,"thinkingLevel":thinking}), &query,
        |result| parse(result).map(|_| ()))?;
    let native = parse(&result)?;
    // Another App may have signed out while this helper was doing metadata.
    query.renewed().stage("credential_status")?;
    AuthStore::load(&backend.owned)?.gate(&model.provider_id, &capabilities)?;
    query.renewed().check("resolve_model")?;
    Ok(ResolvedRecruit {
        schema: 1,
        provider_id: native.provider_id,
        model_arg: format!("{}/{}", model.provider_id, model.model_id),
        model_id: native.model_id,
        thinking_level: native.thinking_level,
        auto_route: None,
    })
}

/// Physical models go through Pi; `kota/auto` goes through the Auto Route release condition.
pub(crate) fn resolve_selection(backend: &Backend, model: &ModelSelection, thinking: &str,
    auto_route: Option<&super::auto_route::Group>, query: &super::query::Query) -> LocalResult<ResolvedRecruit> {
    if super::auto_route::is_auto(model) {
        query.stage("credential_status")?;
        return super::auto_route::resolved(&backend.owned, auto_route);
    }
    resolve_model_observed(backend, model, thinking, query)
}

pub(crate) fn gate_resolved(backend: &Backend, model: &ResolvedRecruit, query: &super::query::Query) -> LocalResult<()> {
    query.stage("credential_status")?;
    if model.provider_id == super::auto_route::PROVIDER && model.model_id == super::auto_route::MODEL {
        super::auto_route::gate(&backend.owned, model.auto_route.as_ref())?;
        return query.renewed().check("credential_status");
    }
    validate_model(&ModelSelection { provider_id: model.provider_id.clone(), model_id: model.model_id.clone() })?;
    let caps = native_catalog_observed(backend, query)?.providers.into_iter().map(|p| p.capabilities).collect::<Vec<_>>();
    let store = AuthStore::load(&backend.owned)?;
    store.preflight()?;
    store.gate(&model.provider_id, &caps)?;
    query.renewed().check("credential_status")
}

pub(crate) fn resolve_local_recruit_config(
    backend: &Backend,
    selection: RecruitSelection,
) -> LocalResult<ResolvedRecruit> {
    if selection.schema != 1 {
        return Err(LocalError::new("invalid_request"));
    }
    let model = ModelSelection {
        provider_id: selection.provider_id,
        model_id: selection.model_id,
    };
    if super::auto_route::is_auto(&model) {
        return super::auto_route::resolved(&backend.owned, selection.auto_route.as_ref());
    }
    resolve_model(backend, &model, selection.thinking_level.as_deref().unwrap_or(INITIAL_THINKING_LEVEL))
}
