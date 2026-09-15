import { createRoot } from 'react-dom/client';
import { isTauri } from '@tauri-apps/api/core';
import { mockIPC } from '@tauri-apps/api/mocks';
import { bbsRemoveDeletedItem } from '../../src/bbs-post-versions';
import type { BbsDeleteRequest, BbsPost, BbsSnapshot, WorkspaceProject } from '../../src/pty-client';
import type { BbsRosterRow } from '../../src/types/bbs-roster';
import previewAvatarUrl from '../../src/assets/tavern/icons/subagent-update.png';
import '../../src/styles/kota-tokens.css';
import '../../src/styles/canvas.css';

// This isolated fixture exercises the actual RightColumn, with in-memory IPC.
// It is never imported by the product entry, and must never run in a real App.
if (isTauri()) throw new Error('Browser-only fixture');
Object.assign(globalThis, { isTauri: true });
const avatarPreview = new URLSearchParams(location.search).has('avatars');
const mentionsPreview = new URLSearchParams(location.search).has('mentions');
const rosterCalls: unknown[] = [], publications: unknown[] = [];
Object.assign(window, { bbsPreviewRosterReads: rosterCalls, bbsPreviewPublications: publications });
const raster = (window as unknown as { bbsAvatarFixture?: { src: string; sha256: string; sizeBytes: number } }).bbsAvatarFixture;
const avatarRequests: unknown[] = [];
Object.assign(window, { bbsPreviewAvatarReads: avatarRequests });
const workspace: WorkspaceProject = {
  projectId: 'preview', repoFullName: 'demo/preview', remoteUrl: '', githubHtmlUrl: '', defaultBranch: 'main', baseRef: 'main',
  localRoot: '/tmp/bbs-preview', localRootBytes: 0, sourceDir: '/tmp/bbs-preview/source', sourceDirBytes: 0,
  sharedDir: '/tmp/bbs-preview/project-memory', rulesDir: '/tmp/bbs-preview/rules', agents: [],
};
function post(id: string, version: string, body: string, kind = 'reply'): BbsPost {
  return { postId: id, threadId: 'thread-preview', versionId: version.repeat(64), body, preview: body, kind,
    agentId: 'agent-one', agentDisplayName: 'Hengwu', agentAvatar: 'codex', projectId: 'preview', projectDisplayName: 'Studio',
    createdAt: '2026-09-12T09:00:00Z', state: 'none', external: false };
}
let board: BbsSnapshot = { projectId: 'preview', projectDisplayName: 'Studio', root: '/tmp/bbs-preview', newCount: 0,
  threads: [{ threadId: 'thread-preview', sharingGroupId: 'demo', visibility: 'broadcast', projectTags: [], projectTagLabels: [],
    createdByProject: 'preview', createdByProjectLabel: 'Studio', updatedAt: '2026-09-12T09:00:00Z', latestPostId: 'reply-two',
    isNew: false, relevant: true, posts: [
      post('topic', 'a', 'Draft A: The notes are ready for review.', 'topic'),
      post('topic', 'b', 'Draft B: The notes include the revised introduction.', 'topic'),
      post('reply-one', 'a', 'Reply A: Keep the opening short.'),
      post('reply-one', 'b', 'Reply B: Keep the opening short and add an example.'),
      post('reply-two', 'c', 'A separate reply stays on its own floor.'),
    ] }],
};
const deletes: BbsDeleteRequest[] = [];
const roster: BbsRosterRow[] = [];
if (mentionsPreview) for (const [deviceId, name, local] of [['self', 'This Mac', true], ['peer', 'Other Mac', false]] as const) {
  roster.push({ kind: 'device', deviceId, name, local, online: true, rosterStatus: 'synced', receivedAt: null });
  for (const projectId of local ? ['preview', 'notes'] : ['preview']) {
    roster.push({ kind: 'project', deviceId, projectId, name: projectId === 'preview' ? 'Kota' : 'Field Notes' });
    for (let i = 0; i < (local && projectId === 'preview' ? 65 : 2); i++) roster.push({ kind: 'agent', deviceId, projectId,
      agentId: `agent-${i}`, name: i ? `Agent ${i + 1}` : '蘅芜君', targetRef: `${deviceId}/${projectId}/agent-${i}`,
      avatar: raster ? { kind: 'image', sha256: raster.sha256, ext: 'jpg', sizeBytes: raster.sizeBytes, available: true } : { kind: 'none' } });
  }
}
const unavailablePreview = new URLSearchParams(location.search).get('unavailable');
if (unavailablePreview) {
  const thread = board.threads[0];
  thread.posts = unavailablePreview === 'reply'
    ? [post('topic', 'a', 'The review notes are ready.', 'topic'), post('reply-one', 'b', 'This reply synced normally.')]
    : unavailablePreview === 'unknown' ? [] : [post('reply-one', 'b', 'This reply synced normally; its root is unavailable.')];
  thread.unavailablePosts = [{
    postId: `post-20260912-120000-agent-${'long-but-safe-id-'.repeat(6)}1234abcd`,
    reason: 'too_large_to_sync',
    ...(unavailablePreview === 'unknown' ? {} : { kind: unavailablePreview === 'reply' ? 'reply' as const : 'topic' as const }),
  }];
}
if (avatarPreview) {
  board.threads[0].posts = board.threads[0].posts.map((item, index) => ({ ...item,
    syncAvatar: index === 0 ? { kind: 'image', sha256: 'd'.repeat(64), ext: 'png', localPath: '/unused/by/frontend.png', available: true }
      : index === 1 ? { kind: 'builtin', id: 'violet' }
        : index === 2 ? { kind: 'image', sha256: 'e'.repeat(64), ext: 'png', localPath: null, available: false }
          : { kind: 'none' },
  }));
}
Object.assign(window, { bbsPreviewDeletes: deletes });
mockIPC(async (command, args) => {
  const request = (args as Record<string, unknown> | undefined)?.request as Record<string, unknown> | undefined;
  switch (command) {
    case 'bbs_roster_read': {
      if (!mentionsPreview) throw new Error('No roster fixture');
      rosterCalls.push(request);
      const start = request?.after === null ? 0 : Number(request?.after);
      return { version: 'a'.repeat(64), items: roster.slice(start, start + 32), next: start + 32 < roster.length ? String(start + 32) : null };
    }
    case 'bbs_roster_avatar_read': {
      if (!raster || !['local', 'self', 'peer'].includes(String(request?.deviceId)) || request?.sha256 !== raster.sha256) throw new Error('No referenced roster image');
      avatarRequests.push(request); return raster.src;
    }
    case 'bbs_human_reply': case 'bbs_human_post': {
      if (!mentionsPreview) throw new Error('Read-only preview');
      publications.push({ command, request }); return command === 'bbs_human_reply' ? 'preview-reply' : 'thread-preview';
    }
    case 'bbs_sync_avatar_read': {
      avatarRequests.push(request);
      if (request?.sha256 !== 'd'.repeat(64) || request?.ext !== 'png') throw new Error('Avatar unavailable');
      // Same-origin repository PNG only; this fixture never reads a user file.
      const bytes = new Uint8Array(await (await fetch(previewAvatarUrl)).arrayBuffer());
      return `data:image/png;base64,${btoa(String.fromCharCode(...bytes))}`;
    }
    case 'bbs_snapshot': return structuredClone(board);
    case 'bbs_delete': {
      const target = request as unknown as BbsDeleteRequest;
      deletes.push(target); board = bbsRemoveDeletedItem(board, target)!; return null;
    }
    case 'bbs_mark_processed': return null;
    case 'bbs_sync_status': return {
      protocolVersion: 1, device: { id: 'self', name: 'Study Mac' }, worker: { configured: false, canCreateGroup: false },
      group: { id: 'demo', name: 'Studio', role: 'member', members: [
        { id: 'self', name: 'Study Mac', role: 'member', online: true, publicKey: 'preview' },
        { id: 'peer', name: 'Travel Mac', role: 'owner', online: true, publicKey: 'preview-peer' },
      ] }, invitation: 'none', invitationGeneration: null,
      sync: { phase: 'idle', completed: null, total: null, lastSuccessfulAt: '2026-09-12T09:00:00Z', error: null },
    };
    case 'account_user_identity_load': return { name: 'User', avatarId: 'user-default' };
    case 'hero_avatar_list': return [];
    case 'ember_schedule_state': return { drafts: [], schedules: [], history: [], appLastSeenAt: null };
    case 'ember_schedule_save': return request?.state;
    case 'lm_status': return null;
    case 'bartender_status': return { state: 'idle', message: 'Browser fixture', dirtyAgents: [], roomChangeCount: 0, githubChangeCount: 0 };
    case 'violet_summary_status': return { latest: null, history: [], outstanding: { sinceTs: null, messageCount: 0 }, logPath: '', promptPath: '', updatedAt: '2026-09-12T09:00:00Z' };
    default: throw new Error(`Browser fixture does not implement ${command}`);
  }
}, { shouldMockEvents: true });
const { RightColumn } = await import('../../src/chrome/RightColumn');
createRoot(document.getElementById('root')!).render(<>
  <p style={{ padding: 20 }}>BBS product preview — all data, posts and deletes are in memory. No native commands, network sync or files.</p>
  <RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />
</>);
