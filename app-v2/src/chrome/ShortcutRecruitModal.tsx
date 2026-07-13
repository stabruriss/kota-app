import { useCallback, useEffect, useRef, useState } from 'react';
import type { WorkingHero } from '../types/agentbar';
import type { AgentId } from '../types/scene';
import { WorkingHeroPicker } from './WorkingHeroPicker';

export function ShortcutRecruitModal({
  seatNumber,
  heroes,
  unavailableHeroIds,
  onSelect,
  onDismiss,
}: {
  seatNumber: number;
  heroes: readonly WorkingHero[];
  unavailableHeroIds?: ReadonlySet<AgentId>;
  onSelect: (hero: WorkingHero) => void | Promise<void>;
  onDismiss: () => void;
}) {
  const pendingRef = useRef(false);
  const mountedRef = useRef(true);
  const [pending, setPending] = useState(false);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const selectHero = useCallback((hero: WorkingHero) => {
    if (pendingRef.current) return;
    pendingRef.current = true;
    setPending(true);
    Promise.resolve(onSelect(hero))
      .catch((err) => {
        console.warn('[kota-recruit] shortcut recruit failed', err);
      })
      .finally(() => {
        pendingRef.current = false;
        if (mountedRef.current) setPending(false);
      });
  }, [onSelect]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (pendingRef.current) {
        if (event.key === 'Escape' || /^[1-9]$/.test(event.key)) {
          event.preventDefault();
          event.stopPropagation();
        }
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        onDismiss();
        return;
      }
      if (/^[1-9]$/.test(event.key)) {
        const index = Number(event.key) - 1;
        const hero = heroes[index];
        if (!hero || hero.available === false || unavailableHeroIds?.has(hero.id)) return;
        event.preventDefault();
        event.stopPropagation();
        selectHero(hero);
      }
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, [heroes, onDismiss, selectHero, unavailableHeroIds]);

  return (
    <div
      className={`shortcut-recruit-layer ${pending ? 'pending' : ''}`}
      role="presentation"
      tabIndex={-1}
      onKeyDownCapture={(event) => {
        if (pending) {
          event.preventDefault();
          event.stopPropagation();
          return;
        }
        if (event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          onDismiss();
        }
      }}
      onClick={() => {
        if (!pending) onDismiss();
      }}
      data-testid="shortcut-recruit-modal"
      aria-busy={pending}
    >
      <div
        className="shortcut-recruit-panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby="shortcut-recruit-title"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="shortcut-recruit-head">
          <div>
            <div className="shortcut-recruit-kicker">Seat {seatNumber}</div>
            <div id="shortcut-recruit-title" className="shortcut-recruit-title">
              Add agent
            </div>
          </div>
          <button
            type="button"
            className="shortcut-recruit-close"
            onClick={onDismiss}
            disabled={pending}
            aria-label="Close add agent dialog"
          >
            x
          </button>
        </div>
        <WorkingHeroPicker
          heroes={heroes}
          unavailableHeroIds={unavailableHeroIds}
          onSelect={selectHero}
          testIdPrefix="incarnate-shortcut"
          variant="modal"
          showHotkeys
          disabled={pending}
        />
      </div>
    </div>
  );
}
