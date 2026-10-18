import { memo } from 'react';
import type { LocalViewBlock } from '../lib/local-rpc';
import { LocalWorkbenchText } from './LocalWorkbenchText';
import flame from '../assets/tavern/icons/tavern-entry.svg';

export const localElapsedLabel = (value: string | null | undefined, now: number) => {
  if (!value || !Number.isFinite(Date.parse(value))) return '';
  const seconds = Math.max(0, Math.floor((now - Date.parse(value)) / 1000));
  return `${Math.floor(seconds / 60).toString().padStart(2, '0')}:${(seconds % 60).toString().padStart(2, '0')}`;
};

/** The parent and copies share the same bounded stream block DOM. */
export const LocalStreamBlock = memo(function LocalStreamBlock({ block, now }: { block: LocalViewBlock; now?: number }) {
  const elapsed = now === undefined ? '' : localElapsedLabel(block.startedAt, now);
  return <article className={`stream-block type-${block.kind}${block.active ? ' active' : ''}`} data-stream-block={block.id}>
    {block.kind === 'end'
      ? <h3 className="end-heading"><img src={flame} alt="" />{block.error ? 'Response error' : 'Answer'}</h3>
      : <h3 className="step-head"><span className="step-kind">{block.kind}</span><span className="step-summary">{block.label}</span>{block.error && <span>Failed</span>}{elapsed && <time className="step-time" aria-label="Thinking elapsed">{elapsed}</time>}</h3>}
    <pre className="stream-content">{block.kind === 'tool' ? block.text : <LocalWorkbenchText text={block.text} active={block.active} />}</pre>
    {block.truncated && <small className="stream-limit">Display limit reached.</small>}
  </article>;
});
