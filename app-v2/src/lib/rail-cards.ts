/** Order and piles of the system cards in the right column. A pile lists its top card first; a single card is a pile of one. */

export const RAIL_CARDS = ['violet', 'bartender', 'ember', 'dance', 'lm', 'bbs'] as const;
export type RailCardId = (typeof RAIL_CARDS)[number];
export type RailPiles = readonly (readonly RailCardId[])[];

const STORAGE_KEY = 'kota-v2.right-column.card-piles';

function savedPiles(raw: string | null): unknown[][] {
  if (raw === null) return [];
  try {
    const value: unknown = JSON.parse(raw);
    if (!Array.isArray(value) || !value.every(Array.isArray)) return [];
    const ids = value.flat();
    return new Set(ids).size === ids.length ? value : [];
  } catch {
    return [];
  }
}

/** Anything unreadable counts as never saved. Unknown cards are dropped, and cards the saved layout does not know go last. */
export function restoreRailPiles(raw: string | null): RailPiles {
  const piles = savedPiles(raw)
    .map((pile) => pile.filter((id): id is RailCardId => RAIL_CARDS.includes(id as RailCardId)))
    .filter((pile) => pile.length > 0);
  const placed = piles.flat();
  return [...piles, ...RAIL_CARDS.filter((id) => !placed.includes(id)).map((id) => [id])];
}

export function loadRailPiles(): RailPiles {
  try {
    return restoreRailPiles(window.localStorage.getItem(STORAGE_KEY));
  } catch {
    return restoreRailPiles(null);
  }
}

export function saveRailPiles(piles: RailPiles): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(piles));
  } catch {
    // Ignore storage failures; the layout still holds for the current run.
  }
}

/** What is on screen: cards that are not rendered right now keep their place in `piles` and are skipped here. */
export function visiblePiles(piles: RailPiles, present: readonly RailCardId[]): RailPiles {
  return piles.map((pile) => pile.filter((id) => present.includes(id))).filter((pile) => pile.length > 0);
}

export function samePiles(a: RailPiles, b: RailPiles): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

function pileWith(piles: RailPiles, id: RailCardId): readonly RailCardId[] {
  return piles.find((pile) => pile.includes(id)) ?? [];
}

/** The top card goes to the bottom of its pile. */
export function flipPile(piles: RailPiles, top: RailCardId): RailPiles {
  return piles.map((pile) => (pile.includes(top) ? [...pile.filter((id) => id !== top), top] : pile));
}

/** The top card leaves its pile and becomes a single card right below it. */
export function dealCard(piles: RailPiles, top: RailCardId): RailPiles {
  return piles.flatMap((pile) => (pile.includes(top) ? [pile.filter((id) => id !== top), [top]] : [pile]));
}

/** The pile holding `moving` goes on top of the pile holding `onto`; both keep their own order. */
export function stackPile(piles: RailPiles, moving: RailCardId, onto: RailCardId): RailPiles {
  const from = pileWith(piles, moving);
  return piles.filter((pile) => pile !== from).map((pile) => (pile.includes(onto) ? [...from, ...pile] : pile));
}

/** The pile holding `moving` steps to the other side of the pile holding `past`. */
export function movePile(piles: RailPiles, moving: RailCardId, past: RailCardId, side: 'above' | 'below'): RailPiles {
  const from = pileWith(piles, moving);
  return piles
    .filter((pile) => pile !== from)
    .flatMap((pile) => (!pile.includes(past) ? [pile] : side === 'above' ? [from, pile] : [pile, from]));
}
