// Real attachment component and styles; the same JSON is checked against Rust snapshots.
import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { BbsAttachments } from '../../src/chrome/BbsAttachments';
import { MarkdownText } from '../../src/chrome/VioletRoomPanel';
import type { BbsAttachment } from '../../src/pty-client';
import fixtures from './bbs-attachment-states.json';
import '../../src/styles/kota-tokens.css';
import '../../src/styles/canvas.css';

const state = { calls: [] as { command: string; args: unknown }[] };
Object.assign(window, { attachmentFixture: state, isTauri: true,
  __TAURI_INTERNALS__: { invoke: async (command: string, args: unknown) => {
    state.calls.push({ command, args });
    if (command === 'file_image_data_url') return 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aX1sAAAAASUVORK5CYII=';
    throw new Error('Unexpected fixture IPC: ' + command);
  } },
});

function Preview() {
  const [mode, setMode] = useState<keyof typeof fixtures>('pending');
  const [complete, setComplete] = useState(false);
  const fixture = fixtures[mode];
  const original = fixture.attachments as BbsAttachment[];
  const attachments: BbsAttachment[] = complete
    ? original.map(({ syncState: _syncState, ...item }) => ({ ...item, available: true }))
    : original;
  return <main style={{ padding: 28 }}>
    <nav style={{ display: 'flex', gap: 12, alignItems: 'center', marginBottom: 24 }}>
      <span>Real component · backend state fixture</span>
      <select aria-label="Attachment fixture" value={mode} onChange={event => {
        setMode(event.target.value as keyof typeof fixtures); setComplete(false);
      }}>
        <option value="pending">Pending</option><option value="paused">Paused</option><option value="mixed">Mixed</option>
      </select>
      <button onClick={() => setComplete(true)}>Apply verified snapshot</button>
    </nav>
    <section className="bbs-modal" style={{ margin: '0 auto', width: 'min(840px, 100%)', minHeight: 420 }}>
      <header className="bbs-modal-head"><div className="bbs-modal-title"><b>Bulletin Board</b><span>Thread</span></div></header>
      <div className="bbs-detail">
        <div className="bbs-detail-posts">
          <article className="bbs-msg">
            <div className="bbs-msg-main">
              <div className="bbs-msg-head"><span className="bbs-msg-author">Author</span><span className="bbs-op-badge">OP</span><span className="bbs-time">Just now</span></div>
              <div className="bbs-msg-body">
                <MarkdownText text={fixture.body} />
                <BbsAttachments body={fixture.body} baseRoot={null} postId={fixture.postId} attachments={attachments} />
              </div>
            </div>
          </article>
          <article className="bbs-msg"><div className="bbs-msg-main">
            <div className="bbs-msg-head"><span className="bbs-msg-author">Reader</span><span className="bbs-time">Just now</span></div>
            <div className="bbs-msg-body">正文可读，先讨论这里的内容。</div>
          </div></article>
        </div>
        <div className="bbs-reply-box"><textarea aria-label="Reply" placeholder="Reply to this thread…" /></div>
      </div>
    </section>
  </main>;
}
createRoot(document.getElementById('root')!).render(<Preview />);
