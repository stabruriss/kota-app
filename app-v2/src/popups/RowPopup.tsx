import { AGENTS } from '../mock/fixtures';
import type { LogRow } from '../types/scene';
import { useEsc } from './useEsc';

/** Row popup — expand a meeting-log row into the full group-chat
 *  transcript (ported from `popups.jsx`). */
export function RowPopup({ row, onClose }: { row: LogRow; onClose: () => void }) {
  useEsc(onClose);
  const thread = row.thread ?? [{ who: row.who, t: row.time, body: row.text }];

  return (
    <div className="pop-scrim" onClick={onClose}>
      <div className="pop pop-row" onClick={(e) => e.stopPropagation()}>
        <div className="pop-head">
          <div>
            <div className="pop-title">
              Turn · {row.who} · {row.time}
            </div>
            <div className="pop-sub">full group-chat transcript · {thread.length} parts</div>
          </div>
          <button className="pop-close" onClick={onClose} aria-label="Close">×</button>
        </div>
        <div className="pop-row-body">
          {thread.map((m, i) => {
            const agent = AGENTS[m.who.toLowerCase()];
            return (
              <div
                key={i}
                className={`chat-bubble ${m.think ? 'think' : ''} ${m.tool ? 'tool' : ''} ${m.side ? 'side' : ''}`}
              >
                <div className="cb-av">{agent?.emoji ?? '·'}</div>
                <div className="cb-body">
                  <div className="cb-head">
                    <span className="cb-who">{m.who}</span>
                    {m.think && <span className="cb-tag">thinking</span>}
                    {m.tool && (
                      <span className={`cb-tag tool ${m.ok ? 'ok' : m.warn ? 'warn' : ''}`}>
                        tool
                      </span>
                    )}
                    <span className="cb-time">{m.t}</span>
                  </div>
                  <div className="cb-text">{m.body}</div>
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
