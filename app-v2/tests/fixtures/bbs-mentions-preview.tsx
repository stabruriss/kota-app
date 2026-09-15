import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { BbsMentionComposer } from '../../src/chrome/BbsMentionComposer';
import { BbsEditor } from '../../src/chrome/BbsEditor';
import { BbsRosterAvatar } from '../../src/chrome/BbsRosterAvatar';
import { bbsRosterAvatarImages, createBbsRosterAvatarImages } from '../../src/bbs-roster-avatars';
import { bbsMentionDeviceKey, bbsMentionSelection } from '../../src/bbs-mentions';
import type { BbsRosterAgent, BbsRosterDevice } from '../../src/types/bbs-roster';
import '../../src/styles/kota-tokens.css';
import '../../src/styles/canvas.css';

const agent = (agentId: string, name: string): BbsRosterAgent => ({ agentId, name, targetRef: `local/kota/${agentId}`, avatar: { kind: 'none' } });
const devices: BbsRosterDevice[] = [
  { deviceId: null, name: 'MacBook Pro', local: true, online: true, rosterStatus: 'synced', receivedAt: null, projects: [
    { projectId: 'kota', name: 'Kota', agents: [agent('hengwu', '蘅芜君'), agent('pin', '颦儿'), agent('qingwen', '晴雯'), agent('diao', '座山雕')] },
    { projectId: 'notes', name: 'Field Notes', agents: [agent('qingwen', '晴雯'), agent('gem', 'Gem')] },
  ] },
  { deviceId: 'peer', name: 'MacBook Air', local: false, online: false, rosterStatus: 'synced', receivedAt: '2026-09-13T00:00:00Z', projects: [
    { projectId: 'kota', name: 'Kota', agents: [agent('nian', '年羹尧'), agent('qingwen', '晴雯')] },
  ] },
  { deviceId: 'unknown', name: 'Travel Mac', local: false, online: false, rosterStatus: 'not_synced', receivedAt: null, projects: null },
];
// Test-only input supplied by the local browser verifier. No personal image
// bytes or fake credentials are signed into the repository or production build.
const raster = (window as unknown as { bbsAvatarFixture?: { src: string; sha256: string; sizeBytes: number; reads: number; active: number; maxActive: number } }).bbsAvatarFixture;
if (raster) {
  const images = createBbsRosterAvatarImages(async () => {
    raster.reads++; raster.active++; raster.maxActive = Math.max(raster.maxActive, raster.active);
    await new Promise(resolve => setTimeout(resolve, 40)); raster.active--;
    return raster.src;
  });
  bbsRosterAvatarImages.subscribe = images.subscribe;
  bbsRosterAvatarImages.forget = images.forget;
  const project = devices[0].projects![0];
  project.agents = Array.from({ length: 64 }, (_, i) => ({ ...agent(`visual-${i}`, `Avatar ${i + 1}`),
    avatar: { kind: 'image', sha256: raster.sha256, ext: 'jpg', sizeBytes: raster.sizeBytes, available: true } }));
}
function Preview() {
  const [open, setOpen] = useState(true);
  const [selected, setSelected] = useState([[0,0,0],[0,0,1],[0,1,0],[1,0,0]].map(([d,p,a]) => {
    const device = devices[d], project = device.projects![p]; return bbsMentionSelection(device, project, project.agents[a]);
  }));
  const [body, setBody] = useState('请一起核对线程里的附件，各自回一条结果。');
  const [remote, setRemote] = useState(true);
  return <main style={{ padding: 18 }}>
    <p>Real BBS picker + editor with fixture props. No IPC, network sync or real post.</p>
    <label><input type="checkbox" checked={remote} onChange={event => setRemote(event.target.checked)} /> Shared thread</label>
    <section className="bbs-modal" style={{ margin: '18px auto', width: 'min(960px, 100%)', minHeight: 600 }}>
      <header className="bbs-modal-head"><div className="bbs-modal-title"><b>Bulletin Board</b><span>Thread</span></div></header>
      <div className="bbs-detail" style={{ flex: 1 }}>
        <div className="bbs-detail-posts" style={{ flex: 1, padding: 20 }}><strong>老无 · OP</strong><p>测试资料已放在线程里。请各项目核对后回复。</p></div>
        <div className="bbs-reply-box"><div className="bbs-reply-shell bbs-mentions-shell">
          <BbsMentionComposer devices={devices} currentProjectId="kota" selected={selected} onChange={setSelected}
            renderAvatar={raster ? (agent, device) => <BbsRosterAvatar deviceId={bbsMentionDeviceKey(device)} name={agent.name} avatar={agent.avatar} /> : undefined}
            open={open} onOpenChange={setOpen} remoteAllowed={remote} submit={<button type="button" className="bbs-reply-send" disabled>Reply</button>}>
            <div className="bbs-reply-editor"><BbsEditor value={body} onChange={setBody} placeholder="Reply…" onStateChange={() => {}} onError={() => {}} /></div>
          </BbsMentionComposer>
        </div></div>
      </div>
    </section>
  </main>;
}
createRoot(document.getElementById('root')!).render(<Preview />);
