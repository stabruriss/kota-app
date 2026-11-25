import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';

// Pi renders its system prompt as an untagged preamble followed by <name>…</name> sections joined
// by blank lines. Kota Local leaves the preamble out of every model request: since 2026-11-14
// Antigravity answers a bare 429 to Pi 1.3.0's ("…operating inside pi, a coding agent harness. …")
// whatever quota is left, and a Kota agent's identity comes from its adapter's project context.
// It also leaves out Pi's <docs> section, which points the model at Pi's own README, docs and
// examples: Kota Local prompts carry no harness details. Only the request changes. Pi still
// records all of its own sections in the session transcript, so a replay of the transcript shows
// sections the model was never sent. A prompt of any other shape goes as Pi rendered it.

type Section = { name: string; text: string };

const LEFT_OUT = new Set(['preamble', 'docs']);
const OPENING = /^<([a-z][a-z0-9_-]*)>\n/;

export function promptSections(prompt: string): Section[] | undefined {
  const sections: Section[] = [];
  let at = 0;
  if (!prompt.startsWith('<')) {
    const end = prompt.indexOf('\n\n<');
    if (end < 0) return undefined;
    sections.push({ name: 'preamble', text: prompt.slice(0, end) });
    at = end + 2;
  }
  while (at < prompt.length) {
    const name = OPENING.exec(prompt.slice(at, at + 80))?.[1];
    if (!name) return undefined;
    const closing = `\n</${name}>`;
    const next = prompt.indexOf(`${closing}\n\n<`, at);
    const end = next >= 0 ? next + closing.length : prompt.endsWith(closing) ? prompt.length : -1;
    if (end < 0) return undefined;
    sections.push({ name, text: prompt.slice(at, end) });
    at = end + 2;
  }
  return sections;
}

let unrecognized = false;

export function kotaSystemPrompt(prompt: string): string {
  const sections = promptSections(prompt);
  if (!sections) {
    if (!unrecognized) console.error('[kota-ext] system prompt sections not recognized; sending it as Pi rendered it');
    unrecognized = true;
    return prompt;
  }
  return sections.filter(({ name }) => !LEFT_OUT.has(name)).map(({ text }) => text).join('\n\n');
}

export function registerSystemPrompt(pi: ExtensionAPI) {
  pi.on('before_agent_start', (event) => {
    const systemPrompt = kotaSystemPrompt(event.systemPrompt);
    return systemPrompt === event.systemPrompt ? undefined : { systemPrompt };
  });
}
