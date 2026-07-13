/** ColorPicker — Room / Desk tint + Center sprite picker.
 *
 *  Polish v2: collapsed by default into a translucent paintbrush
 *  trigger at the scene's top-left corner. Clicking opens a popover
 *  with four compact rows. Outside-click / Esc / re-click
 *  close. Selection persists immediately (no Apply button); state
 *  flow into App.tsx is unchanged.
 *
 *  Rows inside the popover:
 *    - Scene: image/solid room background
 *    - Table: image/solid desk texture
 *    - Room Light: 5 tint swatches → sets `--room-tint-color` on `.rt-scene`
 *    - Desk Light: 5 tint swatches → sets `--desk-tint-color` on `.rt-desk`
 *    - Center: Fire / Magic / Flora → swaps `Hearth`'s centerpiece
 *
 *  Light swatches apply as translucent overlays (soft-light blend) —
 *  the warm wood gradient and perspective underneath stay visible.
 *  Salvaged at M2 from legacy `app/index.html` .theme-picker.       */

import { useEffect, useRef, useState } from 'react';
import type { Centerpiece } from './Hearth';

export type RoomTheme = 'classic' | 'study' | 'workshop' | 'observatory' | 'dark' | 'light';
export type DeskTheme = 'warm' | 'walnut' | 'ember' | 'terminal' | 'star' | 'parchment' | 'dark' | 'light';

export const ROOM_THEMES: ReadonlyArray<{ id: RoomTheme; label: string }> = [
  { id: 'classic', label: 'Tavern' },
  { id: 'study', label: 'Study' },
  { id: 'workshop', label: 'Workshop' },
  { id: 'observatory', label: 'Stars' },
  { id: 'dark', label: 'Dark' },
  { id: 'light', label: 'Light' },
];

export const DESK_THEMES: ReadonlyArray<{ id: DeskTheme; label: string }> = [
  { id: 'warm', label: 'Wood' },
  { id: 'walnut', label: 'Walnut' },
  { id: 'ember', label: 'Ember' },
  { id: 'terminal', label: 'Terminal' },
  { id: 'star', label: 'Stars' },
  { id: 'parchment', label: 'Draft' },
  { id: 'dark', label: 'Dark' },
  { id: 'light', label: 'Light' },
];

/** Swatch palette (hex) — preserved from legacy picker so the feel of
 *  the six warm-dark / cream options stays consistent. */
export const ROOM_SWATCHES: ReadonlyArray<{ color: string; label: string }> = [
  { color: '#C5BBAA', label: 'Amber' },
  { color: '#8AAE8E', label: 'Sage' },
  { color: '#94BDE0', label: 'Sky' },
  { color: '#B8ADD4', label: 'Violet' },
  { color: '#2C2720', label: 'Hearth' },
];

export const DESK_SWATCHES: ReadonlyArray<{ color: string; label: string }> = [
  { color: '#AAA397', label: 'Brass' },
  { color: '#7A9E7E', label: 'Moss' },
  { color: '#BA8A73', label: 'Copper' },
  { color: '#6B8CA8', label: 'Moon' },
  { color: '#2A2119', label: 'Walnut' },
];

const CENTERS: ReadonlyArray<{ id: Centerpiece; label: string; glyph: string }> = [
  { id: 'fire',  label: 'Fire',  glyph: '🔥' },
  { id: 'magic', label: 'Magic', glyph: '✨' },
  { id: 'flora', label: 'Flora', glyph: '🌸' },
];


export interface ColorPickerProps {
  roomColor: string;
  deskColor: string;
  roomTheme: RoomTheme;
  deskTheme: DeskTheme;
  centerpiece: Centerpiece;
  onChangeRoom: (color: string) => void;
  onChangeDesk: (color: string) => void;
  onChangeRoomTheme: (theme: RoomTheme) => void;
  onChangeDeskTheme: (theme: DeskTheme) => void;
  onChangeCenter: (c: Centerpiece) => void;
}

export function ColorPicker({
  roomColor,
  deskColor,
  roomTheme,
  deskTheme,
  centerpiece,
  onChangeRoom,
  onChangeDesk,
  onChangeRoomTheme,
  onChangeDeskTheme,
  onChangeCenter,
}: ColorPickerProps) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  // Esc + outside-click close.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        setOpen(false);
      }
    };
    const onMouseDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('mousedown', onMouseDown);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('mousedown', onMouseDown);
    };
  }, [open]);

  return (
    <div
      ref={wrapRef}
      className="picker-drawer"
      data-testid="theme-picker"
    >
      <button
        type="button"
        className={`picker-trigger ${open ? 'open' : ''}`}
        aria-label="Room decoration"
        title="Room decoration"
        aria-expanded={open}
        data-testid="picker-trigger"
        onClick={() => setOpen((v) => !v)}
      >
        <svg viewBox="0 0 557.064 557.064" fill="currentColor" aria-hidden>
          <path d="M379.333,407.754l15.807-10.768l3.949-2.695l-56.543-82.975l146.191-99.631c6.589-4.494,10.365-9.945,11.227-16.218c1.291-9.486-4.906-16.983-4.935-16.983l-47.583-69.663c-0.229-0.545-5.795-13.167-18.848-13.167c-4.857,0-9.993,1.817-15.262,5.412l-17.098,11.657c1.052,1.205,2.104,2.362,3.155,3.625c3.969,4.743,7.908,9.84,11.59,15.243l0,0c0.899,1.329,1.77,2.668,2.63,4.006l11.925-8.128l43.079,63.208l-146.183,99.632c-6.254,4.255-9.926,9.523-10.882,15.645c-1.511,9.609,4.504,17.488,4.59,17.574l59.23,86.924L379.333,407.754z" />
          <path d="M125.362,332.039c-0.937-1.271-1.884-2.543-2.792-3.883l0,0c-3.749-5.508-6.942-11.092-9.677-16.553c-0.746-1.492-1.348-2.936-2.027-4.398L85.4,324.561l16.151,23.705L125.362,332.039z" />
          <path d="M387.584,436.844l78.824,115.668c0,0,10.786,10.537,26.153-0.449c15.481-11.074,6.148-21.287,6.148-21.287l-82.84-116.672l15.807-10.768l-8.568-13.902l-22.586,15.461l-15.807,10.768l-23.706,16.152l10.768,15.807L387.584,436.844z" />
          <path d="M395.187,142.367L395.187,142.367c-3.682-5.403-7.688-10.471-11.705-15.176c-9.132-10.691-18.341-19.316-23.915-24.203L117.808,267.741c1.434,6.914,4.695,19.431,11.102,32.943c2.61,5.508,5.689,11.16,9.457,16.695l0,0c13.024,19.115,30.361,31.854,38.671,37.285l241.759-164.753C415.192,179.861,407.207,160.009,395.187,142.367z" />
          <path d="M321.365,63.62L283.564,4.752c-3.481-5.422-11.705-6.34-18.37-2.056s-9.247,12.145-5.766,17.557l0.966,1.501c-2.85,0.038-5.843,0.708-8.606,2.486c-6.665,4.284-9.247,12.144-5.766,17.557l8.396,13.082c-3.481-5.422-11.705-6.34-18.37-2.056c-6.665,4.284-9.247,12.145-5.766,17.557l-8.396-13.082c-3.481-5.422-11.705-6.34-18.37-2.056s-9.247,12.145-5.766,17.557l4.198,6.541c-3.48-5.422-11.705-6.34-18.37-2.056c-6.665,4.284-9.247,12.145-5.766,17.566l4.198,6.541c-3.48-5.422-11.704-6.34-18.369-2.056c-6.665,4.284-9.247,12.145-5.767,17.557l-8.405-13.082c-3.481-5.422-11.705-6.34-18.37-2.056c-6.665,4.284-9.247,12.145-5.766,17.557l8.396,13.082c-3.481-5.422-11.705-6.34-18.37-2.056c-2.955,1.894-5.049,4.504-6.225,7.287l-7.947-12.364c-3.48-5.422-11.704-6.34-18.369-2.056c-6.665,4.284-9.247,12.144-5.767,17.557l4.198,6.541c-3.481-5.422-11.705-6.34-18.37-2.056c-6.665,4.284-9.247,12.145-5.766,17.566l37.8,58.867c3.481,5.422,11.705,6.34,18.37,2.056s9.247-12.145,5.766-17.566l-4.198-6.541c3.481,5.422,11.705,6.34,18.37,2.056c2.955-1.893,5.049-4.504,6.225-7.287l7.946,12.364c3.481,5.422,11.705,6.34,18.37,2.056s9.247-12.145,5.766-17.557l-8.396-13.082c3.48,5.422,11.704,6.34,18.37,2.056c6.665-4.284,9.247-12.144,5.766-17.557l8.396,13.082c3.481,5.422,11.705,6.34,18.37,2.056c6.665-4.284,9.247-12.145,5.766-17.557l-4.198-6.541c3.481,5.422,11.705,6.34,18.37,2.056c6.665-4.284,9.247-12.144,5.766-17.566l-4.198-6.541c3.48,5.422,11.705,6.34,18.37,2.056s9.247-12.145,5.766-17.557l8.405,13.081c2.515,3.921,7.526,5.403,12.623,4.342c1.95-0.402,3.901-1.1,5.747-2.286c2.151-1.377,3.815-3.156,5.059-5.078c2.591-4.045,3.069-8.816,0.717-12.489l-8.396-13.082c3.48,5.422,11.704,6.34,18.369,2.056c6.665-4.284,9.247-12.145,5.767-17.557l-0.976-1.511c2.85-0.028,5.853-0.698,8.616-2.467c3.193-2.056,5.422-4.935,6.521-7.975C323.315,69.912,323.181,66.44,321.365,63.62z" />
        </svg>
        {/* Removed picker-trigger-cue dot — too easily read as a stray
            speck on the bristle brush icon. The popover already shows
            the active room swatch. */}
      </button>

      {open && (
        <div
          className="picker-popover"
          role="group"
          aria-label="Room, desk, and centerpiece picker"
          data-testid="picker-popover"
        >
          <div className="picker-row" role="radiogroup" aria-label="Room background">
            <span className="picker-label">Scene</span>
            {ROOM_THEMES.map(({ id, label }) => (
              <button
                key={id}
                type="button"
                className={`asset-dot room-${id} ${roomTheme === id ? 'active' : ''}`}
                title={label}
                aria-label={`Scene · ${label}`}
                aria-pressed={roomTheme === id}
                data-testid={`room-theme-${id}`}
                onClick={() => onChangeRoomTheme(id)}
              />
            ))}
          </div>
          <div className="picker-row" role="radiogroup" aria-label="Desk texture">
            <span className="picker-label">Table</span>
            {DESK_THEMES.map(({ id, label }) => (
              <button
                key={id}
                type="button"
                className={`asset-dot table-${id} ${deskTheme === id ? 'active' : ''}`}
                title={label}
                aria-label={`Table · ${label}`}
                aria-pressed={deskTheme === id}
                data-testid={`desk-theme-${id}`}
                onClick={() => onChangeDeskTheme(id)}
              />
            ))}
          </div>
          <div className="picker-row" role="radiogroup" aria-label="Room light">
            <span className="picker-label wide">Room Light</span>
            {ROOM_SWATCHES.map(({ color, label }) => (
              <button
                key={color}
                type="button"
                className={`color-dot ${roomColor === color ? 'active' : ''}`}
                style={{
                  background: color,
                  ...(label === 'White' ? { borderColor: '#ccc' } : null),
                }}
                title={label}
                aria-label={`Room light · ${label}`}
                aria-pressed={roomColor === color}
                data-testid={`room-swatch-${label.toLowerCase()}`}
                onClick={() => onChangeRoom(color)}
              />
            ))}
          </div>
          <div className="picker-row" role="radiogroup" aria-label="Desk light">
            <span className="picker-label wide">Desk Light</span>
            {DESK_SWATCHES.map(({ color, label }) => (
              <button
                key={color}
                type="button"
                className={`color-dot ${deskColor === color ? 'active' : ''}`}
                style={{
                  background: color,
                  ...(label === 'White' ? { borderColor: '#ccc' } : null),
                }}
                title={label}
                aria-label={`Desk light · ${label}`}
                aria-pressed={deskColor === color}
                data-testid={`desk-swatch-${label.toLowerCase()}`}
                onClick={() => onChangeDesk(color)}
              />
            ))}
          </div>
          <div className="picker-row" role="radiogroup" aria-label="Centerpiece">
            <span className="picker-label">Center</span>
            {CENTERS.map(({ id, label, glyph }) => (
              <button
                key={id}
                type="button"
                className={`cp-opt ${centerpiece === id ? 'active' : ''}`}
                aria-label={`Centerpiece · ${label}`}
                aria-pressed={centerpiece === id}
                data-testid={`center-${id}`}
                onClick={() => onChangeCenter(id)}
              >
                <span aria-hidden>{glyph}</span> {label}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
