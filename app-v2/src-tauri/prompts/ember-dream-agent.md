Dream routine has started.

The wrapper below is a one-off format for this dream entry only. After the closing </KOTA_DREAM_ENTRY> tag, resume normal behavior and do not wrap later replies.

You are writing a dream entry for Ember to consolidate into Kota Dreams.

Dreams are not a summary of your own recent work, implementation details, files changed, terminal output, or agent performance.
Dreams are about what Kota has learned about the user in the past: durable user preferences, fun facts, recent user-life context, and open threads that help future agents understand the user.

Read only user-authored messages from recent chathistory. Ignore assistant, agent, tool, commentary, progress, and system messages except as metadata that helps locate user messages.

Rules:
- Use only recent user messages or directly observed user statements.
- Prefer durable facts about the user: preferences, habits, constraints, fun facts, recurring workflows, current priorities, and user-life context.
- Do not include source file paths, code implementation details, build logs, agent names as accomplishments, or a recap of what you did.
- Do not infer emotions, personality traits, or private facts unless the user explicitly said them.
- Keep each bullet short, factual, and useful for future conversations.
- If there are no durable user facts, return an empty dream entry with a single bullet saying no durable user facts were found.
- Do not edit the final dreams.md directly; Ember will consolidate entries.

Final Dreams digest path: {{dreams_path}}
Dreaming agents: {{dreaming_agents}}

Return exactly this wrapper, with nothing before or after it (no preamble, no progress notes, no commentary). Any text outside the wrapper will trigger workflow errors downstream.

<KOTA_DREAM_ENTRY>
- 3 to 8 concise bullets about the user only.
</KOTA_DREAM_ENTRY>
