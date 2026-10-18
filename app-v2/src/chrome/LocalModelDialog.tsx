import { useEffect, useId, useRef, type ReactNode, type RefObject } from 'react';
import { createPortal } from 'react-dom';

/** Shared Local picker/editor shell. The caller owns staged or immediate saving. */
export function LocalModelDialog({ title, subtitle, art, titleAction, children, footer, onClose, onEscape, returnFocus, closeLabel = 'Close model picker' }: {
  title: string; subtitle?: string; art?: ReactNode; titleAction?: ReactNode; children: ReactNode; footer?: ReactNode; onClose: () => void; onEscape?: () => void; closeLabel?: string;
  returnFocus?: RefObject<HTMLElement>;
}) {
  const titleId = useId();
  const panel = useRef<HTMLElement>(null);
  const previous = useRef(document.activeElement instanceof HTMLElement ? document.activeElement : null);
  const close = useRef(onClose);
  close.current = onClose;
  const escape = useRef(onEscape ?? onClose);
  escape.current = onEscape ?? onClose;
  useEffect(() => {
    const element = panel.current;
    (element?.querySelector<HTMLElement>('[data-initial-focus]') ?? element)?.focus();
    // Tavern and agent profiles own Escape at document capture. Take it first,
    // as the existing LocalAuthLayer does, so an editor never closes its parent.
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.isComposing) return;
      // Apply and key dialogs can sit above an editor. Only the top dialog
      // consumes Escape; its parent must keep the staged draft.
      if (document.querySelectorAll('.local-model-dialog').item(document.querySelectorAll('.local-model-dialog').length - 1) !== element) return;
      event.preventDefault(); event.stopPropagation(); escape.current();
    };
    window.addEventListener('keydown', onKey, true);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      // A menu option can disappear as it opens the editor. Fall back to the
      // owner's model trigger instead of leaving focus on the document body.
      const target = previous.current?.isConnected ? previous.current : returnFocus?.current;
      if (target?.isConnected) target.focus();
    };
  }, []);
  return createPortal(<div className="local-model-backdrop" onWheel={event => event.stopPropagation()} onTouchMove={event => event.stopPropagation()} onPointerDown={event => {
    if (event.target === event.currentTarget) close.current();
  }}>
    <section ref={panel} className="local-model-dialog" role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1}
      onKeyDown={event => {
        if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close.current(); return; }
        if (event.key !== 'Tab') return;
        const controls = [...(panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex="0"]') ?? [])]
          .filter(control => !control.closest('[hidden]'));
        const first = controls[0], last = controls[controls.length - 1];
        if (!first) { event.preventDefault(); panel.current?.focus(); }
        else if (event.shiftKey && (document.activeElement === first || document.activeElement === panel.current)) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
      }}>
      <header className="local-model-dialog-head"><div className="local-model-title">{art}<div>{titleAction ? <div className="local-model-title-line"><h2 id={titleId}>{title}</h2>{titleAction}</div> : <h2 id={titleId}>{title}</h2>}{subtitle && <p>{subtitle}</p>}</div></div>
        <button type="button" className="local-model-close" aria-label={closeLabel} onClick={onClose}>×</button>
      </header>
      {children}
      {footer && <footer className="local-model-dialog-foot">{footer}</footer>}
    </section>
  </div>, document.body);
}
