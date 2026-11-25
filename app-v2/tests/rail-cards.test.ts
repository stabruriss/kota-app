import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  RAIL_CARDS, dealCard, flipPile, loadRailPiles, movePile, restoreRailPiles, saveRailPiles, stackPile, visiblePiles,
  type RailPiles,
} from '../src/lib/rail-cards';

const single: RailPiles = [['violet'], ['bartender'], ['ember'], ['dance'], ['lm'], ['bbs']];
const piled: RailPiles = [['violet', 'bbs'], ['bartender'], ['ember', 'lm', 'dance']];

describe('rail card piles', () => {
  it('stacks one pile on top of another and keeps both orders', () => {
    expect(stackPile(single, 'bartender', 'ember')).toEqual([['violet'], ['bartender', 'ember'], ['dance'], ['lm'], ['bbs']]);
    expect(stackPile(piled, 'violet', 'ember')).toEqual([['bartender'], ['violet', 'bbs', 'ember', 'lm', 'dance']]);
    expect(stackPile(piled, 'ember', 'violet')).toEqual([['ember', 'lm', 'dance', 'violet', 'bbs'], ['bartender']]);
  });

  it('moves a pile past a neighbour in either direction, as a whole', () => {
    expect(movePile(piled, 'violet', 'bartender', 'below')).toEqual([['bartender'], ['violet', 'bbs'], ['ember', 'lm', 'dance']]);
    expect(movePile(piled, 'ember', 'bartender', 'above')).toEqual([['violet', 'bbs'], ['ember', 'lm', 'dance'], ['bartender']]);
  });

  it('flips the top card to the bottom; as many flips as cards bring the pile round', () => {
    const once = flipPile(piled, 'ember');
    expect(once).toEqual([['violet', 'bbs'], ['bartender'], ['lm', 'dance', 'ember']]);
    expect(flipPile(flipPile(once, 'lm'), 'dance')).toEqual(piled);
  });

  it('deals the top card to a place right below its pile', () => {
    expect(dealCard(piled, 'violet')).toEqual([['bbs'], ['violet'], ['bartender'], ['ember', 'lm', 'dance']]);
    expect(dealCard(piled, 'ember')).toEqual([['violet', 'bbs'], ['bartender'], ['lm', 'dance'], ['ember']]);
  });

  it('skips a card that is not rendered and brings it back to its own place', () => {
    const without = RAIL_CARDS.filter((id) => id !== 'dance');
    expect(visiblePiles(piled, without)).toEqual([['violet', 'bbs'], ['bartender'], ['ember', 'lm']]);
    expect(visiblePiles([['dance'], ['violet']], without)).toEqual([['violet']]);
    // Changes made while it is away do not disturb it: it returns under the cards it was under.
    const flipped = flipPile(piled, 'ember');
    expect(visiblePiles(flipped, without)).toEqual([['violet', 'bbs'], ['bartender'], ['lm', 'ember']]);
    expect(visiblePiles(flipped, RAIL_CARDS)).toEqual([['violet', 'bbs'], ['bartender'], ['lm', 'dance', 'ember']]);
    expect(visiblePiles(movePile([['violet'], ['dance'], ['bartender']], 'violet', 'bartender', 'below'), RAIL_CARDS)).toEqual([['dance'], ['bartender'], ['violet']]);
    expect(visiblePiles(stackPile([['dance', 'ember'], ['lm']], 'ember', 'lm'), without)).toEqual([['ember', 'lm']]);
  });
});

describe('saved rail cards', () => {
  afterEach(() => { vi.restoreAllMocks(); window.localStorage.clear(); });

  it('starts from the plain column and reads back what was saved', () => {
    expect(loadRailPiles()).toEqual(single);
    saveRailPiles(piled);
    expect(loadRailPiles()).toEqual(piled);
  });

  it('puts cards the saved layout does not know last and drops cards that no longer exist', () => {
    expect(restoreRailPiles('[["bbs","violet"],["ember"]]')).toEqual([['bbs', 'violet'], ['ember'], ['bartender'], ['dance'], ['lm']]);
    expect(restoreRailPiles('[["gone"],["bbs","older","violet"]]')).toEqual([['bbs', 'violet'], ['bartender'], ['ember'], ['dance'], ['lm']]);
  });

  it.each(['', 'not json', 'null', '{"violet":0}', '["violet"]', '[["violet"],["violet"]]', '7'])('treats %j as never saved', (raw) => {
    expect(restoreRailPiles(raw)).toEqual(single);
  });

  it('still works when storage is unavailable', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('blocked'); });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('blocked'); });
    expect(loadRailPiles()).toEqual(single);
    expect(() => saveRailPiles(piled)).not.toThrow();
  });
});
