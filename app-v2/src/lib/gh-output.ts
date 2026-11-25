// What the GitHub sign-in shows, worked out from everything gh has printed so far. Shapes only,
// never wording. No lookbehind: Kota runs on macOS 13.0, whose WebKit cannot parse one.
const ANSI = /\u001b\[[0-9;?]*[ -/]*[@-~]/g;
const CODE = /(?:^|[^A-Za-z0-9-])([A-Z0-9]{4}-[A-Z0-9]{4})(?![A-Za-z0-9-])/g;
const linesOf = (output: string) => output.replace(ANSI, '').split(/\r?\n/).map(line => line.trimEnd()).filter(Boolean);
const codesIn = (line: string) => [...line.matchAll(CODE)].map(match => match[1]);

export const plainOutput = (output: string) => linesOf(output).join('\n');

/** The one-time code and the line it stands in; null unless exactly one code is there. */
export function readCode(output: string): { code: string; line: string } | null {
  const found = new Map<string, string>();
  for (const line of linesOf(output)) for (const code of codesIn(line)) if (!found.has(code)) found.set(code, line.trim());
  if (found.size !== 1) return null;
  const [[code, line]] = found;
  return { code, line };
}

/** Why gh stopped: what it printed after the code and the address, which are no use any more.
 *  All of it when that leaves nothing. */
export function readReason(output: string): string {
  const lines = linesOf(output);
  const fresh = lines.findIndex((_, index) => !lines.slice(index).some(line => line.includes('/login/device') || codesIn(line).length));
  return (fresh < 0 ? lines : lines.slice(fresh)).join('\n');
}
