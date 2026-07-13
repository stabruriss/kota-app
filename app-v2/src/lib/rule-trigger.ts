/** Keep the YAML frontmatter value single-line while the editor wraps visually. */
export function normalizedRuleTrigger(
  rule: { loadPolicy: string; taskTrigger: string },
): string {
  return rule.loadPolicy === 'on-demand'
    ? rule.taskTrigger.trim().replace(/\s+/g, ' ')
    : '';
}
