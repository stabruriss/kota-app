/** Motion of the rail cards: measure, change, then play every card from where it was. Nothing runs between changes. */

const SHIFT_MS = 252;
const EASE_OUT = 'cubic-bezier(.22,1,.36,1)';
const EASE_SETTLE = 'cubic-bezier(.2,1.2,.36,1)';

/** A card under the top one is drawn narrower: 6 px in on each side of the 275 px column per step. */
export const railPose = (shown: number) => 1 - shown * 0.0436;

const liftOf = (slot: HTMLElement) => slot.firstElementChild as HTMLElement;
const shownOf = (slot: HTMLElement) => Number(slot.dataset.shown);

export interface RailPlayOptions {
  /** Cards kept above their neighbours while they travel. */
  front?: readonly string[];
  /** The card that slides out in front and tucks back in under its pile. */
  sink?: string;
  /** Cards that let the pointer through to what is below while they travel. */
  through?: string;
  /** Cards something else is moving. */
  skip?: readonly string[];
  duration?: number;
}

export class RailMotion {
  private live: Animation[] = [];
  private marked: HTMLElement[] = [];

  constructor(private readonly slots: () => HTMLElement[]) {}

  snapshot(): Map<string, DOMRect> {
    return new Map(this.slots().map((slot) => [slot.dataset.card!, liftOf(slot).getBoundingClientRect()]));
  }

  stop(): void {
    for (const animation of this.live) animation.cancel();
    this.live = [];
    for (const slot of this.marked) {
      slot.removeAttribute('data-moving');
      slot.removeAttribute('data-through');
    }
    this.marked = [];
  }

  play(before: ReadonlyMap<string, DOMRect>, { front = [], sink, through, skip = [], duration = SHIFT_MS }: RailPlayOptions = {}): void {
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    for (const slot of this.slots()) {
      const id = slot.dataset.card!;
      const from = before.get(id);
      if (!from || skip.includes(id)) continue;
      const lift = liftOf(slot);
      const to = lift.getBoundingClientRect();
      const still = (['left', 'top', 'width', 'height'] as const).every((side) => Math.abs(to[side] - from[side]) < 0.5);
      if (still && id !== sink) continue;

      const shown = shownOf(slot);
      const pose = railPose(shown);
      // The card's own box before any transform. Every frame keeps the top edge where the eye expects it.
      const box = { width: to.width / pose, height: to.height / pose, centre: to.left + to.width / 2, bottom: to.bottom };
      const at = (rect: { left: number; top: number; width: number }) => {
        const scale = rect.width / box.width;
        return `translate(${rect.left + rect.width / 2 - box.centre}px,${rect.top + scale * box.height - box.bottom}px) scale(${scale})`;
      };
      const rest = `translate(0px,0px) scale(${pose})`;
      const seen = slot.classList.contains('is-deep') ? { visibility: 'visible' } : {};

      if (id === sink) {
        const startBottom = from.top + (from.width / box.width) * box.height;
        const scale = (from.width / box.width) * 0.955;
        const floor = slot.parentElement!.getBoundingClientRect().bottom - 2;
        const reach = Math.max(startBottom + Math.min(box.height * 0.5, 58), to.bottom + 22);
        const bottom = Math.max(to.bottom + 8, Math.min(reach, floor));
        const out = { width: box.width * scale, left: box.centre - (box.width * scale) / 2, top: bottom - scale * box.height };
        this.run(lift, [
          { transform: at(from), easing: 'cubic-bezier(.45,0,.75,.85)', ...seen },
          { transform: at(out), offset: 0.44, easing: 'cubic-bezier(.2,.7,.3,1)', ...seen },
          { transform: rest, ...seen },
        ], { duration });
        const under = 6 - shown;
        this.run(slot, [{ zIndex: 45, offset: 0 }, { zIndex: 45, offset: 0.44 }, { zIndex: under, offset: 0.44 }, { zIndex: under, offset: 1 }], { duration });
        continue;
      }

      const inFront = front.includes(id);
      this.run(lift, [{ transform: at(from), ...seen }, { transform: rest, ...seen }], { duration, easing: inFront ? EASE_SETTLE : EASE_OUT });
      if (inFront) {
        this.run(slot, [{ zIndex: 44 - shown }, { zIndex: 44 - shown }], { duration });
        slot.setAttribute('data-moving', '');
        if (id === through) slot.setAttribute('data-through', '');
        this.marked.push(slot);
      }
      // A card that just grew unrolls downward instead of popping.
      const grown = box.height - from.height / (from.width / box.width);
      if (shown === 0 && grown > 2) {
        this.run(lift.firstElementChild as HTMLElement,
          [{ clipPath: `inset(-14px -14px ${grown}px -14px)` }, { clipPath: 'inset(-14px -14px -14px -14px)' }], { duration, easing: EASE_OUT });
      }
    }
    const mine = this.live;
    void Promise.allSettled(mine.map((animation) => animation.finished)).then(() => {
      if (this.live === mine) this.stop();
    });
  }

  /** The card that received a pile acknowledges it once. */
  pulse(slot: HTMLElement): void {
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    liftOf(slot).animate([{ opacity: 0.95, transform: 'scale(1)' }, { opacity: 0, transform: 'scale(1.035,1.07)' }],
      { duration: 560, easing: 'ease-out', pseudoElement: '::after' });
  }

  private run(element: HTMLElement, frames: Keyframe[], options: KeyframeAnimationOptions): void {
    this.live.push(element.animate(frames, options));
  }
}
