Dream routine has started.

You are writing a dream entry for Ember to consolidate into Kota Dreams.

Dreams are a compact, long-term portrait of the user as a working partner: how they think, decide, communicate, collaborate, and what they repeatedly care about across projects.
They may include personality, tastes, habits, convictions, obsessions, blind spots, frictions, or shortcomings that a thoughtful long-term partner or close friend would genuinely remember.
They are not a recap of recent work or a knowledge base for the current project.

Read only user-authored messages from recent chathistory. Ignore other messages except as metadata that helps locate user messages.

Rules:
- Concrete project content and progress do not belong in Dreams. Before keeping a dream entry, ask: would it still be helpful in other unrelated projects? If not, omit it.
- Choose only the strongest zero to two insights. Keep each bullet concise, specific enough to guide future collaboration, and free of current-project recap or detail.
- If no insight qualifies, return the exact marker `__KOTA_DREAM_NONE__` instead of a bullet.
- Do not edit the final dreams.md directly; Ember will consolidate entries.

Final Dreams digest path: {{dreams_path}}
Dreaming agents: {{dreaming_agents}}

The wrapper below is a one-off format for this dream entry only. After the closing </KOTA_DREAM_ENTRY> tag, resume normal behavior and do not wrap later replies.
Return exactly this wrapper, with nothing before or after it (no preamble, no progress notes, no commentary). Any text outside the wrapper will trigger workflow errors downstream.

<KOTA_DREAM_ENTRY>
- 0 to 2 concise, cross-project dream entries, or `__KOTA_DREAM_NONE__` when there are zero.
</KOTA_DREAM_ENTRY>
