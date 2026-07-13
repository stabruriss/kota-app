/**
 * Agent title registry — the easter-egg "first name prefix" system.
 *
 * Each title has a full form (shown in the config / profile overlay) and
 * an abbreviation (shown in pills, terminal headers, anywhere space is
 * tight). Title is persisted by encoding its abbr at the front of the
 * displayName string, e.g. `"Dr. Aria-Bunshin v. Kota"`. This keeps the
 * Rust backend untouched — displayName remains the single source of truth.
 *
 * Parsing is greedy-by-length so that overlapping prefixes (`God` vs
 * `Goddess`) resolve correctly.
 */

export interface TitleDef {
  /** Stable id used for CSS class (`title-{id}`). */
  id: string;
  /** Long form shown in config / profile overlay. */
  full: string;
  /** Short form shown in pills, terminals — also the persistence prefix. */
  abbr: string;
  /** Legacy or alternate prefixes accepted when parsing saved display names. */
  aliases?: readonly string[];
  /** Loose category, used only by the picker for visual grouping. */
  category: 'academic' | 'knightly' | 'nobility' | 'royal' | 'spiritual' | 'divine' | 'programmer';
}

/**
 * Order matters for the picker dropdown (academic → knightly → nobility →
 * royal → spiritual → divine → programmer). Keep the entries here in the
 * order you want users to see them.
 */
export const TITLE_DEFS: readonly TitleDef[] = [
  { id: 'doctor',  full: 'Doctor',             abbr: 'Dr.',     category: 'academic' },
  { id: 'sir',     full: 'Sir',                abbr: 'Sir',     category: 'knightly' },
  { id: 'knight',  full: 'Knight',             abbr: 'Kn.',     category: 'knightly' },
  { id: 'lord',    full: 'Lord',               abbr: 'Lord',    category: 'nobility' },
  { id: 'lady',    full: 'Lady',               abbr: 'Lady',    category: 'nobility' },
  { id: 'hm',      full: 'Her Majesty',        abbr: 'Her M.',  aliases: ['H.M.'], category: 'royal' },
  { id: 'his-hm',  full: 'His Majesty',        abbr: 'His M.',  category: 'royal' },
  { id: 'hh',      full: 'Her Highness',       abbr: 'Her H.',  aliases: ['H.H.'], category: 'royal' },
  { id: 'his-hh',  full: 'His Highness',       abbr: 'His H.',  category: 'royal' },
  { id: 'he',      full: 'Her Excellency',     abbr: 'Her E.',  aliases: ['H.E.'], category: 'royal' },
  { id: 'his-he',  full: 'His Excellency',     abbr: 'His E.',  category: 'royal' },
  { id: 'sage',    full: 'Sage',               abbr: 'Sage',    category: 'spiritual' },
  { id: 'god',     full: 'God',                abbr: 'God',     category: 'divine' },
  { id: 'goddess', full: 'Goddess',            abbr: 'Goddess', category: 'divine' },
  { id: 'angel',   full: 'Angel',              abbr: 'Angel',   category: 'divine' },
  { id: 'buddha',  full: 'Buddha',             abbr: 'Buddha',  category: 'spiritual' },
  { id: 'junior',  full: 'Junior',             abbr: 'Jr.',     category: 'programmer' },
  { id: 'senior',  full: 'Senior',             abbr: 'Sr.',     category: 'programmer' },
  { id: '3x',      full: '3x',                 abbr: '3x',      category: 'programmer' },
  { id: '10x',     full: '10x',                abbr: '10x',     category: 'programmer' },
];

const TITLE_BY_ID = new Map(TITLE_DEFS.map((def) => [def.id, def]));
const TITLE_BY_ABBR = new Map<string, TitleDef>();
for (const def of TITLE_DEFS) {
  if (!TITLE_BY_ABBR.has(def.abbr)) TITLE_BY_ABBR.set(def.abbr, def);
  for (const alias of def.aliases ?? []) {
    if (!TITLE_BY_ABBR.has(alias)) TITLE_BY_ABBR.set(alias, def);
  }
}

/** Sorted longest-first so 'Goddess' wins over 'God' during prefix matching. */
const TITLES_BY_LONGEST_ABBR = TITLE_DEFS.flatMap((def) => [
  { def, abbr: def.abbr },
  ...(def.aliases ?? []).map((abbr) => ({ def, abbr })),
]).sort((a, b) => b.abbr.length - a.abbr.length);

export function getTitleDef(id: string | null | undefined): TitleDef | null {
  if (!id) return null;
  return TITLE_BY_ID.get(id) ?? null;
}

export function getTitleDefByAbbr(abbr: string | null | undefined): TitleDef | null {
  if (!abbr) return null;
  return TITLE_BY_ABBR.get(abbr) ?? null;
}

/**
 * Pull a title prefix off the front of a name string.
 *
 * @returns title def + the remainder of the string (with the title and the
 *          single separating space stripped). If no known title prefix is
 *          present, returns `{ title: null, rest: name }`.
 */
export function extractTitle(name: string): { title: TitleDef | null; rest: string } {
  const input = (name ?? '').replace(/^\s+/, '');
  for (const entry of TITLES_BY_LONGEST_ABBR) {
    if (input.startsWith(entry.abbr + ' ')) {
      return { title: entry.def, rest: input.slice(entry.abbr.length + 1) };
    }
  }
  return { title: null, rest: input };
}

/**
 * Apply (or remove) a title on a displayName.
 *
 * Idempotent: if the same title is already present it is left in place.
 * Replaces any existing title when a different one is passed. Pass null to
 * strip the title entirely.
 */
export function applyTitle(name: string, titleId: string | null): string {
  const { rest } = extractTitle(name);
  if (!titleId) return rest;
  const def = getTitleDef(titleId);
  if (!def) return rest;
  return `${def.abbr} ${rest}`;
}
