//! Fixed upstream-source regressions, independently of the Rust allowlist.
//! The package pin must be updated deliberately with both parsers on upgrade.
use super::{
    bundle::PI_VERSION,
    credentials::{key_syntax, KeySyntax},
    environment::MODEL_KEY_NAMES,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, process::Command};
const SOURCE: &str = include_str!("fixtures/pi-auth-1.3.0/SOURCE.json");
const ENV: &str = include_str!("fixtures/pi-auth-1.3.0/env-api-keys.js");
const PARSER: &str = include_str!("fixtures/pi-auth-1.3.0/resolve-config-value.js");

#[test]
fn the_key_list_is_derived_from_pinned_pi_source_and_an_upgrade_requires_review() {
    let source: Value = serde_json::from_str(SOURCE).unwrap();
    let provenance: Value = serde_json::from_str(include_str!(
        "../../../vendor/pi-runtime/kota-pi-1.3.0-darwin-arm64-r1.provenance.json"
    ))
    .unwrap();
    assert_eq!(
        source["piVersion"], PI_VERSION,
        "Pi upgrade: review auth source and model-key filter together"
    );
    assert_eq!(
        source["sourceCommit"],
        provenance["origin"]["source_commit"]
    );
    for (name, text) in [
        ("env-api-keys.js", ENV),
        ("resolve-config-value.js", PARSER),
    ] {
        assert_eq!(
            format!("{:x}", Sha256::digest(text.as_bytes())),
            source["files"][name]["sha256"]
        );
    }
    let function = ENV
        .split("function getApiKeyEnvVars(provider) {")
        .nth(1)
        .unwrap()
        .split("export function findEnvKeys")
        .next()
        .unwrap();
    let mut expected: BTreeSet<String> = function
        .split('"')
        .enumerate()
        .filter(|(i, s)| {
            i % 2 == 1
                && !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        })
        .map(|(_, s)| s.to_owned())
        .collect();
    for declaration in ENV.lines().filter_map(|s| s.strip_prefix("export const ")) {
        let Some((name, value)) = declaration.split_once(" = ") else {
            continue;
        };
        if function.contains(name) {
            expected.insert(value.trim_end_matches(';').trim_matches('"').into());
        }
    }
    assert_eq!(expected.len(), 38);
    assert!(ENV.contains("getProviderEnvValue(\"AWS_BEARER_TOKEN_BEDROCK\", env)"));
    expected.insert("AWS_BEARER_TOKEN_BEDROCK".into());
    assert_eq!(
        MODEL_KEY_NAMES
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>(),
        expected
    );
}

#[test]
fn rust_key_classification_matches_the_upstream_pure_tokenizer_without_evaluation() {
    let root = std::env::temp_dir().join(format!("local-pin-tokenizer-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let values = [
        "literal",
        "!never-executed",
        " !not-trimmed",
        "$",
        "$$VAR",
        "$!VAR",
        "$$$VAR",
        "$VAR",
        "part-${VAR}-tail",
        "${9BAD}",
        "${bad-name}",
        "${$INNER}",
        "${unfinished-$VALID",
        "${_A9}",
        "$_",
        "$9",
        "中文💭",
    ];
    // Extract only pure parser functions, not imports, environment resolution,
    // execSync/spawnSync, or any credential/store code.
    let pure = PARSER
        .split("const ENV_VAR_NAME_RE")
        .nth(1)
        .unwrap()
        .split("function resolveEnvConfigValue")
        .next()
        .unwrap();
    let script=format!("const ENV_VAR_NAME_RE{}\nconst values={};\nprocess.stdout.write(JSON.stringify(values.map(v => {{const r=parseConfigValueReference(v);return r.type==='command'?'Command':r.parts.some(p=>p.type==='env')?'Environment':'Literal';}})));",pure,json!(values));
    let path = root.join("pure-tokenizer.mjs");
    fs::write(&path, script).unwrap();
    let result = Command::new("node")
        .arg(&path)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(&root)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(root);
    assert!(result.status.success(), "pure source parser failed");
    let kinds: Vec<String> = serde_json::from_slice(&result.stdout).unwrap();
    for (value, kind) in values.iter().zip(kinds) {
        let native = match kind.as_str() {
            "Command" => KeySyntax::Command,
            "Environment" => KeySyntax::Environment,
            "Literal" => KeySyntax::Literal,
            _ => panic!("unknown native classification"),
        };
        assert_eq!(key_syntax(value), native);
    }
}
