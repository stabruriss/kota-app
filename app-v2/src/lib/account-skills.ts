import type { AccountSkillDraft } from '../pty-client';

export interface SkillLoomEntry extends AccountSkillDraft {
  selected: boolean;
  missing: boolean;
}

export function selectableAccountSkills(skills: readonly AccountSkillDraft[]): AccountSkillDraft[] {
  return skills.filter((skill) => skill.valid);
}

export function skillLoomEntries(
  catalog: readonly AccountSkillDraft[],
  selectedIds: readonly string[],
): SkillLoomEntry[] {
  const selected = new Set(selectedIds);
  const entries = selectableAccountSkills(catalog).map((skill) => ({
    ...skill,
    selected: selected.has(skill.id),
    missing: false,
  }));
  const catalogIds = new Set(entries.map((skill) => skill.id));
  for (const id of selectedIds) {
    if (catalogIds.has(id)) continue;
    entries.push({
      id,
      name: id,
      description: 'Selected in SHELL.yaml but missing from $KOTA_HOME/skills.',
      path: '',
      kind: 'missing',
      bundledDefault: false,
      valid: false,
      createdAt: '',
      error: 'Missing from $KOTA_HOME/skills',
      selected: true,
      missing: true,
    });
  }
  return entries;
}
