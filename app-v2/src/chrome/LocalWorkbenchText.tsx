import { useState } from 'react';

const graphemes = typeof Intl.Segmenter === 'function'
  ? new Intl.Segmenter(undefined, { granularity: 'grapheme' }) : null;

function appendStart(previous: string, text: string): number {
  // Replacements are authoritative, not a new typing queue. Without a
  // grapheme segmenter, prefer immediate plain text over splitting an emoji.
  if (!graphemes || !text.startsWith(previous)) return text.length;
  return graphemes.segment(text).containing(previous.length)?.index ?? text.length;
}

/** At most one new-fragment span. No per-character DOM, timers or delayed text. */
export function LocalWorkbenchText({ text, active }: { text: string; active: boolean }) {
  const [paint, setPaint] = useState(() => ({ text, active, start: text.length }));
  if (paint.text !== text || paint.active !== active) {
    // Derive before committing children: unrelated parent renders must not
    // re-animate the last fragment. Opening a snapshot starts fully visible.
    setPaint({ text, active, start: active && paint.text !== text ? appendStart(paint.text, text) : text.length });
  }
  if (!active || paint.start >= text.length) return <>{text}</>;
  return <>{text.slice(0, paint.start)}<span key={text.length} className="stream-append">{text.slice(paint.start)}</span></>;
}
