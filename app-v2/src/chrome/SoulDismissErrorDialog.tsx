import { useEffect, useId, useLayoutEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import '../styles/soul-dismiss.css';

export type SoulDismissChoice = 'cancel' | 'retry' | 'force';

export function SoulDismissErrorDialog({
  name,
  onChoose,
}: {
  name: string;
  onChoose: (choice: SoulDismissChoice) => void;
}) {
  const cardRef = useRef<HTMLElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const answeredRef = useRef(false);
  const descriptionId = useId();
  const choose = (choice: SoulDismissChoice) => {
    if (answeredRef.current) return;
    answeredRef.current = true;
    onChoose(choice);
  };
  useLayoutEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    cancelRef.current?.focus();
    return () => {
      if (previous?.isConnected && (document.activeElement === document.body || cardRef.current?.contains(document.activeElement))) previous.focus();
    };
  }, []);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault(); event.stopImmediatePropagation();
        choose('cancel');
      } else if (event.key === 'Tab') {
        const buttons = Array.from(cardRef.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []);
        const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
        event.preventDefault();
        const next = current === -1
          ? (event.shiftKey ? buttons.length - 1 : 0)
          : (current + (event.shiftKey ? -1 : 1) + buttons.length) % buttons.length;
        buttons[next]?.focus();
        event.stopImmediatePropagation();
      }
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  });

  return createPortal(
    <div className="kota-confirm-layer soul-dismiss-layer" role="presentation" onMouseDown={() => choose('cancel')}>
      <section ref={cardRef} className="kota-confirm-card danger plain-copy" role="alertdialog"
        aria-modal="true" aria-label="Delete archived agent?" aria-describedby={`${descriptionId}-body ${descriptionId}-error`}
        onMouseDown={event => event.stopPropagation()}>
        <h2>Delete archived agent?</h2>
        <pre id={`${descriptionId}-body`}>{name}'s workspace will be permanently deleted. This cannot be undone.</pre>
        <p id={`${descriptionId}-error`} className="soul-dismiss-error" role="alert">Could not save this incarnation's soul to the Tavern.</p>
        <div className="kota-confirm-actions soul-dismiss-actions">
          <button ref={cancelRef} type="button" onClick={() => choose('cancel')}>Cancel</button>
          <button type="button" className="discard" onClick={() => choose('force')}>Dismiss anyway</button>
          <button type="button" className="confirm" onClick={() => choose('retry')}>Retry</button>
        </div>
      </section>
    </div>, document.body,
  );
}
