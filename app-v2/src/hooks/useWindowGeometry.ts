/** useWindowGeometry — per-window OS-style geometry state.
 *
 *  Stores the user's preferred `{x, y, w, h, z, minimized}` per
 *  `storageKey` in localStorage.
 *  Designed for `<WindowFrame>` consumers: they `useWindowGeometry()`
 *  and pass the result into the frame, which mutates it during drag /
 *  resize. The hook handles defaults, viewport-clamp on mount, and
 *  debounced persistence.
 *
 *  Per HANDOFF-W-FloatingWindows.md §4 default parameters:
 *    - default size 680 × 460, min 320 × 200
 *    - cascade offset (28, 28) per new window
 *    - off-screen rescue: keep at least 80px header in viewport on load   */

import { useCallback, useEffect, useRef, useState } from 'react';

export interface WindowGeometry {
  x: number;
  y: number;
  w: number;
  h: number;
  /** Higher z = closer to user. Initial value picked by the parent
   *  window manager so all windows share a consistent z-stack. */
  z: number;
  minimized: boolean;
}

export interface UseWindowGeometryOpts {
  /** Unique persistence key — caller usually derives this from
   *  `kota-v2.window-geom.{project_id}.{agent_id}`. */
  storageKey: string;
  /** Used when nothing's persisted. */
  defaultGeom: Pick<WindowGeometry, 'x' | 'y' | 'w' | 'h' | 'z'>;
}

export const WINDOW_DEFAULT_W = 680;
export const WINDOW_DEFAULT_H = 460;
export const WINDOW_MIN_W = 320;
export const WINDOW_MIN_H = 200;
export const WINDOW_CASCADE_DX = 28;
export const WINDOW_CASCADE_DY = 28;
export const WINDOW_HEADER_VISIBLE_PX = 80;

/** Compute a cascade-offset position for the Nth window, centred on
 *  viewport. nIndex = 0 → centred; nIndex > 0 → cascade. */
export function cascadePosition(
  nIndex: number,
  w: number = WINDOW_DEFAULT_W,
  h: number = WINDOW_DEFAULT_H,
  viewport: { width: number; height: number } = readViewport(),
): { x: number; y: number } {
  const baseX = Math.max(16, Math.floor((viewport.width - w) / 2));
  const baseY = Math.max(16, Math.floor((viewport.height - h) / 2));
  return {
    x: baseX + nIndex * WINDOW_CASCADE_DX,
    y: baseY + nIndex * WINDOW_CASCADE_DY,
  };
}

function readViewport(): { width: number; height: number } {
  if (typeof window === 'undefined') return { width: 1280, height: 800 };
  return { width: window.innerWidth, height: window.innerHeight };
}

/** Clamp geometry so at least `headerVisible` px of the header stays
 *  on-screen — matches macOS behaviour after a monitor / resize. */
export function clampToViewport(
  geom: WindowGeometry,
  viewport: { width: number; height: number } = readViewport(),
  headerVisible: number = WINDOW_HEADER_VISIBLE_PX,
): WindowGeometry {
  const w = Math.max(WINDOW_MIN_W, Math.min(geom.w, viewport.width));
  const h = Math.max(WINDOW_MIN_H, Math.min(geom.h, viewport.height));
  // X: keep at least `headerVisible` px of the title bar reachable.
  const minX = -(w - headerVisible);
  const maxX = viewport.width - headerVisible;
  const x = Math.max(minX, Math.min(geom.x, maxX));
  // Y: never let the header escape the top — and keep the whole header
  // (≈ 28 px) in view at the bottom.
  const minY = 0;
  const maxY = Math.max(0, viewport.height - 28);
  const y = Math.max(minY, Math.min(geom.y, maxY));
  return { ...geom, x, y, w, h };
}

function loadPersisted(
  key: string,
  fallback: Pick<WindowGeometry, 'x' | 'y' | 'w' | 'h' | 'z'>,
): WindowGeometry {
  const initial: WindowGeometry = { ...fallback, minimized: false };
  if (typeof window === 'undefined') return initial;
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return initial;
    const parsed = JSON.parse(raw) as Partial<WindowGeometry>;
    return {
      x: typeof parsed.x === 'number' ? parsed.x : initial.x,
      y: typeof parsed.y === 'number' ? parsed.y : initial.y,
      w: typeof parsed.w === 'number' ? parsed.w : initial.w,
      h: typeof parsed.h === 'number' ? parsed.h : initial.h,
      z: typeof parsed.z === 'number' ? parsed.z : initial.z,
      minimized: parsed.minimized === true,
    };
  } catch {
    return initial;
  }
}

function hasGeometryPatch<K extends keyof WindowGeometry>(
  patch: Partial<WindowGeometry>,
  key: K,
): patch is Partial<WindowGeometry> & Pick<WindowGeometry, K> {
  return Object.prototype.hasOwnProperty.call(patch, key);
}

function mergePreferredGeometry(
  preferred: WindowGeometry,
  patch: Partial<WindowGeometry>,
  visible: WindowGeometry,
): WindowGeometry {
  const next = { ...preferred, ...patch };
  if (hasGeometryPatch(patch, 'x')) next.x = visible.x;
  if (hasGeometryPatch(patch, 'y')) next.y = visible.y;
  if (hasGeometryPatch(patch, 'w')) next.w = visible.w;
  if (hasGeometryPatch(patch, 'h')) next.h = visible.h;
  return next;
}

export function useWindowGeometry(opts: UseWindowGeometryOpts) {
  const preferredGeomRef = useRef<WindowGeometry | null>(null);
  if (preferredGeomRef.current === null) {
    preferredGeomRef.current = loadPersisted(opts.storageKey, opts.defaultGeom);
  }

  const [geom, setGeomState] = useState<WindowGeometry>(() =>
    clampToViewport(preferredGeomRef.current ?? { ...opts.defaultGeom, minimized: false }),
  );

  // Debounce persistence: window geom changes a lot during drag.
  const persistTimer = useRef<number | null>(null);
  useEffect(() => {
    if (typeof window === 'undefined') return;
    if (persistTimer.current != null) {
      window.clearTimeout(persistTimer.current);
    }
    persistTimer.current = window.setTimeout(() => {
      try {
        localStorage.setItem(opts.storageKey, JSON.stringify(preferredGeomRef.current ?? geom));
      } catch {
        /* quota / private mode — ignore */
      }
    }, 200);
    return () => {
      if (persistTimer.current != null) {
        window.clearTimeout(persistTimer.current);
      }
    };
  }, [geom, opts.storageKey]);

  // Re-clamp on viewport resize.
  useEffect(() => {
    if (typeof window === 'undefined') return;
    // Re-clamp from the user's PREFERRED geometry, not the currently-displayed
    // (possibly already-shrunk) geometry — otherwise a viewport shrink ratchets
    // windows down and they never grow back when the viewport expands again.
    const onResize = () =>
      setGeomState((prev) => clampToViewport(preferredGeomRef.current ?? prev));
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);

  const setGeom = useCallback(
    (next: Partial<WindowGeometry> | ((prev: WindowGeometry) => Partial<WindowGeometry>)) => {
      setGeomState((prev) => {
        const patch = typeof next === 'function' ? next(prev) : next;
        const visible = clampToViewport({ ...prev, ...patch });
        preferredGeomRef.current = mergePreferredGeometry(
          preferredGeomRef.current ?? prev,
          patch,
          visible,
        );
        return visible;
      });
    },
    [],
  );

  return [geom, setGeom] as const;
}
