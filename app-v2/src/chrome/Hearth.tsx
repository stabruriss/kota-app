/** Hearth — the round-table centerpiece animation.
 *
 *  Three modes: `fire` (24f PNG sprite, warm campfire), `magic` (10-crop
 *  sprite, blue/gold glow), `flora` (static PNG with gentle sway).
 *  Visual behavior is driven by the `data-centerpiece` attribute — CSS
 *  in `canvas.css` switches background-size, keyframes, and filters.
 *  Mounts as a child of `#hearth-anim-slot` inside `.rt-hearth`.
 *  See M2 salvage in `.context/HANDOFF-app-v2.md`. */

import campfireUrl from '../assets/campfire-24f-alpha.png';
import magicUrl from '../assets/magic-fire-alpha.png';
import floraUrl from '../assets/flora-alpha.png';

export type Centerpiece = 'fire' | 'magic' | 'flora';

const SPRITE_URLS: Record<Centerpiece, string> = {
  fire: campfireUrl,
  magic: magicUrl,
  flora: floraUrl,
};

export function Hearth({ centerpiece }: { centerpiece: Centerpiece }) {
  return (
    <div
      className="rt-fire-flames"
      data-centerpiece={centerpiece}
      data-testid="hearth"
      style={{ backgroundImage: `url(${SPRITE_URLS[centerpiece]})` }}
      aria-hidden
    />
  );
}
