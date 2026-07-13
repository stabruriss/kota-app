/** WindowFrame — OS-style draggable / resizable / focusable window.
 *
 *  Per HANDOFF-W-FloatingWindows.md:
 *    - Header drag (whole header, not a small grip).
 *    - Four-corner resize (NE / NW / SE / SW; no edge handles).
 *    - Click anywhere → onFocus (parent decides z-bump).
 *    - Minimize button on header — close button does NOT exist.
 *    - Focused state visually highlighted (border + shadow).
 *
 *  Geometry is owned by the parent via `useWindowGeometry`. This frame
 *  is dumb about persistence: it just reports drag/resize ends via
 *  `onGeomChange` and the parent applies + persists. */

import { useCallback, useEffect, useRef, type CSSProperties, type ReactNode } from 'react';
import {
  WINDOW_MIN_H,
  WINDOW_MIN_W,
  type WindowGeometry,
} from '../hooks/useWindowGeometry';

export interface WindowFrameProps {
  windowId: string;
  geom: WindowGeometry;
  focused: boolean;
  className?: string;
  /** Header content — usually agent avatar + name + status. */
  title: ReactNode;
  /** Body content — usually <TerminalGrid>. */
  children: ReactNode;
  onGeomChange: (next: Partial<WindowGeometry>) => void;
  onFocus: () => void;
  onMinimize: () => void;
  /** Optional ⋯ menu (W stub for M6.E). */
  onSettings?: () => void;
}

interface DragState {
  kind:
    | 'move'
    | 'resize-ne' | 'resize-nw' | 'resize-se' | 'resize-sw'
    | 'resize-n'  | 'resize-s'  | 'resize-e'  | 'resize-w';
  startPointerX: number;
  startPointerY: number;
  startGeom: WindowGeometry;
}

export function WindowFrame({
  windowId,
  geom,
  focused,
  className,
  title,
  children,
  onGeomChange,
  onFocus,
  onMinimize,
  onSettings,
}: WindowFrameProps) {
  const dragRef = useRef<DragState | null>(null);
  const frameRef = useRef<HTMLDivElement | null>(null);

  // Imperatively patch the frame's inline style during drag/resize so we
  // don't trigger a React re-render every pointermove. setGeom is fired
  // once on pointer-up.
  const applyDragPreview = useCallback((next: Partial<WindowGeometry>) => {
    const el = frameRef.current;
    if (!el) return;
    if (typeof next.x === 'number') el.style.left = `${next.x}px`;
    if (typeof next.y === 'number') el.style.top = `${next.y}px`;
    if (typeof next.w === 'number') el.style.width = `${next.w}px`;
    if (typeof next.h === 'number') el.style.height = `${next.h}px`;
  }, []);

  const onPointerMove = useCallback(
    (e: PointerEvent) => {
      const drag = dragRef.current;
      if (!drag) return;
      const dx = e.clientX - drag.startPointerX;
      const dy = e.clientY - drag.startPointerY;
      const g = drag.startGeom;
      const patch: Partial<WindowGeometry> = {};
      switch (drag.kind) {
        case 'move':
          patch.x = g.x + dx;
          patch.y = g.y + dy;
          break;
        case 'resize-se':
          patch.w = Math.max(WINDOW_MIN_W, g.w + dx);
          patch.h = Math.max(WINDOW_MIN_H, g.h + dy);
          break;
        case 'resize-sw':
          patch.w = Math.max(WINDOW_MIN_W, g.w - dx);
          patch.h = Math.max(WINDOW_MIN_H, g.h + dy);
          patch.x = g.x + (g.w - (patch.w ?? g.w));
          break;
        case 'resize-ne':
          patch.w = Math.max(WINDOW_MIN_W, g.w + dx);
          patch.h = Math.max(WINDOW_MIN_H, g.h - dy);
          patch.y = g.y + (g.h - (patch.h ?? g.h));
          break;
        case 'resize-nw':
          patch.w = Math.max(WINDOW_MIN_W, g.w - dx);
          patch.h = Math.max(WINDOW_MIN_H, g.h - dy);
          patch.x = g.x + (g.w - (patch.w ?? g.w));
          patch.y = g.y + (g.h - (patch.h ?? g.h));
          break;
        case 'resize-n':
          patch.h = Math.max(WINDOW_MIN_H, g.h - dy);
          patch.y = g.y + (g.h - (patch.h ?? g.h));
          break;
        case 'resize-s':
          patch.h = Math.max(WINDOW_MIN_H, g.h + dy);
          break;
        case 'resize-e':
          patch.w = Math.max(WINDOW_MIN_W, g.w + dx);
          break;
        case 'resize-w':
          patch.w = Math.max(WINDOW_MIN_W, g.w - dx);
          patch.x = g.x + (g.w - (patch.w ?? g.w));
          break;
      }
      applyDragPreview(patch);
    },
    [applyDragPreview],
  );

  // Keep a stable ref to the latest onGeomChange so onPointerUp's
  // useCallback does not change identity every render. This matters
  // because the parent re-renders on focus change and passes a NEW
  // arrow function for onGeomChange — without this ref, the cleanup
  // useEffect below would tear down the in-flight drag's window
  // listeners the moment focus flipped, killing the drag mid-gesture.
  const onGeomChangeRef = useRef(onGeomChange);
  useEffect(() => { onGeomChangeRef.current = onGeomChange; }, [onGeomChange]);

  const onPointerUp = useCallback(
    (e: PointerEvent) => {
      const drag = dragRef.current;
      if (!drag) return;
      // Read final inline-style values back as the new geom.
      const el = frameRef.current;
      if (el) {
        const next: Partial<WindowGeometry> = {
          x: parseFloat(el.style.left) || drag.startGeom.x,
          y: parseFloat(el.style.top) || drag.startGeom.y,
          w: parseFloat(el.style.width) || drag.startGeom.w,
          h: parseFloat(el.style.height) || drag.startGeom.h,
        };
        onGeomChangeRef.current(next);
      }
      dragRef.current = null;
      window.removeEventListener('pointermove', onPointerMove);
      window.removeEventListener('pointerup', onPointerUp);
      // Release pointer capture if it was held by the captured target.
      const t = e.target as HTMLElement | null;
      if (t && t.releasePointerCapture) {
        try { t.releasePointerCapture(e.pointerId); } catch { /* ignore */ }
      }
      document.body.style.userSelect = '';
      document.body.style.cursor = '';
    },
    [onPointerMove],
  );

  const startDrag = useCallback(
    (kind: DragState['kind']) =>
      (e: React.PointerEvent) => {
        e.preventDefault();
        // Don't stopPropagation — the frame's onPointerDown also calls
        // onFocus, which is fine (idempotent) and ensures focus is set
        // even if the drag is preempted. macOS standard: a single
        // mouse-down on an inactive window's header activates AND
        // begins the drag in one gesture; pointer capture below keeps
        // the move/up events flowing to us regardless of any focus-
        // induced re-render shifting the layout under the cursor.
        const target = e.currentTarget as HTMLElement;
        try { target.setPointerCapture(e.pointerId); } catch { /* unsupported */ }
        dragRef.current = {
          kind,
          startPointerX: e.clientX,
          startPointerY: e.clientY,
          startGeom: { ...geom },
        };
        document.body.style.userSelect = 'none';
        document.body.style.cursor =
          kind === 'move' ? 'grabbing'
          : kind === 'resize-se' || kind === 'resize-nw' ? 'nwse-resize'
          : kind === 'resize-ne' || kind === 'resize-sw' ? 'nesw-resize'
          : kind === 'resize-n'  || kind === 'resize-s'  ? 'ns-resize'
          : 'ew-resize';
        window.addEventListener('pointermove', onPointerMove);
        window.addEventListener('pointerup', onPointerUp);
        // Activate immediately so the window frame's focused styling
        // updates this frame.
        onFocus();
      },
    [geom, onPointerMove, onPointerUp, onFocus],
  );

  // Belt-and-suspenders cleanup if unmounted mid-drag.
  useEffect(() => {
    return () => {
      window.removeEventListener('pointermove', onPointerMove);
      window.removeEventListener('pointerup', onPointerUp);
      document.body.style.userSelect = '';
      document.body.style.cursor = '';
    };
  }, [onPointerMove, onPointerUp]);

  if (geom.minimized) return null;

  const frameStyle: CSSProperties = {
    position: 'absolute',
    left: geom.x,
    top: geom.y,
    width: geom.w,
    height: geom.h,
    zIndex: geom.z,
  };

  return (
    <div
      ref={frameRef}
      className={`win-frame ${focused ? 'focused' : 'blurred'} ${className ?? ''}`}
      style={frameStyle}
      data-window-id={windowId}
      data-testid={`win-frame-${windowId}`}
      onPointerDown={onFocus}
    >
      <div
        className="win-header"
        onPointerDown={startDrag('move')}
        data-testid={`win-header-${windowId}`}
      >
        <div className="win-title">{title}</div>
        <div className="win-controls">
          {onSettings && (
            <button
              type="button"
              className="win-btn"
              title="Settings"
              aria-label="Window settings"
              onClick={(e) => {
                e.stopPropagation();
                onSettings();
              }}
              onPointerDown={(e) => e.stopPropagation()}
            >
              ⋯
            </button>
          )}
          <button
            type="button"
            className="win-btn"
            title="Minimize"
            aria-label="Minimize window"
            onClick={(e) => {
              e.stopPropagation();
              onMinimize();
            }}
            onPointerDown={(e) => e.stopPropagation()}
          >
            –
          </button>
        </div>
      </div>
      <div className="win-body">{children}</div>

      {/* 8 resize handles — 4 corners (16×16) + 4 edges (6px strip). */}
      <div className="win-resize win-resize-nw" onPointerDown={startDrag('resize-nw')} aria-hidden />
      <div className="win-resize win-resize-ne" onPointerDown={startDrag('resize-ne')} aria-hidden />
      <div className="win-resize win-resize-sw" onPointerDown={startDrag('resize-sw')} aria-hidden />
      <div className="win-resize win-resize-se" onPointerDown={startDrag('resize-se')} aria-hidden />
      <div className="win-resize win-resize-n"  onPointerDown={startDrag('resize-n')}  aria-hidden />
      <div className="win-resize win-resize-s"  onPointerDown={startDrag('resize-s')}  aria-hidden />
      <div className="win-resize win-resize-w"  onPointerDown={startDrag('resize-w')}  aria-hidden />
      <div className="win-resize win-resize-e"  onPointerDown={startDrag('resize-e')}  aria-hidden />
    </div>
  );
}
