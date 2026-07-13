import type { AgentId } from '../types/scene';
import type { WorkingHero } from '../types/agentbar';
import { avatarClassForCli, avatarImageStyleForId } from '../lib/hero-avatars';
import { splitProjectAgentName } from './ProjectAgentName';

export function WorkingHeroPicker({
  heroes,
  liveAgents,
  unavailableHeroIds,
  onSelect,
  testIdPrefix,
  variant = 'orbit',
  emptyLabel = 'No active working heroes',
  showHotkeys = false,
  disabled = false,
}: {
  heroes: readonly WorkingHero[];
  liveAgents?: ReadonlySet<AgentId>;
  unavailableHeroIds?: ReadonlySet<AgentId>;
  onSelect: (hero: WorkingHero) => void;
  testIdPrefix: string;
  variant?: 'orbit' | 'modal';
  emptyLabel?: string;
  showHotkeys?: boolean;
  disabled?: boolean;
}) {
  const unavailable = new Set<AgentId>();
  liveAgents?.forEach((id) => unavailable.add(id));
  unavailableHeroIds?.forEach((id) => unavailable.add(id));
  const rootClass = variant === 'modal'
    ? 'working-hero-picker modal'
    : `working-hero-orbit ${heroes.length > 4 ? 'many' : ''}`;
  return (
    <div className={rootClass} data-testid={`${testIdPrefix}-picker`}>
      {heroes.length === 0 ? (
        <div className="working-hero-empty">{emptyLabel}</div>
      ) : (
        heroes.map((hero, index) => {
          const unavailableReason = hero.available === false
            ? hero.unavailableReason ?? 'CLI unavailable'
            : unavailable.has(hero.id)
              ? 'already incarnated in this project'
              : null;
          const isUnavailable = unavailableReason != null;
          const isDisabled = disabled || isUnavailable;
          const nameParts = splitProjectAgentName(hero.name);
          return (
            <button
              key={hero.id}
              type="button"
              className={[
                'working-hero-pill',
                variant === 'orbit' ? `p${index}` : '',
                isUnavailable ? 'unavailable' : '',
                disabled ? 'pending' : '',
              ].filter(Boolean).join(' ')}
              title={unavailableReason ? `${hero.name} (${unavailableReason})` : hero.name}
              data-full-name={hero.name}
              onClick={(event) => {
                event.stopPropagation();
                if (isDisabled) return;
                onSelect(hero);
              }}
              disabled={isDisabled}
              data-testid={`${testIdPrefix}-${hero.id}`}
              aria-label={
                isUnavailable
                  ? `${hero.name} is unavailable: ${unavailableReason}`
                  : disabled
                    ? `Incarnating ${hero.name}`
                  : `Incarnate ${hero.name}`
              }
            >
              <span
                className={`working-hero-avatar tavern-avatar-art ${hero.avatarClass ?? providerAvatarClass(hero.cli)}`}
                style={avatarImageStyleForId(hero.avatarId)}
                aria-hidden
              >
                <span />
                <i />
                <b />
              </span>
              <span className="working-hero-copy">
                {nameParts.title && (
                  <span className={`working-hero-name-title title-${nameParts.title.id}`}>
                    {nameParts.title.abbr}
                  </span>
                )}
                <b>{nameParts.base}</b>
              </span>
              {showHotkeys && index < 9 && <kbd className="working-hero-hotkey">{index + 1}</kbd>}
            </button>
          );
        })
      )}
    </div>
  );
}

function providerAvatarClass(cli: WorkingHero['cli']): string {
  return avatarClassForCli(cli);
}
