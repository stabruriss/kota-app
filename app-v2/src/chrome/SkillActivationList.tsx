import { useEffect, useState } from 'react';
import type { MouseEvent } from 'react';
import type { SkillLoomEntry } from '../lib/account-skills';
import { SkillDescription } from './SkillDescription';

interface SkillActivationListProps {
  entries: SkillLoomEntry[];
  /** Lists only the active skills, as plain rows without checkboxes. */
  readOnly?: boolean;
  disabled?: boolean;
  onChange: (skillId: string, active: boolean) => void;
  onOpenSkillFolder?: (skill: SkillLoomEntry) => void | Promise<void>;
}

export function SkillActivationList({
  entries,
  readOnly = false,
  disabled = false,
  onChange,
  onOpenSkillFolder,
}: SkillActivationListProps) {
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    skill: SkillLoomEntry;
  } | null>(null);

  useEffect(() => {
    if (!contextMenu) return undefined;
    const close = () => setContextMenu(null);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') close();
    };
    window.addEventListener('pointerdown', close);
    window.addEventListener('keydown', onKeyDown);
    return () => {
      window.removeEventListener('pointerdown', close);
      window.removeEventListener('keydown', onKeyDown);
    };
  }, [contextMenu]);

  const openContextMenu = (event: MouseEvent, skill: SkillLoomEntry) => {
    if (!onOpenSkillFolder || skill.missing || !skill.path) return;
    event.preventDefault();
    event.stopPropagation();
    setContextMenu({ x: event.clientX, y: event.clientY, skill });
  };

  return (
    <div className="tavern-skill-list">
      {(readOnly ? entries.filter((skill) => skill.selected) : entries).map((skill) => {
        const description = skill.description || skill.error || '';
        const path = skill.path || 'SHELL.yaml only';
        const Row = readOnly ? 'div' : 'label';
        return (
          <Row
            key={skill.id}
            className={[
              'tavern-skill-row',
              skill.selected ? 'active' : '',
              skill.missing ? 'missing' : '',
              readOnly ? 'static' : '',
            ].filter(Boolean).join(' ')}
            onContextMenu={(event: MouseEvent) => openContextMenu(event, skill)}
          >
            {readOnly ? <span className="tavern-skill-tick" aria-hidden="true">✓</span> : (
              <input
                type="checkbox"
                checked={skill.selected}
                disabled={disabled}
                onChange={(event) => onChange(skill.id, event.currentTarget.checked)}
                aria-label={`${skill.name} skill`}
              />
            )}
            <span className="tavern-skill-row-copy">
              <span className="tavern-skill-row-title">
                <b>{skill.name}{skill.missing ? ' (missing)' : ''}</b>
              </span>
              <SkillDescription
                className="tavern-skill-description"
                text={description}
                fallback={path}
              />
            </span>
          </Row>
        );
      })}
      {contextMenu && (
        <div
          className="st-ctx-menu tavern-skill-context-menu"
          role="menu"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          onPointerDown={(event) => event.stopPropagation()}
        >
          <button
            type="button"
            role="menuitem"
            className="st-ctx-row"
            onClick={() => {
              const skill = contextMenu.skill;
              setContextMenu(null);
              void onOpenSkillFolder?.(skill);
            }}
          >
            Open skill folder
          </button>
        </div>
      )}
    </div>
  );
}
