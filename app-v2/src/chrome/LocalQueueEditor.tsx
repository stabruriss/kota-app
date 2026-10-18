import { useEffect, useRef, useState } from 'react';
import type { Agent, AgentId } from '../types/scene';
import { parseRoomQuotePrompt } from '../lib/room-quote';
import { saveComposerClipboardImage } from '../pty-client';
import { InputBar, type InputBarHandle } from './InputBar';
import { HeroAvatarArt } from './HeroAvatarPicker';
import type { LocalQueueAddDraft, LocalQueueController, LocalQueueDraft } from '../hooks/useLocalQueue';

export function LocalQueueEditor({ draft, queue, agents, targets, direct = false, privateMessage = false }: { draft: LocalQueueDraft | LocalQueueAddDraft; queue: LocalQueueController;
  agents: Readonly<Record<AgentId, Agent>>; targets: readonly AgentId[]; direct?: boolean; privateMessage?: boolean }) {
  const adding = 'kind' in draft;
  const input = useRef<InputBarHandle>(null);
  const editor = useRef<HTMLElement>(null);
  const [value, setValue] = useState(() => parseRoomQuotePrompt(draft.text).body);
  useEffect(() => {
    for (const quote of parseRoomQuotePrompt(draft.text).quotes) input.current?.insertQuote(quote);
    input.current?.focusEnd();
    // The draft survives unmounting; the embedded composer is rebuilt once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft.instance]);
  useEffect(() => {
    if (!adding) return;
    // InputBar focuses with preventScroll; reveal the textbox after its draft/quotes mount.
    const frame = requestAnimationFrame(() => editor.current?.querySelector<HTMLElement>('[role="textbox"]')?.scrollIntoView?.({ block: 'nearest' }));
    return () => cancelAnimationFrame(frame);
  }, [adding, draft.instance]);
  const patch = (change: Partial<LocalQueueDraft>) => {
    if (adding) { if (change.text !== undefined) queue.updateAddDraft(draft.scope.projectRoot, draft.scope.agentId, change.text); }
    else queue.updateDraft(draft.scope.projectRoot, draft.scope.agentId, change);
  };
  return <section ref={editor} className={`queue-editor${adding ? ' adding' : ''}${draft.valid ? '' : ' expired'}`} aria-label={adding ? 'New message' : 'Edit queued message'}>
    <header className="queue-editor-heading"><div><strong>{adding ? 'New message' : 'Editing message'}</strong><span>{!adding && draft.item.customType === 'kota.lm' ? 'Laughing Man' : 'Composer'} · {adding ? 'not sent' : 'not queued'}</span></div>
      {!adding && <button className="queue-remove" disabled={draft.busy} onClick={() => queue.removeDraft(draft.scope.projectRoot, draft.scope.agentId)}>Remove</button>}</header>
    {draft.valid ? <p className="queue-editor-note">{adding ? direct ? `This message goes directly to ${agents[draft.target]?.name ?? draft.target}.` : 'New messages join the end of this queue.' : 'Other queued messages continue. Cancel puts the original message back at the end of the queue.'}</p>
      : <p className="queue-editor-note" role="status">This edit has expired. Your draft is kept for copying; it will not be sent.</p>}
    <div className="queue-edit-targets" role="group" aria-label="Message target">
      <span>To</span>{(adding ? [draft.target] : targets).filter(id => agents[id]).slice(0, 8).map(id => <button key={id} aria-pressed={draft.target === id}
        disabled={!draft.valid || draft.busy || adding} onClick={() => patch({ target: id })}>
        <HeroAvatarArt avatarId={agents[id]!.avatarId} provider={agents[id]!.provider} /><span>{agents[id]!.name}</span>
      </button>)}
    </div>
    <InputBar ref={input} variant="embedded" value={value} disabled={!draft.valid || draft.busy}
      onChange={next => { setValue(next); patch({ text: input.current?.serialize().payload ?? next, changed: true }); }}
      targetAgent={draft.target} agentMeta={agents} mentionAgentIds={targets} quoteProjectRoot={draft.scope.projectRoot}
      privacyMode={adding && privateMessage} privacyControlsEnabled={adding}
      onPasteImage={file => saveComposerClipboardImage(file, { projectRoot: draft.scope.projectRoot })}
      placeholder={adding ? `Write a message to ${agents[draft.target]?.name ?? draft.target}…` : 'Edit the message before putting it back in the queue…'} />
    <footer className="queue-editor-actions">
      <button className="soft-button" disabled={!draft.valid || draft.busy} onClick={() => adding ? queue.removeAddDraft(draft.scope.projectRoot, draft.scope.agentId) : void queue.finish(draft.scope.projectRoot, draft.scope.agentId, true)}>Cancel</button>
      <button className="primary-button" disabled={!draft.valid || draft.busy || !draft.text.trim() || (!adding && !targets.includes(draft.target))}
        onClick={() => { const serialized = input.current?.serialize(); if (adding) void queue.finishAdd(draft.scope.projectRoot, draft.scope.agentId, serialized?.payload ?? draft.text, serialized?.mentions, serialized?.eligibilityText);
          else void queue.finish(draft.scope.projectRoot, draft.scope.agentId, false, serialized?.payload ?? draft.text, serialized?.mentions, serialized?.eligibilityText); }}>
        {draft.busy ? adding ? 'Sending…' : 'Queueing…' : adding && direct ? 'Send message' : 'Queue message'}</button>
    </footer>
  </section>;
}
