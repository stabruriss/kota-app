/** Brass tether — right-angle path from the terminal's header avatar
 *  to the owner's seat pill on the table. Rendered as an SVG overlay
 *  inside the scene so it shares `--rt-scale`.
 *
 *  Path shape (avatar at top of terminal, seat below on the desk):
 *    start at `(fromX, fromY)` — terminal avatar center
 *    stub down out of the header  → `(fromX, fromY + STUB)`
 *    elbow horizontal to seat x   → `(toX,   fromY + STUB)`
 *    drop vertical to seat pill   → `(toX,   toY)`
 *
 *  The middle leg runs inside the terminal body — that's OK; the
 *  terminal is 72% opacity so the line reads faintly through the
 *  text. User directive: "右角折线" (right-angle fold, not diagonal).
 *
 *  M3.1: endpoint coords spring to new values when the target agent
 *  changes, so the bend slides along the elbow path instead of
 *  snapping.  */

import { useEffect } from 'react';
import { motion, useMotionTemplate, useSpring, useTransform } from 'framer-motion';

const SCENE_W = 1120;
const SCENE_H = 660;
const STUB = 26;

/** Critical-damped spring — ~240ms to settle, no overshoot.  Matches
 *  the 200–300ms ease-out target in the design system without the
 *  bounciness that default `useSpring` gives. */
const SPRING = { stiffness: 260, damping: 32, mass: 0.8 } as const;

export interface TetherProps {
  fromX: number;
  fromY: number;
  toX: number;
  toY: number;
}

export function Tether({ fromX, fromY, toX, toY }: TetherProps) {
  // Four springs, one per endpoint coord.  `useMotionTemplate` then
  // composes them into the SVG `d` string, which Framer Motion can
  // animate directly on `<motion.path>`.
  const xs = useSpring(fromX, SPRING);
  const ys = useSpring(fromY, SPRING);
  const xe = useSpring(toX,   SPRING);
  const ye = useSpring(toY,   SPRING);

  useEffect(() => { xs.set(fromX); }, [xs, fromX]);
  useEffect(() => { ys.set(fromY); }, [ys, fromY]);
  useEffect(() => { xe.set(toX);   }, [xe, toX]);
  useEffect(() => { ye.set(toY);   }, [ye, toY]);

  // Elbow midline sits STUB px below the avatar — itself a spring so
  // the bend glides as the terminal slot changes.
  const midY = useTransform(ys, (v) => v + STUB);

  const pathD = useMotionTemplate`M ${xs} ${ys} L ${xs} ${midY} L ${xe} ${midY} L ${xe} ${ye}`;

  return (
    <svg
      className="term-tether-svg"
      viewBox={`0 0 ${SCENE_W} ${SCENE_H}`}
      preserveAspectRatio="none"
      width="100%"
      height="100%"
      style={{ position: 'absolute', inset: 0, pointerEvents: 'none', zIndex: 28 }}
      data-testid="tether"
    >
      <motion.path
        d={pathD}
        stroke="var(--brass-hi)"
        strokeWidth="1.5"
        fill="none"
        opacity="0.72"
        strokeLinecap="round"
        strokeLinejoin="round"
        style={{ filter: 'drop-shadow(0 0 3px rgba(197,187,170,0.45))' }}
      />
      <motion.circle cx={xs} cy={ys} r="2" fill="var(--brass-bright)" opacity="0.9" />
      <motion.circle
        cx={xe}
        cy={ye}
        r="3"
        fill="var(--brass-bright)"
        style={{ filter: 'drop-shadow(0 0 6px rgba(197,187,170,0.9))' }}
      />
    </svg>
  );
}
