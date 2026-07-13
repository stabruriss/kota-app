---
load_policy: on-demand
task_trigger: coding, debugging, refactoring, testing, reviewing, implementing, or modifying code
source: https://github.com/multica-ai/andrej-karpathy-skills/blob/main/CLAUDE.md
---

# Rules For Coding

Coding taste adapted from Andrej Karpathy's public coding guidance.

- Optimize for simple, boring, maintainable code that solves the actual request.
- Keep the change narrow. Avoid speculative abstractions and unrelated cleanup.
- Inspect the existing code first, then follow its local patterns.
- Preserve user or teammate edits. Re-read files before patching when the worktree may be dirty.
- Define a concrete done condition and verify it with the most relevant test or build.
- When tradeoffs matter, state them briefly and choose the option that keeps future maintenance cheapest.

Kota-specific execution defaults:

- Do not give time estimates unless the user explicitly asks for scheduling.
- Do not choose temporary solutions by default.
- Prefer simple, elegant, long-term maintainable solutions.
- Keep UI changes aligned with Kota's existing design system.
- For test builds, use the app's established debug packaging flow and report what to verify manually.
