import { useEffect, useState } from 'react';
import type { MouseEvent } from 'react';
import type { SkillLoomEntry } from '../lib/account-skills';
import { SkillDescription } from './SkillDescription';

interface SkillActivationListProps {
  entries: SkillLoomEntry[];
  disabled?: boolean;
  onChange: (skillId: string, active: boolean) => void;
  onOpenSkillFolder?: (skill: SkillLoomEntry) => void | Promise<void>;
}

export function SkillActivationList({
  entries,
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
      {entries.map((skill) => {
        const description = skill.description || skill.error || '';
        const path = skill.path || 'SHELL.yaml only';
        const state = skill.missing ? 'missing' : skill.selected ? 'active' : 'deactive';
        return (
          <label
            key={skill.id}
            className={[
              'tavern-skill-row',
              skill.selected ? 'active' : '',
              skill.missing ? 'missing' : '',
            ].filter(Boolean).join(' ')}
            onContextMenu={(event) => openContextMenu(event, skill)}
          >
            <input
              type="checkbox"
              checked={skill.selected}
              disabled={disabled}
              onChange={(event) => onChange(skill.id, event.currentTarget.checked)}
              aria-label={`${skill.name} skill`}
            />
            <span className="tavern-skill-row-copy">
              <span className="tavern-skill-row-title">
                <b>{skill.name}{skill.missing ? ' (missing)' : ''}</b>
                <span className={`tavern-skill-state-pill ${state}`}>{state}</span>
              </span>
              <SkillDescription
                className="tavern-skill-description"
                text={description}
                fallback={path}
              />
              <code title={path}>{path}</code>
            </span>
          </label>
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
