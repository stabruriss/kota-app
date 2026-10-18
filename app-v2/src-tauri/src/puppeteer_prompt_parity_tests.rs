use super::*;
// Independent snapshots of the four approved JEV-PROMPTS.md JSON blocks, not product constants.
// Dance latest_reply objects and Rule wording: dance-latest-reply-content-plan-20261010/PLAN.md
// A/B, approved by the user at 2026-10-10 20:27 UTC. JEV-PROMPTS.md itself stays unchanged.
fn approved() -> Vec<Value> {
    serde_json::from_str(include_str!(
        "../../kota-ext/test/fixtures/jev-approved-requests.json"
    ))
    .unwrap()
}
fn assert_dance_assemblies(templates: &Value, user_questions: Option<&[Value]>) {
    for (index, strategy) in [(2, Strategy::Context), (3, Strategy::Rule)] {
        let mut expected = approved().remove(index);
        expected.as_object_mut().unwrap().remove("model");
        if let Some(questions) = user_questions {
            expected["questions"] = questions[index].clone();
        }
        let state = &expected["state"];
        let msg = &state["new_message"];
        let seats = state["roster"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| Seat {
                id: r["id"].as_str().unwrap().into(),
                name: r["name"].as_str().unwrap().into(),
            })
            .collect::<Vec<_>>();
        let mut settings = Settings {
            strategy: strategy.clone(),
            ..Settings::default()
        };
        if strategy == Strategy::Rule {
            settings.conditions = state["roster"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    (
                        r["id"].as_str().unwrap().into(),
                        r["condition"].as_str().unwrap().into(),
                    )
                })
                .collect();
        }
        let request = RouteRequest {
            message_id: "approved-fixture".into(),
            timestamp: msg["at"].as_str().unwrap().into(),
            payload: msg["text"].as_str().unwrap().into(),
            text: None,
            attachments: vec![],
            links: vec![],
            paths_mentioned: vec![],
            // Raw room history is independent of the outgoing Jev projection. Rule
            // receives the same input, without sending every agent's reply/history.
            roster: serde_json::from_str(include_str!(
                "../../kota-ext/test/fixtures/dance-roster-history.json"
            ))
            .unwrap(),
        };
        assert_eq!(
            super::super::request_body(&request, &settings, &seats, templates),
            expected
        );
    }
}
#[test]
fn both_actual_dance_assemblies_match_the_approved_full_states_and_questions_verbatim() {
    assert_dance_assemblies(prompts(), None);
}
#[test]
fn valid_user_file_assembles_the_same_independent_questions_fixture_as_string() {
    let templates: Value = serde_json::from_str(include_str!(
        "../../kota-ext/test/fixtures/puppeteer-user-prompts.json"
    ))
    .unwrap();
    let questions: Vec<Value> = serde_json::from_str(include_str!(
        "../../kota-ext/test/fixtures/puppeteer-user-questions.json"
    ))
    .unwrap();
    assert!(crate::puppeteer_prompts::valid(&templates));
    assert_dance_assemblies(&templates, Some(&questions));
}
#[test]
fn compatible_questions_only_ipc_returns_four_effective_templates_without_metadata() {
    if std::env::var_os("KOTA_DANCE_PARITY_CHILD").is_none() {
        let root = project("prompt-parity-account");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "puppeteer::tests::prompt_parity::compatible_questions_only_ipc_returns_four_effective_templates_without_metadata", "--test-threads=1"])
            .env("HOME", &root).env("KOTA_HOME", root.join("Kota"))
            .env("KOTA_DANCE_PARITY_CHILD", "1").output().unwrap();
        fs::remove_dir_all(root).unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    let templates = puppeteer_jev_prompts();
    assert_eq!(templates.as_object().unwrap().len(), 4);
    for key in ["stringFirst", "stringLater", "danceContext", "danceRule"] {
        assert_eq!(templates[key], prompts()[key]);
    }
}
