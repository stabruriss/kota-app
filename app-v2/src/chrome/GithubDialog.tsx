import { useEffect, useLayoutEffect, useRef, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import '../styles/project-connection.css';

/** A project dialog is above Settings/Link; only its topmost layer handles keys. */
export function GithubDialog({ title, confirm = false, busy = false, onCancel, children }: {
  title: string; confirm?: boolean; busy?: boolean; onCancel: () => void; children: ReactNode;
}) {
  const layer = useRef<HTMLDivElement>(null);
  const card = useRef<HTMLElement>(null);
  useLayoutEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    card.current?.querySelector<HTMLElement>('[data-dialog-focus]')?.focus();
    return () => {
      if (previous?.isConnected && (document.activeElement === document.body || card.current?.contains(document.activeElement))) previous.focus();
    };
  }, []);
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      const layers = document.querySelectorAll('.github-dialog-layer');
      if (layers[layers.length - 1] !== layer.current) return;
      if (event.key === 'Escape') {
        event.preventDefault(); event.stopImmediatePropagation();
        if (!busy) onCancel();
      } else if (event.key === 'Tab') {
        const items = Array.from(card.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), [tabindex="0"]') ?? []);
        const index = items.indexOf(document.activeElement as HTMLElement);
        event.preventDefault(); event.stopImmediatePropagation();
        items[(index + (event.shiftKey ? -1 : 1) + items.length) % items.length]?.focus();
      }
    };
    window.addEventListener('keydown', keydown, true);
    return () => window.removeEventListener('keydown', keydown, true);
  }, [busy, onCancel]);
  return createPortal(<div ref={layer} className="kota-confirm-layer github-dialog-layer" onPointerDown={event => {
    event.stopPropagation();
    if (event.target === event.currentTarget && !busy) onCancel();
  }}>
    <section ref={card} className={`kota-confirm-card plain-copy ${confirm ? 'github-confirm-card' : 'github-connect-card'}`}
      role={confirm ? 'alertdialog' : 'dialog'} aria-label={title} aria-modal="true" aria-busy={busy}>
      <header className="github-dialog-head"><h2>{title}</h2>
        <button type="button" aria-label="Close GitHub dialog" disabled={busy} onClick={onCancel}>×</button></header>
      {children}
    </section>
  </div>, document.body);
}
