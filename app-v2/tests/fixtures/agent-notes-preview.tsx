// Real UI components with an isolated, synthetic IPC boundary. Never mounts App.
import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { ProjectAgentProfileOverlay } from '../../src/chrome/ProjectAgentProfileOverlay';
import { VioletRoomPanel } from '../../src/chrome/VioletRoomPanel';
import type { ProjectAgentDetail, VioletChatMessage } from '../../src/pty-client';
import '../../src/styles/kota-tokens.css';
import '../../src/styles/canvas.css';

const root = '/synthetic/notes-project';
const state = { exists: true, failOpen: false, calls: [] as { command: string; args: unknown }[] };
Object.assign(window, { notesFixture: state, isTauri: true,
  __TAURI_INTERNALS__: {
    transformCallback: () => 1, unregisterCallback: () => {},
    invoke: async (command: string, args: unknown) => {
      state.calls.push({ command, args });
      switch (command) {
        case 'project_agent_load_detail': return detail();
        case 'project_agent_notes_open': if (state.failOpen) throw 'Notes file not found.'; return;
        case 'account_skills_list': case 'supported_shells_status': case 'hero_avatar_list': return [];
        case 'violet_room_read_cache': case 'violet_room_sync': return { messages, sources: [], workEvents: [],
          rawLogDir: `${root}/project-memory/raw_logs`, chathistoryDir: `${root}/project-memory/chathistory`, syncedAt: '2026-10-27T10:00:01Z' };
        case 'plugin:event|listen': return 1;
        case 'plugin:event|unlisten': return;
        case 'violet_resolve_file_ref': return null;
        default: throw new Error(`Unexpected preview IPC: ${command}`);
      }
    },
  },
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
});

function detail(): ProjectAgentDetail {
  return { agentId: 'agent-preview', displayName: '颦儿 v. Kota', sourceHeroId: 'hero-source', sourceHeroName: 'Source',
    projectId: 'notes-project', projectName: 'Kota', cli: 'codex', provider: 'codex', model: 'default', effort: 'high',
    avatarId: 'codex', skills: [], args: [], ghost: 'Prefer concrete file references and clear handoff notes.\nPreserve unknown changes from the user or other agents.\n用林黛玉的口吻说话',
    adapterPath: `${root}/.agent-workspaces/agent-preview/AGENTS.md`, shellPath: `${root}/.agent-workspaces/agent-preview/SHELL.yaml`,
    agentYamlPath: `${root}/.agent-workspaces/agent-preview/agent.yaml`,
    notes: { exists: state.exists, path: `${root}/.agent-workspaces/agent-preview/under-table/soul/marks.json` },
    status: 'archived', inviteEligibility: { eligible: false, proposedHeroId: 'h', proposedDisplayName: '颦儿' },
    record: { turns: 42, incarnations: 1, estimatedTokens: 0, commends: 3 }, forkable: false, dirty: false, dirtySummary: '' };
}
const messages: VioletChatMessage[] = [{ id: 'saved-note', sessionId: 'soul', agentId: 'agent-preview', shell: 'codex',
  role: 'assistant', kind: 'commentary', messageOrigin: 'note', timestamp: '2026-10-27T10:00:00Z',
  text: '修改界面交互时，先实际完成点击、拖动和松手，再与正常控件对照。`单元测试`通过不能代替真实操作验证。',
  agentDisplayName: '颦儿 v. Kota', agentProvider: 'codex', agentAvatarId: 'codex' }];

function Preview() {
  const [profile, setProfile] = useState(false);
  const [mode, setMode] = useState('available');
  return <>
    <style>{`
      .notes-preview-nav {position:fixed;inset:0 0 auto;height:48px;display:flex;align-items:center;gap:12px;padding:0 24px;background:#191a1c;z-index:150;font:12px var(--font-sans);color:var(--text-2)}
      .notes-preview-nav button,.notes-preview-nav select {padding:5px 9px;color:var(--text);background:#222326;border:1px solid var(--border);border-radius:5px}
      .notes-preview-room {position:absolute;inset:72px 110px 28px;overflow:hidden;border:1px solid var(--border);border-radius:14px;}
    `}</style>
    <nav className="notes-preview-nav"><span>Real components · synthetic data</span>
      <button onClick={() => setProfile(false)}>Room</button><button onClick={() => setProfile(true)}>Agent detail</button>
      {profile && <select aria-label="Fixture file state" value={mode} onChange={event => {
        setMode(event.target.value); state.exists = event.target.value !== 'missing'; state.failOpen = event.target.value === 'error';
      }}><option value="available">Notes available</option><option value="missing">No file yet</option><option value="error">Open error</option></select>}
    </nav>
    {profile ? <ProjectAgentProfileOverlay key={mode} agentId="agent-preview" projectRoot={root} existingNames={[]}
      onClose={() => setProfile(false)} onSaved={() => {}} onRemoveFromProject={() => {}} />
      : <div className="notes-preview-room"><VioletRoomPanel projectRoot={root} agentIds={['agent-preview']} /></div>}
  </>;
}
createRoot(document.getElementById('root')!).render(<Preview />);
