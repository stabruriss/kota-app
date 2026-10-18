//! String's per-agent/Hero model group. No global presets or named routes. The virtual model
//! remains `kota/auto`; a complete group travels to Pi as one JSON flag/command argument.
use super::{
    auth_protocol::THINKING_LEVELS,
    error::{identifier, LocalError, LocalResult},
    jev,
    models::{ModelSelection, ResolvedRecruit},
    owned::OwnedDirectory,
};
use serde::{Deserialize, Serialize};

pub(crate) const PROVIDER: &str = "kota";
pub(crate) const MODEL: &str = "auto";
pub(crate) const MODEL_ARG: &str = "kota/auto";
pub(crate) const THINKING: &str = "off";
pub(crate) const DEFAULT_THRESHOLD: f64 = 0.5;
fn default_threshold() -> f64 {
    DEFAULT_THRESHOLD
}
fn default_model() -> String {
    "model-1".into()
}
pub(crate) fn is_auto(model: &ModelSelection) -> bool {
    model.provider_id == PROVIDER && model.model_id == MODEL
}
/// String and Dance use the same 0.05-step slider; floating point IPC/YAML values are tolerated.
pub(crate) fn valid_threshold(value: f64) -> bool {
    value.is_finite()
        && (0.05..=0.95).contains(&value)
        && (value * 20.0 - (value * 20.0).round()).abs() < 1e-9
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RouteModel {
    pub id: String,
    pub provider_id: String,
    pub model_id: String,
    pub thinking_level: String,
    pub intent: String,
}
impl RouteModel {
    fn same_physical_config(&self, other: &Self) -> bool {
        self.provider_id == other.provider_id
            && self.model_id == other.model_id
            && self.thinking_level == other.thinking_level
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Group {
    pub models: Vec<RouteModel>,
    pub default_model: String,
    pub threshold: f64,
}
impl Group {
    /// Shape only; no Pi IO while profile metadata is locked.
    pub fn validate(&self) -> LocalResult<()> {
        if self.models.len() < 2 {
            return Err(LocalError::new("auto_route_models_required"));
        }
        if self.models.len() > 6
            || !valid_threshold(self.threshold)
            || !self.models.iter().any(|m| m.id == self.default_model)
        {
            return Err(LocalError::new("auto_route_invalid"));
        }
        for (index, model) in self.models.iter().enumerate() {
            if model.id != format!("model-{}", index + 1)
                || model.intent.chars().count() > 200
                || model.intent.contains('\0')
            {
                return Err(LocalError::new("auto_route_invalid"));
            }
            if !identifier(&model.provider_id, 128)
                || model.provider_id.contains('/')
                || !identifier(&model.model_id, 1024)
                || model.provider_id == PROVIDER
            {
                return Err(LocalError::new("model_not_found"));
            }
            if !THINKING_LEVELS.contains(&model.thinking_level.as_str()) {
                return Err(LocalError::new("thinking_invalid"));
            }
        }
        Ok(())
    }
    pub fn launch_json(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

/// YAML keeps provider/model joined, with only the first slash separating them. Old pair
/// keys are deliberately not migrated: no user has received that unreleased format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ShellGroup {
    #[serde(default)]
    pub models: Vec<ShellModel>,
    #[serde(default = "default_model", rename = "default-model")]
    pub default_model: String,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ShellModel {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub effort: String,
    #[serde(default)]
    pub intent: String,
}
impl ShellGroup {
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
    pub fn group(&self) -> Group {
        Group {
            models: self
                .models
                .iter()
                .map(|m| {
                    // Do not drop damaged entries: validation must reject the whole group.
                    let (provider, model) =
                        m.model.split_once('/').unwrap_or(("", m.model.as_str()));
                    RouteModel {
                        id: m.id.clone(),
                        provider_id: provider.into(),
                        model_id: model.into(),
                        thinking_level: m.effort.clone(),
                        intent: m.intent.clone(),
                    }
                })
                .collect(),
            default_model: self.default_model.clone(),
            threshold: self.threshold,
        }
    }
}
impl From<&Group> for ShellGroup {
    fn from(group: &Group) -> Self {
        Self {
            models: group
                .models
                .iter()
                .map(|m| ShellModel {
                    id: m.id.clone(),
                    model: format!("{}/{}", m.provider_id, m.model_id),
                    effort: m.thinking_level.clone(),
                    intent: m.intent.clone(),
                })
                .collect(),
            default_model: group.default_model.clone(),
            threshold: group.threshold,
        }
    }
}

pub(crate) fn gate(owned: &OwnedDirectory, group: Option<&Group>) -> LocalResult<Group> {
    let group = group.ok_or_else(|| LocalError::new("auto_route_models_required"))?;
    group.validate()?;
    jev::gate(owned)?;
    Ok(group.clone())
}
pub(crate) fn choose(
    owned: &OwnedDirectory,
    requested: &Group,
    saved: Option<&Group>,
    resolve: impl FnMut(&ModelSelection, &str) -> LocalResult<ResolvedRecruit>,
) -> LocalResult<ResolvedRecruit> {
    gate(owned, Some(requested))?;
    let group = confirm(requested, saved, resolve)?;
    resolved(owned, Some(&group))
}
pub(crate) fn resolved(
    owned: &OwnedDirectory,
    group: Option<&Group>,
) -> LocalResult<ResolvedRecruit> {
    let group = gate(owned, group)?;
    Ok(ResolvedRecruit {
        schema: 1,
        provider_id: PROVIDER.into(),
        model_id: MODEL.into(),
        model_arg: MODEL_ARG.into(),
        thinking_level: THINKING.into(),
        auto_route: Some(group),
    })
}
/// Reordering or changing intent/default/threshold needs no catalog query. Confirm each new
/// physical configuration once; unchanged configurations keep their already-clamped level.
pub(crate) fn confirm(
    requested: &Group,
    saved: Option<&Group>,
    mut resolve: impl FnMut(&ModelSelection, &str) -> LocalResult<ResolvedRecruit>,
) -> LocalResult<Group> {
    requested.validate()?;
    let mut confirmed: Vec<RouteModel> = Vec::new();
    for model in &requested.models {
        let previous = saved
            .into_iter()
            .flat_map(|s| &s.models)
            .find(|m| model.same_physical_config(m));
        let duplicate = requested
            .models
            .iter()
            .take(confirmed.len())
            .position(|m| model.same_physical_config(m));
        let mut next = model.clone();
        if let Some(previous) = previous {
            next.thinking_level = previous.thinking_level.clone();
        } else if let Some(index) = duplicate {
            next.provider_id = confirmed[index].provider_id.clone();
            next.model_id = confirmed[index].model_id.clone();
            next.thinking_level = confirmed[index].thinking_level.clone();
        } else {
            let resolved = resolve(
                &ModelSelection {
                    provider_id: model.provider_id.clone(),
                    model_id: model.model_id.clone(),
                },
                &model.thinking_level,
            )?;
            next.provider_id = resolved.provider_id;
            next.model_id = resolved.model_id;
            next.thinking_level = resolved.thinking_level;
        }
        confirmed.push(next);
    }
    let group = Group {
        models: confirmed,
        ..requested.clone()
    };
    group.validate()?;
    Ok(group)
}

#[cfg(test)]
pub(crate) fn fixture_group() -> Group {
    Group {
        models: ["reason", "plain"]
            .into_iter()
            .enumerate()
            .map(|(i, m)| RouteModel {
                id: format!("model-{}", i + 1),
                provider_id: "fable".into(),
                model_id: m.into(),
                thinking_level: "off".into(),
                intent: format!("Fixture intent {}", i + 1),
            })
            .collect(),
        default_model: "model-1".into(),
        threshold: DEFAULT_THRESHOLD,
    }
}

#[cfg(test)]
#[path = "auto_route_tests.rs"]
mod tests;
