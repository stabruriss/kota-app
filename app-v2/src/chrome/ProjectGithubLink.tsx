import type { ReactNode, Ref, TransitionEventHandler } from 'react';
import '../styles/project-github.css';

/** Shared appearance; existing-project changes are wired with confirmation separately. */
export function ProjectGithubLink({ linked, disabled, onChange, children, buttonRef, onTransitionEnd }: {
  linked: boolean; disabled?: boolean; onChange?: (linked: boolean) => void; children: ReactNode;
  buttonRef?: Ref<HTMLButtonElement>; onTransitionEnd?: TransitionEventHandler<HTMLButtonElement>;
}) {
  return <div className="project-github-link">
    <div className="project-github-caption"><strong>Link GitHub</strong><div>{children}</div></div>
    <button ref={buttonRef} onTransitionEnd={onTransitionEnd} type="button" className="project-github-switch" role="switch" aria-label="Link GitHub"
      aria-checked={linked} disabled={disabled || !onChange} onClick={() => onChange?.(!linked)}>
      <span className="project-github-on" aria-hidden>ON</span><span className="project-github-off" aria-hidden>OFF</span>
      <span className="project-github-thumb" aria-hidden>{Array.from({ length: 9 }, (_, i) => <i key={i} />)}</span>
    </button>
  </div>;
}
