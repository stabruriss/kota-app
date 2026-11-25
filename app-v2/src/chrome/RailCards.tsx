import {
  Children, createContext, useCallback, useContext, useId, useLayoutEffect, useRef, useState,
  type CSSProperties, type ReactElement, type ReactNode,
} from 'react';
import { flushSync } from 'react-dom';
import { loadRailPiles, visiblePiles, type RailCardId, type RailPiles } from '../lib/rail-cards';
import { RailDeck } from '../lib/rail-deck';
import '../styles/rail-cards.css';

/** Cards drawn under the top one. Deeper ones are hidden behind the last of them. */
const EDGES = 2;

export interface RailCardProps {
  id: RailCardId;
  label: string;
  /** What the card wants the user to know while it is buried. `fault` marks a failure. */
  notice?: { text: string; fault?: boolean } | null | false;
  children: ReactNode;
}

interface Place {
  order: number;
  rank: number;
  depth: number;
  size: number;
  lamp: 'note' | 'fault' | null;
  tip: string;
}

const RailContext = createContext<{ deck: RailDeck; places: ReadonlyMap<RailCardId, Place> } | null>(null);
const GrowContext = createContext<(() => () => void) | null>(null);

function placeCards(piles: RailPiles, cards: ReadonlyMap<RailCardId, RailCardProps>): ReadonlyMap<RailCardId, Place> {
  const places = new Map<RailCardId, Place>();
  visiblePiles(piles, [...cards.keys()]).forEach((pile, position) => {
    const members = pile.map((id) => cards.get(id)!);
    const told = members.slice(1).flatMap(({ label, notice }) => (notice ? [`${label} · ${notice.text}`] : []));
    members.forEach(({ id }, rank) => {
      // The last visible edge speaks for every card beneath it.
      const spoken = rank === 0 ? [] : rank < EDGES ? [members[rank]] : members.slice(rank);
      const notices = spoken.flatMap(({ notice }) => notice || []);
      places.set(id, {
        order: position * 10 + rank,
        rank,
        depth: Math.min(EDGES, members.length - 1),
        size: members.length,
        lamp: notices.some(({ fault }) => fault) ? 'fault' : notices.length > 0 ? 'note' : null,
        tip: members.length > 1 ? [`Next: ${members[1].label} · ${members.length} cards`, ...told].join('\n') : '',
      });
    });
  });
  return places;
}

/** The scrolling column of system cards. It owns their order and piles, so dragging never renders the column around it. */
export function RailCards({ children }: { children: ReactNode }) {
  const cards = new Map(
    (Children.toArray(children) as ReactElement<RailCardProps>[])
      .filter((child) => child.type === RailCard)
      .map(({ props }) => [props.id, props] as const),
  );
  const [piles, setPiles] = useState(loadRailPiles);
  const [deck] = useState(() => new RailDeck(piles, (next, animated) => {
    if (animated) flushSync(() => setPiles(next));
    else setPiles(next);
  }));
  const stackRef = useRef<HTMLDivElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const cueRef = useRef<HTMLOutputElement>(null);
  const present = [...cards.keys()].join(' ');

  useLayoutEffect(() => { deck.labels = new Map([...cards].map(([id, { label }]) => [id, label])); });
  useLayoutEffect(() => deck.mount(stackRef.current!, scrollerRef.current!, cueRef.current!), [deck]);
  useLayoutEffect(() => deck.abort, [deck, present]);

  return (
    <>
      <div className="sr-body" ref={scrollerRef}>
        <div
          className="system-agent-stack"
          ref={stackRef}
          onPointerDown={deck.press}
          onPointerOver={deck.hover}
          onPointerLeave={deck.leave}
          onClickCapture={deck.guardClick}
          onKeyDownCapture={deck.guardKey}
          onDragStart={deck.guardDrag}
        >
          <RailContext.Provider value={{ deck, places: placeCards(piles, cards) }}>{children}</RailContext.Provider>
        </div>
      </div>
      <output className="rail-drop-cue" ref={cueRef} aria-live="off" hidden />
    </>
  );
}

export function RailCard({ id, children }: RailCardProps) {
  const { deck, places } = useContext(RailContext)!;
  const { order, rank, depth, size, lamp, tip } = places.get(id)!;
  const tipId = useId();
  const grow = useCallback(() => deck.grow(id), [deck, id]);
  const state = [rank === 0 ? 'is-top' : 'is-under', rank > EDGES && 'is-deep', size > 1 && 'in-pile'].filter(Boolean).join(' ');
  const shown = Math.min(rank, EDGES);

  return (
    <div
      className={`rail-slot ${state}`}
      data-card={id}
      data-shown={shown}
      style={{ order, '--depth': depth, '--shown': shown } as CSSProperties}
      {...(rank > 0 ? { inert: '' } : {})}
    >
      <div className="rail-lift">
        <div className="rail-raise">
          <GrowContext.Provider value={grow}>{children}</GrowContext.Provider>
          {lamp && <i className={`rail-lamp ${lamp}`} aria-hidden />}
          {rank === 0 && size > 1 && (
            <>
              <button type="button" className="rail-knob rail-flip" aria-label="Flip stack" aria-describedby={tipId} onClick={(event) => deck.flip(id, event.detail === 0)}>
                <svg viewBox="0 0 16 16" aria-hidden>
                  <rect x="1.9" y="3.6" width="8" height="5.6" rx="1.3" /><path d="M3.6 12.2h4.6" /><path d="M12.3 3.4c1.9 2 1.9 5.2 0 7.3" /><path d="M14.3 9.1l-2.1 1.8-.6-2.6" />
                </svg>
              </button>
              <span className="rail-tip" id={tipId} role="tooltip">{tip}</span>
              <button type="button" className="rail-knob rail-deal" aria-label="Deal top card below stack" title="Deal top card below stack" onClick={(event) => deck.deal(id, event.detail === 0)}>
                <svg viewBox="0 0 16 16" aria-hidden>
                  <rect x="3.2" y="1.9" width="9.6" height="5.6" rx="1.3" /><path d="M8 9.9v4.3" /><path d="M5.9 12.2 8 14.3l2.1-2.1" />
                </svg>
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

/** Rendered inside a card by content that needs more than the common height, for as long as it is there. */
export function RailCardGrows() {
  const grow = useContext(GrowContext);
  useLayoutEffect(() => grow?.(), [grow]);
  return null;
}
