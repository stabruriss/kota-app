use super::*;
use crate::local_rpc::auth_fixture::Fixture;
use serde_json::json;
fn ready() -> Fixture {
    let f = Fixture::new(json!({}));
    jev::select(&f.backend, "typesafe").unwrap();
    jev::save_key_with(
        &f.backend,
        "fixture",
        "typesafe",
        "fixture-jev-key",
        |_, _, _, _| Ok("verified"),
    )
    .unwrap();
    f
}
fn clamp(model: &ModelSelection, thinking: &str) -> LocalResult<ResolvedRecruit> {
    Ok(ResolvedRecruit {
        schema: 1,
        provider_id: model.provider_id.clone(),
        model_id: model.model_id.clone(),
        model_arg: format!("{}/{}", model.provider_id, model.model_id),
        thinking_level: if thinking == "max" {
            "high".into()
        } else {
            thinking.into()
        },
        auto_route: None,
    })
}
#[test]
fn all_nineteen_threshold_steps_roundtrip_without_changing_the_default() {
    assert_eq!(DEFAULT_THRESHOLD, 0.5);
    let missing: ShellGroup = serde_yaml::from_str("models: []").unwrap();
    assert_eq!(missing.threshold, 0.5);
    for step in 1..=19 {
        let threshold = f64::from(step) / 20.0;
        assert!(valid_threshold(threshold));
        let group = Group { threshold, ..fixture_group() };
        group.validate().unwrap();
        assert_eq!(serde_json::from_str::<Group>(&group.launch_json()).unwrap(), group);
        let yaml = serde_yaml::to_string(&ShellGroup::from(&group)).unwrap();
        assert_eq!(serde_yaml::from_str::<ShellGroup>(&yaml).unwrap().group(), group);
    }
}
#[test]
fn release_requires_two_models_and_a_ready_jev_channel() {
    let f = Fixture::new(json!({}));
    let group = fixture_group();
    let one = Group {
        models: group.models[..1].to_vec(),
        ..group.clone()
    };
    for invalid in [None, Some(&one)] {
        assert_eq!(
            gate(&f.backend.owned, invalid).unwrap_err().code,
            "auto_route_models_required"
        );
    }
    assert_eq!(
        gate(&f.backend.owned, Some(&group)).unwrap_err().code,
        "jev_key_required"
    );
    jev::save_key_with(
        &f.backend,
        "r",
        "openrouter",
        "fixture-jev-key",
        |_, _, _, _| Ok("verified"),
    )
    .unwrap();
    let resolved = resolved(&f.backend.owned, Some(&group)).unwrap();
    assert_eq!(
        (
            resolved.model_arg.as_str(),
            resolved.thinking_level.as_str(),
            resolved.auto_route.as_ref()
        ),
        ("kota/auto", "off", Some(&group))
    );
    jev::mark_rejected(
        &f.backend.owned,
        &jev::current(&f.backend.owned).unwrap().unwrap(),
    )
    .unwrap();
    assert_eq!(
        gate(&f.backend.owned, Some(&group)).unwrap_err().code,
        "jev_key_rejected"
    );
}
#[test]
fn validation_is_whole_and_unicode_bounded() {
    let f = ready();
    let base = fixture_group();
    for (field, value, code) in [
        ("providerId", "kota", "model_not_found"),
        ("providerId", "a/b", "model_not_found"),
        ("modelId", "", "model_not_found"),
        ("thinkingLevel", "huge", "thinking_invalid"),
        ("id", "model-4", "auto_route_invalid"),
        ("intent", &"😀".repeat(201), "auto_route_invalid"),
    ] {
        let mut json = serde_json::to_value(&base).unwrap();
        json["models"][0][field] = value.into();
        let bad: Group = serde_json::from_value(json).unwrap();
        assert_eq!(gate(&f.backend.owned, Some(&bad)).unwrap_err().code, code);
    }
    for threshold in [-0.05, 0.0, 0.025, 0.049, 0.051, 0.49, 0.86, 0.951, 1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            Group {
                threshold,
                ..base.clone()
            }
            .validate()
            .unwrap_err()
            .code,
            "auto_route_invalid"
        );
    }
    assert!(Group {
        default_model: "missing".into(),
        ..base.clone()
    }
    .validate()
    .is_err());
    let mut long = base.clone();
    long.models[0].intent = "😀".repeat(200);
    long.validate().unwrap();
    for count in [2, 3, 4, 5, 6] {
        let mut group = base.clone();
        while group.models.len() < count {
            let mut model = group.models[0].clone();
            model.id = format!("model-{}", group.models.len() + 1);
            group.models.push(model);
        }
        group.validate().unwrap();
    }
    assert!(
        serde_json::from_value::<Group>(json!({"serious":null,"casual":null})).is_err(),
        "no pair migration"
    );
}
#[test]
fn six_models_and_the_sixth_default_roundtrip_but_seven_are_rejected() {
    let mut group = fixture_group();
    group.models = (1..=6)
        .map(|n| RouteModel {
            id: format!("model-{n}"),
            model_id: format!("family/model:{n}"),
            intent: format!("Intent {n}"),
            ..group.models[0].clone()
        })
        .collect();
    group.default_model = "model-6".into();
    group.validate().unwrap();
    assert_eq!(
        serde_json::from_str::<Group>(&group.launch_json()).unwrap(),
        group
    );
    let yaml = serde_yaml::to_string(&ShellGroup::from(&group)).unwrap();
    assert_eq!(
        serde_yaml::from_str::<ShellGroup>(&yaml).unwrap().group(),
        group
    );
    group.models.push(RouteModel {
        id: "model-7".into(),
        ..group.models[0].clone()
    });
    assert_eq!(group.validate().unwrap_err().code, "auto_route_invalid");
}
#[test]
fn confirm_queries_only_new_physical_configs_and_keeps_user_intents() {
    let saved = fixture_group();
    let mut requested = saved.clone();
    requested.models.reverse();
    for (i, m) in requested.models.iter_mut().enumerate() {
        m.id = format!("model-{}", i + 1);
        m.intent = "New intent".into();
    }
    requested.default_model = "model-2".into();
    requested.threshold = 0.7;
    assert_eq!(
        confirm(&requested, Some(&saved), |_, _| panic!(
            "metadata changes need no Pi IO"
        ))
        .unwrap(),
        requested
    );
    requested.models[1].provider_id = "other".into();
    requested.models[1].model_id = "new".into();
    requested.models[1].thinking_level = "max".into();
    let mut calls = 0;
    let confirmed = confirm(&requested, Some(&saved), |m, t| {
        calls += 1;
        clamp(m, t)
    })
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(confirmed.models[1].thinking_level, "high");
    assert_eq!(confirmed.models[1].intent, "New intent");
    let bad = Group {
        models: vec![],
        ..saved
    };
    assert_eq!(
        confirm(&bad, None, |_, _| panic!("gate first"))
            .unwrap_err()
            .code,
        "auto_route_models_required"
    );
}
#[test]
fn duplicate_physical_configs_are_clamped_once() {
    let mut group = fixture_group();
    group.models[0].thinking_level = "max".into();
    group.models[1] = RouteModel {
        id: "model-2".into(),
        ..group.models[0].clone()
    };
    let mut calls = 0;
    let result = confirm(&group, None, |m, t| {
        calls += 1;
        clamp(m, t)
    })
    .unwrap();
    assert_eq!(calls, 1);
    assert!(result.models.iter().all(|m| m.thinking_level == "high"));
}
#[test]
fn shell_roundtrips_native_ids_threshold_default_and_verbatim_intent() {
    let mut group = fixture_group();
    group.models[0].provider_id = "openrouter".into();
    group.models[0].model_id = "anthropic/family/claude:beta".into();
    group.models[0].intent = "A: detail\nwith \"quotes\" 😀".into();
    group.default_model = "model-2".into();
    group.threshold = 0.75;
    let shell = ShellGroup::from(&group);
    assert_eq!(shell.group(), group);
    let yaml = serde_yaml::to_string(&shell).unwrap();
    assert_eq!(
        serde_yaml::from_str::<ShellGroup>(&yaml).unwrap().group(),
        group
    );
    assert_eq!(
        serde_json::from_str::<Group>(&group.launch_json()).unwrap(),
        group
    );
    let old: ShellGroup = serde_yaml::from_str("serious:\n  model: no-slash\n").unwrap();
    assert!(old.is_empty());
    let mut damaged = shell;
    damaged.models[0].model = "no-slash".into();
    assert_eq!(damaged.group().models.len(), 2);
    assert!(damaged.group().validate().is_err());
}
