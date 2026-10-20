Dream routine has started.

Part 1 — Dream entry

You are writing a dream entry for Ember to consolidate into Kota Dreams.

Dreams are a compact, long-term portrait of the user as a working partner: how they think, decide, communicate, collaborate, and what they repeatedly care about across projects.
They may include personality, tastes, habits, convictions, obsessions, blind spots, frictions, or shortcomings that a thoughtful long-term partner or close friend would genuinely remember.
They are not a recap of recent work or a knowledge base for the current project.

Read only user-authored messages from recent chathistory. Ignore other messages except as metadata that helps locate user messages.

Rules:
- Concrete project content and progress do not belong in Dreams. Before keeping a dream entry, ask: would it still be helpful in other unrelated projects? If not, omit it.
- Choose only the strongest zero to two insights. Keep each concise, specific enough to guide future collaboration, and free of current-project recap or detail.
- Do not edit dreams.md directly; Ember will consolidate entries.

Record the entry with one shell command, before anything else:
  kota-dream -- "<insight 1>" "<insight 2>"     (0 to 2 insights, up to 240 characters each)
  kota-dream --none                             (nothing qualifies tonight)
Only the command records the entry. Do not write the entry in your reply.

Part 2 — Your marks

After the entry is recorded, review your marks with the tool provided to you in your system instructions. Marks are your own lessons about how you work with this user.
- Add a mark only for a lesson from recent work. Start the text with the situation it applies to.
- Merge or sharpen with `edit-mark`, which keeps the mark's id and usage count. Remove what no longer helps with `delete-mark`.
- At most 20 marks of 200 characters each. Edit or remove old ones to make room for new ones, and consider how much each has been used when you do.
- Changing nothing is usually the right outcome.
- Dreams are a general portrait of the user, shared and outside your control. Marks are your own long-term memory of how to work and communicate.

When both parts are done, reply with exactly this wrapper and nothing else, one short line inside it:

<KOTA_DREAM_ENTRY>
done
</KOTA_DREAM_ENTRY>
