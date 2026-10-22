import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ProjectGithubLink } from '../src/chrome/ProjectGithubLink';
import { ProjectConnectionConfirm } from '../src/chrome/ProjectConnectionConfirm';
import { useProjectLinkTransition } from '../src/lib/project-link-transition';

function Fixture({ linked = true, change = vi.fn() }: { linked?: boolean; change?: () => void }) {
  const motion = useProjectLinkTransition(linked);
  return <><ProjectGithubLink linked={motion.preview} disabled={motion.phase !== 'idle'} buttonRef={motion.trigger}
    onChange={motion.toggle} onTransitionEnd={motion.onTransitionEnd}>Fixture</ProjectGithubLink>
    {motion.phase === 'popup' && <ProjectConnectionConfirm change={linked ? 'local' : 'connect'} name="Fixture" from="From fixture" to="To fixture"
      login="fixture" loginReady busy={false} onCancel={motion.cancel} onConfirm={change} />}
    <output>{motion.phase}</output></>;
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal('matchMedia', vi.fn(() => ({ matches: false })));
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });
const endTransition = () => {
  // jsdom has no TransitionEvent constructor; keep the browser's propertyName.
  const event = new Event('transitionend', { bubbles: true });
  Object.defineProperty(event, 'propertyName', { value: 'transform' });
  fireEvent(document.querySelector('.project-github-thumb')!, event);
};

it.each([true, false])('opens only after the switch transition, then closes the popup before returning (%s)', async linked => {
  const change = vi.fn(); render(<Fixture linked={linked} change={change} />);
  const toggle = screen.getByRole('switch');
  fireEvent.click(toggle);
  expect(toggle).toHaveAttribute('aria-checked', String(!linked));
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  act(() => vi.advanceTimersByTime(100));
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  endTransition();
  expect(screen.getByRole('alertdialog')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  expect(toggle).toHaveAttribute('aria-checked', String(!linked));
  act(() => vi.advanceTimersByTime(40));
  expect(toggle).toHaveAttribute('aria-checked', String(linked));
  expect(toggle).toBeDisabled();
  endTransition();
  expect(toggle).toBeEnabled();
  expect(change).not.toHaveBeenCalled();
});

it('handles reduced motion in the same order and contains Escape within the popup', () => {
  vi.mocked(window.matchMedia).mockReturnValue({ matches: true } as MediaQueryList);
  const escaped = vi.fn(); document.addEventListener('keydown', escaped, true);
  try {
    render(<Fixture />); const toggle = screen.getByRole('switch');
    fireEvent.click(toggle);
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    act(() => vi.advanceTimersByTime(20));
    expect(screen.getByRole('alertdialog')).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole('button', { name: 'Cancel' }), { key: 'Escape' });
    expect(escaped).not.toHaveBeenCalled();
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(toggle).toHaveAttribute('aria-checked', 'false');
    act(() => vi.advanceTimersByTime(80));
    expect(toggle).toHaveAttribute('aria-checked', 'true');
    act(() => vi.advanceTimersByTime(20));
    expect(toggle).toBeEnabled();
  } finally { document.removeEventListener('keydown', escaped, true); }
});
