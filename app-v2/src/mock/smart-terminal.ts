/** Mock scrollback + NL translator for the Smart Terminal.
 *
 *  Real behaviour lands with M4 (portable-pty for the shell + a Pi-
 *  bridged CLI call for the NL translator). For the chrome-only pass
 *  we fake both:
 *   - `INITIAL_SCROLLBACK` seeds the expanded panel so it doesn't look
 *     empty.
 *   - `mockRunCommand()` appends a few plausible output lines for any
 *     plain-text input; scripted responses for common commands.
 *   - `mockTranslateNL()` returns a canned bash command for recognised
 *     NL prompts, else the selected Magi provider escape hatch.         */

import type { MagiProvider, ScrollbackLine, TranslateResult } from '../types/smart-terminal';

export const INITIAL_SCROLLBACK: ScrollbackLine[] = [
  { kind: 'dim',    text: 'Last login: Thu Apr 23 14:22:08 on ttys003' },
  { kind: 'dim',    text: 'kota shell · click Smart command for help writing commands' },
];

/** Fake execution — not a real PTY. Returns the new lines to append
 *  (including the user's echoed `$ cmd` prompt line). Scripts a
 *  handful of common commands to give the panel believable output.  */
export function mockRunCommand(cwd: string, cmd: string): ScrollbackLine[] {
  const out: ScrollbackLine[] = [
    { kind: 'prompt', text: `${cwd} › ${cmd}` },
  ];
  const trimmed = cmd.trim();
  if (!trimmed) return out;

  // A few scripted responses so the mock shell feels alive enough to
  // demo. Anything unmatched gets a generic "1-liner" reply.
  if (/^git status$/.test(trimmed)) {
    out.push({ kind: 'dim', text: 'On branch main' });
    out.push({ kind: 'dim', text: "Your branch is up to date with 'origin/main'." });
    out.push({ kind: 'dim', text: 'nothing to commit, working tree clean' });
  } else if (/^ls$/.test(trimmed)) {
    out.push({ kind: 'file', text: 'Documents   Downloads   Projects   Desktop' });
  } else if (/^pwd$/.test(trimmed)) {
    out.push({ kind: 'path', text: cwd.replace(/^~/, '/Users/example') });
  } else if (/^clear$/.test(trimmed)) {
    return []; // caller handles clearing the buffer
  } else if (/^exit$/.test(trimmed)) {
    out.push({ kind: 'dim', text: 'logout' });
  } else if (/^claude(\s|$)/.test(trimmed) || /^codex(\s|$)/.test(trimmed)) {
    // Agent TUI launch — handled upstream via shellStatus, but show
    // a banner line so the scrollback reflects the transition. Flagged
    // forms (e.g. `claude --dangerously-skip-permissions`) used by the
    // #ask handoff fall through here too.
    if (/^codex(\s|$)/.test(trimmed)) {
      out.push({ kind: 'ai', text: 'Codex CLI · mock' });
      out.push({ kind: 'dim', text: `working in ${cwd} · model: gpt-5.5` });
    } else {
      out.push({ kind: 'ai', text: `Claude Code · v1.0.48` });
      out.push({ kind: 'dim', text: `working in ${cwd} · model: sonnet-4.5` });
    }
  } else {
    out.push({ kind: 'dim', text: '  (mock shell — real output lands with M4 PTY)' });
  }
  return out;
}

/** Fake NL translation.
 *
 *  Returns either a single bash command (pre-filled on the prompt row)
 *  or the escape-hatch sentinel when the intent needs multiple steps.
 *  In both cases the CALLER echoes `↳ ask: <original>` into the
 *  scrollback so history preserves what the user asked. */
export function mockTranslateNL(ask: string, provider: MagiProvider = 'claude'): TranslateResult {
  const q = ask.trim().toLowerCase();

  if (/todo/.test(q) && /week/.test(q)) {
    return {
      kind: 'command',
      value: 'grep -rn "TODO" --include="*.ts" --include="*.tsx" . | xargs -I {} sh -c \'git log --since="1 week ago" --oneline -- {} > /dev/null 2>&1 && echo {}\'',
    };
  }
  if (/(^|\s)file(s)? (modified|changed) (in )?last 24/i.test(q) || /24h/.test(q)) {
    return { kind: 'command', value: 'find . -type f -mtime -1 -not -path "*/node_modules/*"' };
  }
  if (/\brefactor\b/.test(q) || /\brewrite\b/.test(q) || q.split(' ').length > 10) {
    // Long multi-step intent — escape-hatch.
    return {
      kind: 'escape',
      value: provider,
      provider,
      hint: 'needs multiple steps',
    };
  }
  if (/disk.*usage|size.*(folder|dir)/.test(q)) {
    return { kind: 'command', value: 'du -sh * | sort -h' };
  }
  if (/process|running.*port/.test(q)) {
    return { kind: 'command', value: 'lsof -i -P -n | grep LISTEN' };
  }
  // Default — echo as a comment so the user sees their intent wasn't
  // parsed and can edit.
  return { kind: 'command', value: `# ${ask}` };
}
