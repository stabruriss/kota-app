use std::process::Command;

/// Pi 1.3.0 getApiKeyEnvVars + Bedrock bearer token. SDK overrides and system
/// profile/ADC/SSH/git variables are intentionally inherited. Pin regression
/// derives the list from fixed upstream source tied to the vendor pin; a Pi
/// upgrade requires updating that source and reviewing the filter together.
pub(crate) const MODEL_KEY_NAMES: &[&str] = &[
    "AI_GATEWAY_API_KEY",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_OAUTH_TOKEN",
    "ANT_LING_API_KEY",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AZURE_OPENAI_API_KEY",
    "BASETEN_API_KEY",
    "CEREBRAS_API_KEY",
    "CLOUDFLARE_API_KEY",
    "COPILOT_GITHUB_TOKEN",
    "DEEPSEEK_API_KEY",
    "FIREWORKS_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_CLOUD_API_KEY",
    "GROQ_API_KEY",
    "HF_TOKEN",
    "KIMI_API_KEY",
    "META_API_KEY",
    "MINIMAX_API_KEY",
    "MINIMAX_CN_API_KEY",
    "MISTRAL_API_KEY",
    "MOONSHOT_API_KEY",
    "NVIDIA_API_KEY",
    "OPENAI_API_KEY",
    "OPENCODE_API_KEY",
    "OPENROUTER_API_KEY",
    "QWEN_TOKEN_PLAN_API_KEY",
    "QWEN_TOKEN_PLAN_CN_API_KEY",
    "RADIUS_API_KEY",
    "TOGETHER_API_KEY",
    "TYPESAFE_API_KEY",
    "XAI_API_KEY",
    "XIAOMI_API_KEY",
    "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
    "XIAOMI_TOKEN_PLAN_CN_API_KEY",
    "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
    "ZAI_API_KEY",
    "ZAI_CODING_CN_API_KEY",
];
pub(crate) const TERMINAL_ENV_NAMES: &[&str] = &[
    "FORCE_COLOR",
    "CLICOLOR_FORCE",
    "TERM",
    "TERM_PROGRAM",
    "COLORTERM",
];

pub(crate) fn remove_model_keys(command: &mut Command) {
    for name in MODEL_KEY_NAMES {
        command.env_remove(name);
    }
    // Launcher-only values cannot be inherited by ordinary model owners.
    command
        .env_remove("KOTA_LOCAL_AUTH_HELPER")
        .env_remove("KOTA_LOCAL_INSTALLATION_UUID");
}

/// Only ordinary Local agents get the pinned CLI for native `pi mcp` management.
/// The existing PATH entries and all other provider environments stay intact.
pub(crate) fn prepend_bundled_pi(command: &mut Command, executable: &std::path::Path) -> super::error::LocalResult<()> {
    let directory = executable.parent().ok_or_else(|| super::error::LocalError::new("pi_bundle_invalid"))?;
    let existing = command.get_envs().find(|(key,_)| *key == "PATH").and_then(|(_,value)| value).unwrap_or_default();
    let paths = std::iter::once(directory.to_path_buf()).chain(std::env::split_paths(existing));
    let value = std::env::join_paths(paths).map_err(|_| super::error::LocalError::new("local_environment_invalid"))?;
    command.env("PATH",value);
    Ok(())
}

#[cfg(test)]
mod mcp_environment_tests {
    use super::*;
    #[test]
    fn bundled_cli_is_first_and_other_local_environment_entries_are_preserved() {
        let mut command=Command::new("fixture");
        command.env_clear().env("PATH","/existing/bin:/usr/bin").env("HOME","/fixture-home").env("AWS_PROFILE","fixture-profile");
        let before:Vec<_>=command.get_envs().filter(|(k,_)| *k!="PATH").map(|(k,v)|(k.to_owned(),v.map(|s|s.to_owned()))).collect();
        prepend_bundled_pi(&mut command,std::path::Path::new("/bundle/kota-pi/pi")).unwrap();
        assert_eq!(command.get_envs().find(|(k,_)| *k=="PATH").unwrap().1.unwrap(),"/bundle/kota-pi:/existing/bin:/usr/bin");
        let after:Vec<_>=command.get_envs().filter(|(k,_)| *k!="PATH").map(|(k,v)|(k.to_owned(),v.map(|s|s.to_owned()))).collect();
        assert_eq!(before,after);
    }
}

/// Helper processes have no agent identity. Match the existing agent filter
/// without changing that shared function or the Terminal path.
pub(crate) fn helper_environment(command: &mut Command, directory: &std::path::Path, uuid: &str) {
    command.env_clear();
    for (name, value) in std::env::vars_os() {
        let Some(key) = name.to_str() else {
            continue;
        };
        if crate::pty::agent::local_rpc_inherits_environment(key)
            && !TERMINAL_ENV_NAMES.contains(&key)
        {
            command.env(name, value);
        }
    }
    remove_model_keys(command);
    command
        .env("PI_CODING_AGENT_DIR", directory)
        .env("PI_SKIP_VERSION_CHECK", "1")
        .env("KOTA_LOCAL_AUTH_HELPER", "1")
        .env("KOTA_LOCAL_INSTALLATION_UUID", uuid)
        .env("NO_COLOR", "1")
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE");
}
