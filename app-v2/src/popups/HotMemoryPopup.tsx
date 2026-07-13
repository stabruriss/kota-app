import { useState } from 'react';
import { SCENES } from '../mock/fixtures';
import type { HotMemTile } from '../types/scene';
import { useEsc } from './useEsc';

/** Screen 09 — Hot Memory grid (per Memory System Design §1.9).
 *  M1 renders from mock fixtures. M6 replaces the data source with
 *  `project-memory/hot_memory.md` parsed by Violet. */
export function HotMemoryPopup({ onClose }: { onClose: () => void }) {
  useEsc(onClose);
  const hm = SCENES.conversation!.hotMem;
  const [hover, setHover] = useState<HotMemTile>(hm.tiles[0]!);

  // Heat → color (brass → terra). Blends from cool cream to terra-hi.
  const heatBg = (h: number) => {
    const r = Math.round(0xC4 * h + 0x4A * (1 - h));
    const g = Math.round(0x78 * h + 0x3C * (1 - h));
    const b = Math.round(0x5A * h + 0x28 * (1 - h));
    return `rgba(${r},${g},${b},${0.25 + h * 0.65})`;
  };

  return (
    <div className="pop-scrim" onClick={onClose}>
      <div className="pop pop-hotmem" onClick={(e) => e.stopPropagation()}>
        <div className="pop-head">
          <span className="pop-fire">🔥</span>
          <div>
            <div className="pop-title">Hot Memory</div>
            <div className="pop-sub">
              {hm.tiles.length} records · heat gradient · click a tile to pin for later
            </div>
          </div>
          <button className="pop-close" onClick={onClose} aria-label="Close">×</button>
        </div>
        <div className="pop-hm-body">
          <div className="hm-grid">
            {hm.tiles.map((t, i) => (
              <div
                key={i}
                className={`hm-tile ${hover === t ? 'active' : ''}`}
                style={{ background: heatBg(t.heat) }}
                onMouseEnter={() => setHover(t)}
              >
                <div className="hm-tile-head">
                  <span className="hm-tile-who">{t.who}</span>
                  <span className="hm-tile-hits">{t.hits}×</span>
                </div>
                <div
                  className={`hm-tile-label ${
                    t.kind === 'quote' ? 'quote' :
                    t.kind === 'file' || t.kind === 'code' ? 'mono' : ''
                  }`}
                >
                  {t.label}
                </div>
                <div className="hm-tile-meta">
                  <span className={`hm-kind hm-kind--${t.kind}`}>{t.kind}</span>
                  <span className="hm-tile-last">{t.last}</span>
                </div>
              </div>
            ))}
          </div>
          <aside className="hm-detail">
            <div className="hm-detail-kind">{hover.kind}</div>
            <div className="hm-detail-label">{hover.label}</div>
            <div className="hm-detail-stat">
              <b>{hover.hits}×</b> referenced · last seen {hover.last} · by <b>{hover.who}</b>
            </div>
            <div className="hm-detail-note">{hover.note}</div>
            <div className="hm-heat-legend">
              <span>cool</span>
              <span className="hm-heat-bar" />
              <span>hot</span>
            </div>
          </aside>
        </div>
      </div>
    </div>
  );
}
