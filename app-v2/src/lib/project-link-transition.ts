import { useEffect, useLayoutEffect, useRef, useState, type TransitionEvent } from 'react';

type Phase = 'idle' | 'moving' | 'popup' | 'closing' | 'returning';
const reducedMotion = () => window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;

/** Only previews the switch. It never writes project connection settings. */
export function useProjectLinkTransition(linked: boolean) {
  const [preview, setPreview] = useState(linked);
  const [phase, setPhase] = useState<Phase>('idle');
  const previous = useRef(linked);
  const trigger = useRef<HTMLButtonElement>(null);

  useLayoutEffect(() => {
    if (previous.current === linked) return;
    previous.current = linked;
    setPreview(linked); setPhase('idle');
  }, [linked]);

  useEffect(() => {
    if (phase !== 'moving' && phase !== 'returning') return;
    const done = () => setPhase(current => current === 'moving' ? 'popup' : current === 'returning' ? 'idle' : current);
    // Normal motion finishes on transitionend. The timer is only a missing-event
    // fallback; reduced motion still gives the preview a frame before the popup.
    if (reducedMotion()) {
      const frame = requestAnimationFrame(done);
      return () => cancelAnimationFrame(frame);
    }
    const timer = window.setTimeout(done, 300);
    return () => window.clearTimeout(timer);
  }, [phase]);

  useLayoutEffect(() => {
    if (phase !== 'closing') return;
    let second = 0;
    const first = requestAnimationFrame(() => {
      second = requestAnimationFrame(() => { setPreview(linked); setPhase('returning'); });
    });
    return () => { cancelAnimationFrame(first); cancelAnimationFrame(second); };
  }, [phase, linked]);

  const previousPhase = useRef<Phase>('idle');
  useEffect(() => {
    if (phase === 'idle' && (previousPhase.current === 'returning' || previousPhase.current === 'popup') && document.activeElement === document.body) trigger.current?.focus();
    previousPhase.current = phase;
  }, [phase]);

  const onTransitionEnd = (event: TransitionEvent<HTMLButtonElement>) => {
    if (event.propertyName !== 'transform' || !(event.target instanceof Element) || !event.target.classList.contains('project-github-thumb')) return;
    setPhase(current => current === 'moving' ? 'popup' : current === 'returning' ? 'idle' : current);
  };
  return {
    preview, phase, trigger, onTransitionEnd,
    toggle: () => { if (phase === 'idle') { setPreview(!linked); setPhase('moving'); } },
    cancel: () => setPhase('closing'),
    complete: (next: boolean) => { setPreview(next); setPhase('idle'); },
  };
}
