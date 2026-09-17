import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { BbsSyncScope, BbsSyncControlButton, BbsSyncActivity, BbsThreadSharing } from '../../src/chrome/BbsSyncControls';
import type { BbsSyncView } from '../../src/bbs-sync-view';
import { BBS_SYNC_ERROR_DETAILS } from '../../src/bbs-sync-errors';
import { BBS_SYNC_INDICATORS, type BbsSyncIndicator } from '../../src/types/bbs-sync';
import '../../src/styles/kota-tokens.css';
import '../../src/styles/canvas.css';

const initial: BbsSyncView = {
  deviceId: 'self', deviceName: 'MacBook Pro', workerAvailable: true,
  group: { id: 'demo', name: 'Studio', role: 'owner', members: [
    { id: 'self', name: 'MacBook Pro', role: 'owner', online: true },
    { id: 'peer', name: 'Mac Studio', role: 'member', online: true },
    { id: 'travel', name: 'Travel Mac', role: 'member', online: false },
  ] },
  invitation: { state: 'preparing' }, invitationGeneration: '1',
  phase: 'idle', progress: null, lastSuccessfulAt: '2026-09-12T06:00:00Z', error: null, controlRecoverable: false, serviceRecoverable: false, indicator: 'healthy',
};
// Fake backend for the real control composition. No native commands or Worker.
let state = initial;
let generation = 1;
const listeners = new Set<() => void>();
const change = (next: BbsSyncView) => { state = next; listeners.forEach(changed => changed()); };
const source = {
  read: async () => state,
  listen: async (changed: () => void) => { listeners.add(changed); return () => { listeners.delete(changed); }; },
};
const actions = {
  invitation: async ({ refresh }: { refresh: boolean }) => {
    if (refresh || !state.group) generation += 1;
    change({ ...state, group: state.group ?? { ...initial.group!, members: [initial.group!.members[0]] }, invitationGeneration: String(generation) });
    return { groupId: 'demo', generation: String(generation), invitation: `kota-bbs://studio.demo.invalid/join#0123456789abcdef0123456789abcdef-${generation}` };
  },
  join: async () => change({ ...initial, group: { ...initial.group!, role: 'member' }, invitation: { state: 'none' }, invitationGeneration: null }),
  disconnect: async () => change({ ...state, group: null, invitation: { state: 'none' }, invitationGeneration: null }),
  rename: async ({ name }: { name: string }) => change({ ...state, deviceName: name, group: state.group && { ...state.group, members: state.group.members.map(m => m.id === 'self' ? { ...m, name } : m) } }),
  remove: async ({ deviceId }: { deviceId: string }) => change({ ...state, group: state.group && { ...state.group, members: state.group.members.filter(m => m.id !== deviceId) } }),
  start: async () => change({ ...state, phase: 'syncing', indicator: 'healthy', progress: { completed: 4, total: 12 } }),
  cancel: async () => change({ ...state, phase: 'idle', progress: null }),
};
function Preview() {
  const [open, setOpen] = useState(true);
  return <div style={{ padding: 32 }}>
    <p>Real control composition with a fake backend — no network, credentials, or BBS writes.</p>
    <label>Preview state <select aria-label="Preview state" onChange={event => {
      const s = event.target.value;
      const indicator: BbsSyncIndicator = BBS_SYNC_INDICATORS.includes(s as BbsSyncIndicator) ? s as BbsSyncIndicator
        : s === 'busy' ? 'other_instance' : s === 'identity' ? 'device_identity_error'
          : s === 'unknown' ? 'reconnecting' : s === 'partial' ? 'finishing_sync' : 'healthy';
      change({ ...initial, invitationGeneration: s === 'local' || s === 'no-worker' || s === 'member' || s === 'identity' ? null : String(generation),
        invitation: s === 'local' || s === 'no-worker' || s === 'member' ? { state: 'none' } : { state: 'preparing' },
        group: s === 'local' || s === 'no-worker' || s === 'identity' ? null
          : { ...initial.group!, members: s === 'busy' ? [] : initial.group!.members, role: s === 'member' ? 'member' : 'owner' },
        indicator, controlRecoverable: s === 'busy',
        error: s === 'busy' ? BBS_SYNC_ERROR_DETAILS.sync_busy : s === 'unknown' ? BBS_SYNC_ERROR_DETAILS.unknown
          : s === 'identity' ? BBS_SYNC_ERROR_DETAILS.identity : null,
        workerAvailable: s !== 'no-worker', phase: ['busy', 'unknown', 'identity'].includes(s) ? 'failed'
          : s === 'syncing' ? 'syncing' : s === 'partial' ? 'partial' : 'idle', progress: s === 'syncing' ? { completed: 4, total: 12 } : null });
    }}><option>owner</option><option>member</option><option>local</option><option>no-worker</option><option>syncing</option><option>partial</option>
      <option>busy</option><option>unknown</option><option>identity</option>
      {BBS_SYNC_INDICATORS.map(indicator => <option key={indicator}>{indicator}</option>)}
    </select></label>
    {!open && <button onClick={() => setOpen(true)}>Open BBS</button>}
    {open && <BbsSyncScope source={source} actions={actions}>
      <section className="bbs-modal" style={{ margin: '28px auto' }}>
        <header className="bbs-modal-head"><div className="bbs-modal-title"><b>Bulletin Board</b><span>All threads</span></div>
          <div className="bbs-modal-actions"><BbsSyncControlButton /><button>+ Post</button><button>Refresh</button><button onClick={() => setOpen(false)}>Close</button></div>
        </header>
        <BbsSyncActivity />
        <div className="bbs-thread-list"><button className="bbs-thread-row"><span className="bbs-msg-author">Hengwu</span><span className="bbs-time">11:32 PM</span>
          <BbsThreadSharing sharingGroupId="demo" /><span className="bbs-row-preview">The handoff is ready. I attached the notes here so the original file can stay exactly where it is.</span></button></div>
      </section>
    </BbsSyncScope>}
  </div>;
}
createRoot(document.getElementById('root')!).render(<Preview />);
