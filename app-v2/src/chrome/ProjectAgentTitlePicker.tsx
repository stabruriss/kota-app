import { useEffect, useRef, useState } from 'react';
import { TITLE_DEFS, getTitleDef } from '../lib/agent-titles';

interface ProjectAgentTitlePickerProps {
  titleId: string | null;
  disabled?: boolean;
  onChange: (next: string | null) => void;
}

export function ProjectAgentTitlePicker({ titleId, disabled, onChange }: ProjectAgentTitlePickerProps) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const current = getTitleDef(titleId);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!wrapRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    window.addEventListener('pointerdown', onPointerDown);
    window.addEventListener('keydown', onKeyDown);
    return () => {
      window.removeEventListener('pointerdown', onPointerDown);
      window.removeEventListener('keydown', onKeyDown);
    };
  }, [open]);

  const select = (nextId: string | null) => {
    setOpen(false);
    onChange(nextId);
  };

  return (
    <div ref={wrapRef} className={`title-stamp ${open ? 'open' : ''} ${current ? 'has-title' : 'empty'}`}>
      {current ? (
        <>
          <button
            type="button"
            className={`title-stamp-text project-agent-name-title title-${current.id}`}
            onClick={() => !disabled && setOpen((v) => !v)}
            disabled={disabled}
            aria-label={`Title: ${current.full}. Click to change.`}
          >
            {current.full}
          </button>
          <button
            type="button"
            className="title-stamp-btn remove"
            onClick={() => !disabled && select(null)}
            disabled={disabled}
            aria-label="Remove title"
          >
            −
          </button>
        </>
      ) : (
        <button
          type="button"
          className="title-stamp-btn add"
          onClick={() => !disabled && setOpen((v) => !v)}
          disabled={disabled}
          aria-label="Add a title"
          aria-expanded={open}
        >
          +
        </button>
      )}
      {open && (
        <div className="title-stamp-menu" role="menu">
          <div className="title-stamp-menu-header">Choose a title</div>
          <div className="title-stamp-menu-list">
            {TITLE_DEFS.map((def) => (
              <button
                key={def.id}
                type="button"
                role="menuitem"
                className={`title-stamp-menu-item ${def.id === titleId ? 'selected' : ''}`}
                onClick={() => select(def.id)}
              >
                <span className={`title-stamp-menu-full project-agent-name-title title-${def.id}`}>{def.full}</span>
                <span className="title-stamp-menu-abbr">{def.abbr}</span>
              </button>
            ))}
          </div>
          {current && (
            <button
              type="button"
              className="title-stamp-menu-remove"
              onClick={() => select(null)}
            >
              − Remove title
            </button>
          )}
        </div>
      )}
    </div>
  );
}
