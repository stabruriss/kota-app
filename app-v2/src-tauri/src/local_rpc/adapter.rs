//! The approved Local tool wording, applied only to the generated runtime
//! section. Ghost, rules, identity and all six terminal adapters stay intact.

const TOOL_LINES: [(&str, &str); 11] = [
    ("- CLI, when available: `kota-bbs`", "- Tool: `kota_bbs`."),
    ("When asked to post or reply on the BBS, first run `kota-bbs help`.", "Use `kota_bbs` action `agents` to look up project IDs and mention targets; use `new`, `reply`, or `show` to post and read. Put post text in `body` and attachment paths in `attachments`."),
    ("- Send to a teammate with `kota-agent-bus send --to <AKA-or-agent-id> --intent handoff <<'EOF'`.", "- Send to a teammate with `kota_agent_bus`, using `to` (AKA or agent id), `intent` (`handoff`), and `body`."),
    ("- Put the message body on stdin. Include concrete files, current state, and the requested next action.", "- Put the message text in `body`. Include concrete files, current state, and the requested next action."),
    ("- When asked to sync this project's agent worktrees, run: `kota-bartender sync --json`.", "- When asked to sync this project's agent worktrees, call `kota_bartender` with action `sync`."),
    ("- Create, view, update, or delete project scheduled prompts with `kota-ember`.", "- Create, view, update, or delete project scheduled prompts with `kota_ember`."),
    ("- Prompt bodies are provided on stdin. Targets use agent AKA, agent id, or the configured account human name (`<your-name>` in examples).", "- Put prompt text in `body`. Targets use agent AKA, agent id, or the configured account human name."),
    ("- Before using `kota-ember`, run `kota-ember --help` for the current commands, options, and supported scheduling syntax.", "- Use the tool's `action` and `timing` fields for schedules; use `draft: true` for an unscheduled note."),
    ("- Marks: Run `kota-my-soul add-mark \"<text>\"` when you learn a way of working or communication worth keeping across projects. Start the text with the situation it applies to so Kota may attach them to user messages when needed. You can keep at most 20 marks; `edit-mark`, `delete-mark` and `read-mark` manage them.", "- Marks: Call `kota_my_soul` with action `add-mark` and `text` when you learn a way of working or communication worth keeping across projects. Start the text with the situation it applies to so Kota may attach them to user messages when needed. You can keep at most 20 marks; actions `edit-mark`, `delete-mark` and `read-mark` manage them."),
    ("- Episodes: Run `kota-my-soul search-episode \"<keywords>\"` when information on your past projects is needed. Search returns past room messages, each with the previous message for context; `--id <id>` shows a message in full.", "- Episodes: Call `kota_my_soul` with action `search-episode` and `query` when information on your past projects is needed. Search returns past room messages, each with the previous message for context; action `search-episode` with `id` shows a message in full."),
    ("- `kota-my-soul help` lists the commands.", "- The tool description lists the actions."),
];

pub(crate) fn render(terminal: String) -> Result<String, String> {
    const START: &str = "\n<!-- kota:runtime-context:start -->\n";
    let (head, runtime) = terminal.rsplit_once(START).ok_or("Local adapter is missing its runtime section")?;
    let mut changed = runtime.to_owned();
    for (old, new) in TOOL_LINES {
        // Whole-line matching avoids changing paths or quoted fragments.
        let old = format!("\n{old}\n");
        if changed.matches(&old).count() != 1 {
            return Err("Local adapter tool wording no longer matches the approved template".into());
        }
        changed = changed.replacen(&old, &format!("\n{new}\n"), 1);
    }
    const CLONE_LINE: &str = "- `kota_shadow_clone`: run independent tasks in parallel with clones, such as research or editing separate files.";
    if changed.matches("### System Agents\n").count()!=1 { return Err("Local adapter tool wording no longer matches the approved template".into()); }
    changed = changed.replacen("### System Agents\n", &format!("{CLONE_LINE}\n\n### System Agents\n"), 1);
    Ok(format!("{head}{START}{changed}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_only_approved_lines_even_when_ghost_quotes_the_old_instructions() {
        let baseline = include_str!("fixtures/adapters/pi.md");
        let ghost_quote = "- CLI, when available: `kota-bbs`";
        let input = baseline.replacen("Keep evidence and preserve unrelated work.", ghost_quote, 1);
        let expected = include_str!("fixtures/adapters/kota-local.md")
            .replacen("Keep evidence and preserve unrelated work.", ghost_quote, 1);
        assert_eq!(render(input).unwrap(), expected);
    }
}
