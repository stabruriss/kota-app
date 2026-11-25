import type {
  DragEvent as ReactDragEvent, KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent,
} from 'react';
import {
  dealCard, flipPile, movePile, samePiles, saveRailPiles, stackPile, visiblePiles,
  type RailCardId, type RailPiles,
} from './rail-cards';
import { RailMotion, railPose, type RailPlayOptions } from './rail-motion';

/** A press on a control, on text that can be selected or on a list of choices never starts a drag. The rest of a card does. */
const NO_DRAG = 'button, a, [role="switch"], input, select, textarea, .violet-summary-fault, .bartender-pull-conflict-picker';
const GAP = 8;
const CARD_HEIGHT = 112;
const DRAG_DISTANCE = 5;
/** Past this share of the neighbour, pushing on reorders instead of stacking. */
const REORDER_LINE = 0.75;
const TURN_MS = 360;
const DROP_MS = 288;
/** After a deal the knob may be gone from under the pointer; a second click meant for it must not open the card beneath. */
const DEAL_QUIET_MS = 400;

interface Drag {
  id: RailCardId;
  pointerId: number;
  startX: number;
  startY: number;
  x: number;
  y: number;
  active: boolean;
  /** Where the hand holds the card, from its top edge. */
  grabY: number;
  /** The carried pile, top card first. */
  members: readonly RailCardId[];
  home: RailPiles;
  /** Top card of the pile that takes the carried one if released now. */
  target: RailCardId | null;
  /** 0–1: how far the carried card leans toward the target. */
  lean: number;
  /** Middle of the carried card, following the hand even where the card itself stops. */
  probe: number;
  /** Released here, the pile goes back where it came from. */
  out: boolean;
  dirty: boolean;
  frame: number;
}

/**
 * Drag, flip and deal for the rail cards. React renders where each card belongs; while a pile is carried, its position is
 * written straight to the DOM, and React hears from here only when the piles really change.
 */
export class RailDeck {
  /** Rendered cards and their names, kept current by RailCards after each commit. */
  labels: ReadonlyMap<RailCardId, string> = new Map();
  private stack!: HTMLElement;
  private scroller!: HTMLElement;
  private cue!: HTMLElement;
  private drag: Drag | null = null;
  private hot: RailCardId | null = null;
  private quietUntil = 0;
  private readonly motion = new RailMotion(() => this.slots());

  constructor(private piles: RailPiles, private readonly commit: (piles: RailPiles, animated: boolean) => void) {}

  mount(stack: HTMLElement, scroller: HTMLElement, cue: HTMLElement): void {
    this.stack = stack;
    this.scroller = scroller;
    this.cue = cue;
  }

  // ── Reading the column ────────────────────────────────────────────
  private slots(): HTMLElement[] {
    return [...this.stack.querySelectorAll<HTMLElement>('.rail-slot')];
  }

  private slot(id: RailCardId): HTMLElement {
    return this.stack.querySelector<HTMLElement>(`.rail-slot[data-card="${id}"]`)!;
  }

  private lift(id: RailCardId): HTMLElement {
    return this.slot(id).firstElementChild as HTMLElement;
  }

  private faceHeight(id: RailCardId): number {
    return (this.lift(id).firstElementChild!.firstElementChild as HTMLElement).offsetHeight;
  }

  private visible(): RailPiles {
    return visiblePiles(this.piles, [...this.labels.keys()]);
  }

  private pile(id: RailCardId): readonly RailCardId[] {
    return this.visible().find((pile) => pile.includes(id)) ?? [];
  }

  // ── Changing it ───────────────────────────────────────────────────
  private transition(next: RailPiles, options: RailPlayOptions, beforeCommit?: () => void): void {
    const before = this.motion.snapshot();
    this.motion.stop();
    beforeCommit?.();
    const carried = this.drag?.active ? this.drag : null;
    if (next !== this.piles) {
      // Laid out once with its old offset at the new place, the carried pile would hang below the list and bring in a scrollbar.
      for (const id of carried?.members ?? []) this.lift(id).style.transform = '';
      this.piles = next;
      this.commit(next, true);
    }
    if (carried) this.place(carried);
    this.motion.play(before, carried ? { ...options, skip: carried.members } : options);
    this.paintHot();
  }

  flip = (id: RailCardId, keyboard: boolean): void => {
    const next = this.pile(id)[1];
    this.transition(flipPile(this.piles, id), { sink: id, duration: TURN_MS });
    saveRailPiles(this.piles);
    if (keyboard) this.slot(next).querySelector<HTMLElement>('.rail-flip')!.focus({ preventScroll: true });
  };

  deal = (id: RailCardId, keyboard: boolean): void => {
    const [, next, third] = this.pile(id);
    if (this.hot === id) this.hot = next;
    this.transition(dealCard(this.piles, id), { front: [id], through: id, duration: TURN_MS });
    saveRailPiles(this.piles);
    this.quietUntil = performance.now() + DEAL_QUIET_MS;
    // Focus stays where the pile was: on its next deal knob, or on the one card left, or that card's first control.
    if (keyboard) {
      this.slot(next).querySelector<HTMLElement>(third ? '.rail-deal' : '[tabindex="0"], button:not(:disabled)')?.focus({ preventScroll: true });
    }
  };

  /** A card takes the height its content needs until the returned function runs. */
  grow = (id: RailCardId): (() => void) => {
    const set = (on: boolean) => {
      const before = this.motion.snapshot();
      this.motion.stop();
      this.slot(id).toggleAttribute('data-expanded', on);
      this.motion.play(before, { skip: this.drag?.active ? this.drag.members : [] });
    };
    set(true);
    return () => set(false);
  };

  // ── Hover belongs to the whole pile ───────────────────────────────
  hover = (event: ReactPointerEvent): void => {
    this.setHot(((event.target as Element).closest<HTMLElement>('.rail-slot')?.dataset.card as RailCardId | undefined) ?? null);
  };

  leave = (): void => this.setHot(null);

  private setHot(id: RailCardId | null): void {
    if (this.drag?.active || this.hot === id) return;
    this.hot = id;
    this.paintHot();
  }

  private paintHot(): void {
    const pile = this.hot && !this.drag?.active ? this.pile(this.hot) : [];
    for (const slot of this.slots()) slot.toggleAttribute('data-hot', pile.includes(slot.dataset.card as RailCardId));
  }

  // ── Input that must not get through ───────────────────────────────
  // Dialogs opened by a card are its React children too; only events from the cards themselves count.
  /** The browser's own picture drag would take the mouse away from the card. */
  guardDrag = (event: ReactDragEvent): void => {
    const target = event.target as Element;
    if (target.closest('.rail-slot') && !target.closest(NO_DRAG)) event.preventDefault();
  };

  guardClick = (event: ReactMouseEvent): void => {
    const target = event.target as Element;
    if (performance.now() < this.quietUntil && target.closest('.rail-slot') && !target.closest(NO_DRAG)) event.stopPropagation();
  };

  /** A key held on a knob keeps dealing; once focus lands on a card, its repeats must not open it. */
  guardKey = (event: ReactKeyboardEvent): void => {
    if (event.repeat && (event.key === 'Enter' || event.key === ' ') && (event.target as Element).matches('.rail-raise > [role="button"]')) {
      event.preventDefault();
      event.stopPropagation();
    }
  };

  // ── Drag ──────────────────────────────────────────────────────────
  press = (event: ReactPointerEvent): void => {
    const target = event.target as Element;
    const id = target.closest<HTMLElement>('.rail-slot')?.dataset.card as RailCardId | undefined;
    if (!id || event.button !== 0 || event.pointerType === 'touch' || target.closest(NO_DRAG)) return;
    this.abort();
    this.drag = {
      id, pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, x: event.clientX, y: event.clientY,
      active: false, grabY: 0, members: this.pile(id), home: this.piles, target: null, lean: 0, probe: 0, out: false, dirty: false, frame: 0,
    };
    this.listen('addEventListener');
  };

  /** Only while a press lasts. */
  private listen(method: 'addEventListener' | 'removeEventListener'): void {
    const on = (target: EventTarget, name: string, handler: (event: never) => void, capture = false) => target[method](name, handler as EventListener, capture);
    on(window, 'pointermove', this.move);
    on(window, 'pointerup', this.release);
    on(window, 'pointercancel', this.cancel);
    on(window, 'blur', this.cancel);
    on(window, 'keydown', this.key, true);
    on(this.scroller, 'scroll', this.scrolled);
  }

  private readonly move = (event: PointerEvent): void => {
    const d = this.drag!;
    if (event.pointerId !== d.pointerId) return;
    d.x = event.clientX;
    d.y = event.clientY;
    if (!d.active) {
      // The release never arrived, for example after a context menu: nothing is being held.
      if (!(event.buttons & 1)) { this.abort(); return; }
      if (Math.hypot(d.x - d.startX, d.y - d.startY) < DRAG_DISTANCE) return;
      this.pickUp(d);
    }
    d.dirty = true;
  };

  private pickUp(d: Drag): void {
    d.grabY = d.startY - this.lift(d.id).getBoundingClientRect().top;
    const before = this.motion.snapshot();
    this.motion.stop();
    d.active = true;
    document.body.style.cursor = 'grabbing';
    this.stack.setAttribute('data-carrying', '');
    for (const id of d.members) this.slot(id).setAttribute('data-dragging', '');
    this.place(d);
    this.motion.play(before, { skip: d.members });
    this.paintHot();
    this.slot(d.id).setPointerCapture(d.pointerId);
    this.cue.hidden = false;
    this.update(d);
    d.frame = requestAnimationFrame(this.tick);
  }

  private readonly key = (event: KeyboardEvent): void => {
    const d = this.drag!;
    if (event.key !== 'Escape' || !d.active) return;
    event.preventDefault();
    event.stopPropagation();
    this.drop(d, true);
    window.addEventListener('pointerup', swallowClick, { once: true });   // the button is still held
  };

  private readonly scrolled = (): void => { this.drag!.dirty = true; };

  private readonly tick = (): void => {
    const d = this.drag!;
    const view = this.scroller.getBoundingClientRect();
    const edge = 30;
    let speed = 0;
    if (d.x > view.left - 40 && d.x < view.right + 40) {
      if (d.y < view.top + edge) speed = -Math.ceil((view.top + edge - d.y) / 5);
      else if (d.y > view.bottom - edge) speed = Math.ceil((d.y - view.bottom + edge) / 5);
    }
    if (speed) this.scroller.scrollTop += Math.max(-14, Math.min(14, speed));
    const goal = d.target ? 1 : 0;
    if (Math.abs(goal - d.lean) > 0.01) {
      d.lean += (goal - d.lean) * 0.24;
      d.dirty = true;
    }
    if (d.dirty) {
      d.dirty = false;
      this.update(d);
    }
    d.frame = requestAnimationFrame(this.tick);
  };

  /** The carried pile keeps its slot in the list, which is where it will land; only its lift follows the hand. */
  private place(d: Drag): void {
    const box = this.slot(d.id).getBoundingClientRect();
    const first = this.stack.getBoundingClientRect().top;
    const footer = this.stack.querySelector<HTMLElement>('.sr-footer-slot');
    const last = (footer ? footer.getBoundingClientRect().top - GAP : this.stack.getBoundingClientRect().bottom) - box.height;
    const hand = d.y - d.grabY;
    // The card stops near the ends of the list; the aim keeps following the hand, so the first and last places stay reachable.
    const rubber = (over: number) => Math.min(34, over * 0.5);
    const held = hand < first ? first - rubber(first - hand) : hand > last ? last + rubber(hand - last) : hand;
    const top = d.target ? held + (this.slot(d.target).getBoundingClientRect().top - held) * 0.32 * d.lean : held;
    d.probe = hand + Math.min(this.faceHeight(d.id), CARD_HEIGHT) / 2;
    const dx = Math.max(-5, Math.min(5, (d.x - d.startX) * 0.08));
    for (const id of d.members) {
      const slot = this.slot(id);
      (slot.firstElementChild as HTMLElement).style.transform = `translate(${dx}px,${top - box.top}px) scale(${railPose(Number(slot.dataset.shown))})`;
      slot.style.setProperty('--tilt', `${dx * 0.17}deg`);
    }
  }

  private update(d: Drag): void {
    this.place(d);
    const rail = this.scroller.parentElement!.getBoundingClientRect();
    const out = d.x < rail.left - 90 || d.x > rail.right + 90 || d.y < rail.top - 120 || d.y > rail.bottom + 120;
    if (out && !d.out && this.piles !== d.home) this.transition(d.home, {});
    d.out = out;
    const held = d.target;
    d.target = null;
    if (!out) {
      const piles = this.visible();
      const at = piles.findIndex((pile) => pile.includes(d.id));
      const above = piles[at - 1]?.[0];
      const below = piles[at + 1]?.[0];
      const boxOf = (id: RailCardId) => ({ top: this.slot(id).getBoundingClientRect().top, height: this.faceHeight(id) });
      const a = above && boxOf(above);
      const b = below && boxOf(below);
      if (b && d.probe > b.top + REORDER_LINE * b.height) {
        this.transition(movePile(this.piles, d.id, below, 'below'), {});
        d.dirty = true;
      } else if (a && d.probe < a.top + (1 - REORDER_LINE) * a.height) {
        this.transition(movePile(this.piles, d.id, above, 'above'), {});
        d.dirty = true;
      } else if (b && d.probe >= b.top - (held === below ? 6 : 0)) {
        // The middle of the carried card crosses the near edge: half of the neighbour is covered.
        d.target = below;
      } else if (a && d.probe <= a.top + a.height + (held === above ? 6 : 0)) {
        d.target = above;
      }
    }
    this.paint(d);
  }

  private paint(d: Drag): void {
    for (const slot of this.slots()) slot.toggleAttribute('data-target', slot.dataset.card === d.target);
    this.stack.toggleAttribute('data-armed', d.target !== null);
    for (const id of d.members) {
      this.slot(id).toggleAttribute('data-docking', d.target !== null);
      this.slot(id).toggleAttribute('data-aborting', d.out);
    }
    let kind = 'order';
    let text: string;
    if (d.target) {
      kind = 'stack';
      text = `Release to stack on ${this.labels.get(d.target)}`;
    } else if (samePiles(this.piles, d.home)) {
      kind = 'home';
      text = 'Release to put it back';
    } else {
      const piles = this.visible();
      const above = piles[piles.findIndex((pile) => pile.includes(d.id)) - 1]?.[0];
      text = above ? `Release to place below ${this.labels.get(above)}` : 'Release to place first';
    }
    if (this.cue.textContent !== text) this.cue.textContent = text;
    this.cue.dataset.kind = kind;
  }

  private readonly release = (event: PointerEvent): void => {
    const d = this.drag!;
    if (event.pointerId !== d.pointerId) return;
    if (!d.active) { this.abort(); return; }
    d.x = event.clientX;
    d.y = event.clientY;
    this.update(d);
    this.drop(d, d.out);
    swallowClick();
  };

  private readonly cancel = (): void => {
    const d = this.drag!;
    if (d.active) this.drop(d, true);
    else this.abort();
  };

  private drop(d: Drag, back: boolean): void {
    const target = back ? null : d.target;
    const next = back ? d.home : target ? stackPile(this.piles, d.id, target) : this.piles;
    this.hot = null;
    this.transition(next, { front: d.members, duration: DROP_MS }, () => this.letGo(d));
    if (!back) saveRailPiles(next);
    if (target) this.motion.pulse(this.slot(d.id));
    const box = this.slot(d.id).getBoundingClientRect();
    if (d.x >= box.left && d.x <= box.right && d.y >= box.top && d.y <= box.bottom) this.setHot(d.id);
  }

  /** Everything a drag put on the page comes off again. */
  private letGo(d: Drag): void {
    this.drag = null;
    this.listen('removeEventListener');
    cancelAnimationFrame(d.frame);
    document.body.style.cursor = '';
    this.stack.removeAttribute('data-carrying');
    this.stack.removeAttribute('data-armed');
    this.cue.hidden = true;
    for (const slot of this.slots()) {
      for (const name of ['data-dragging', 'data-target', 'data-docking', 'data-aborting']) slot.removeAttribute(name);
      slot.style.removeProperty('--tilt');
      (slot.firstElementChild as HTMLElement).style.transform = '';
    }
  }

  /** Ends a press without a landing: a plain click, or the cards changed underneath. */
  abort = (): void => {
    const d = this.drag;
    if (!d) return;
    this.letGo(d);
    if (this.piles === d.home) return;
    this.piles = d.home;
    this.commit(d.home, false);
  };
}

function swallowClick(): void {
  const block = (event: Event) => {
    event.preventDefault();
    event.stopPropagation();
  };
  window.addEventListener('click', block, { capture: true, once: true });
  window.setTimeout(() => window.removeEventListener('click', block, { capture: true }));
}
