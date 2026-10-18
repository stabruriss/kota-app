use super::*;
use serde_json::json;

fn request() -> ProjectAgentSaveRequest {
    serde_json::from_value(json!({"agentId":"agent-config","projectRoot":"/fixture","displayName":"Fixture",
        "provider":"kota-local","model":"fable/reason","effort":"high","ghost":"Draft ghost","shellRevision":"opening-revision"})).unwrap()
}

#[test]
fn detail_selection_pairs_model_and_effort_and_carries_the_opening_revision() {
    let shell = parse_shell_yaml("provider: kota-local\nmodel: fable/plain\neffort: off\n").unwrap();
    let value = local_detail_selection_request(&shell, &request()).unwrap().unwrap();
    assert_eq!(value, json!({"type":"set_model","provider":"fable","modelId":"reason","level":"high",
        "autoRoute":null,"shellRevision":"opening-revision"}));
}

#[test]
fn detail_identity_saves_and_other_provider_changes_do_not_configure_a_local_session() {
    let shell = parse_shell_yaml("provider: kota-local\nmodel: fable/reason\neffort: high\n").unwrap();
    assert!(local_detail_selection_request(&shell, &request()).unwrap().is_none());
    let mut request = request(); request.provider = Some("codex".into()); request.model = "default".into();
    assert!(local_detail_selection_request(&shell, &request).unwrap().is_none());
    request.provider = Some("kota-local".into());
    let terminal = parse_shell_yaml("provider: codex\nmodel: default\n").unwrap();
    assert!(local_detail_selection_request(&terminal, &request).unwrap().is_none());
}

#[test]
fn legacy_default_identity_saves_keep_the_original_worker_path() {
    let shell = parse_shell_yaml("provider: kota-local\n").unwrap();
    let mut request = request(); request.model = "default".into(); request.effort = None;
    assert!(local_detail_selection_request(&shell, &request).unwrap().is_none());
    request.model = "fable/plain".into(); request.effort = Some("unsupported".into());
    assert_eq!(local_detail_selection_request(&shell, &request).unwrap_err(), "thinking_invalid");
}

#[test]
fn detail_same_auto_model_group_edits_still_send_the_complete_group() {
    let mut request = request(); request.model = "kota/auto".into(); request.effort = Some("off".into());
    let group = local_rpc::auto_route::fixture_group();
    request.auto_route = Some(group.clone());
    let mut shell = parse_shell_yaml("provider: kota-local\nmodel: kota/auto\neffort: off\n").unwrap();
    shell.auto_route = Some(local_rpc::auto_route::ShellGroup::from(&group));
    assert!(local_detail_selection_request(&shell, &request).unwrap().is_none());
    request.auto_route.as_mut().unwrap().threshold = 0.9;
    request.auto_route.as_mut().unwrap().models[0].intent = "Updated next-input routing intent".into();
    let value = local_detail_selection_request(&shell, &request).unwrap().unwrap();
    assert_eq!(value["provider"], "kota"); assert_eq!(value["modelId"], "auto");
    assert_eq!(value["autoRoute"], json!(request.auto_route));
    assert_eq!(value["shellRevision"], "opening-revision");
}
