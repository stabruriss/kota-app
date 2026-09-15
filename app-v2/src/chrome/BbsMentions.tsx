import { bbsMentionDeviceKey } from '../bbs-mentions';
import { BbsMentionComposer, type BbsMentionComposerProps } from './BbsMentionComposer';
import { BbsRosterAvatar } from './BbsRosterAvatar';
import { useBbsRoster } from './useBbsRoster';
import { useBbsSharing } from './BbsSyncControls';

const renderAvatar: NonNullable<BbsMentionComposerProps['renderAvatar']> = (agent, device) =>
  <BbsRosterAvatar deviceId={bbsMentionDeviceKey(device)} name={agent.name} avatar={agent.avatar} />;

/** Roster/avatar updates stop at this leaf. The supplied editor isn't recreated
 * by pages, online changes, image arrival, or sync byte-progress. */
export function BbsMentions({ kind, sharingGroupId, ...props }: Omit<BbsMentionComposerProps,
  'devices' | 'loading' | 'error' | 'onRetry' | 'remoteAllowed' | 'renderAvatar'> & {
  kind: 'topic' | 'reply'; sharingGroupId?: string | null;
}) {
  const roster = useBbsRoster(props.open);
  const { groupId } = useBbsSharing();
  const remoteAllowed = groupId !== null && (kind === 'topic' || sharingGroupId === groupId);
  return <BbsMentionComposer {...props} devices={roster.view?.devices} loading={roster.loading}
    error={roster.error} onRetry={roster.refresh} remoteAllowed={remoteAllowed} renderAvatar={renderAvatar} />;
}
