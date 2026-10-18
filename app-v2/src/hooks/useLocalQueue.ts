import { useCallback, useEffect, useRef, useState } from 'react';
import { controlLocalWorkbench, onLocalNotice, sameLocalScope, type LocalQueueItem, type LocalScope } from '../lib/local-rpc';
import { localQueueGuardKey, localQueueMessageId, readLocalQueueOriginal, type LocalQueueOriginal } from '../lib/local-queue';
import type { AgentId } from '../types/scene';
import type { ComposerMention } from '../chrome/InputBar';

export interface LocalQueueDraft {
  instance: string; scope: LocalScope; item: LocalQueueItem; original: LocalQueueOriginal;
  text: string; target: AgentId; editToken: string; valid: boolean; changed: boolean; busy: boolean;
}
export interface LocalQueueAddDraft {
  kind: 'add'; instance: string; scope: Pick<LocalScope, 'projectRoot' | 'agentId'>;
  text: string; target: AgentId; valid: boolean; busy: boolean;
}
export type LocalQueueSend = (target: AgentId | null, text: string,
  options?: { broadcast: boolean; privacy: boolean; mentions?: ComposerMention[]; eligibilityText?: string }) => boolean | void | Promise<boolean | void>;
const ownerKey = (root: string, agent: string) => JSON.stringify([root, agent]);
const itemKey = (scope: LocalScope, item: LocalQueueItem) => JSON.stringify([scope.projectRoot, item.kotaId]);
const guardKey = (scope: LocalScope, item: LocalQueueItem) => localQueueGuardKey(scope.projectRoot,
  localQueueMessageId(item.kotaId, scope.agentId) ?? '', scope.agentId);
const EMPTY_PRIVATE_AGENTS: ReadonlySet<AgentId> = new Set();

/** App-lifetime queue drafts only. No streams, Pi polling or persistent state. */
export function useLocalQueue(projectRoot: string | null, onGuardsChange: (keys: ReadonlySet<string>) => void, onSend: LocalQueueSend,
  privateAgents: ReadonlySet<AgentId> = EMPTY_PRIVATE_AGENTS) {
  const [drafts, setDrafts] = useState<ReadonlyMap<string, LocalQueueDraft>>(new Map());
  const [addDrafts, setAddDrafts] = useState<ReadonlyMap<string, LocalQueueAddDraft>>(new Map());
  const [pending, setPending] = useState<ReadonlyMap<string, 'edit' | 'remove'>>(new Map());
  const [errors, setErrors] = useState<ReadonlyMap<string, string>>(new Map());
  const live = useRef({ projectRoot, drafts, addDrafts, pending, onSend, onGuardsChange, privateAgents });
  live.current = { ...live.current, projectRoot, onSend, onGuardsChange, privateAgents };
  const withdrawn = useRef(new Set<string>());
  const takingOut = useRef(new Set<string>());
  const mounted = useRef(false);
  const publish = useCallback(() => {
    const keys = new Set(withdrawn.current);
    for (const draft of live.current.drafts.values()) if (draft.valid) keys.add(guardKey(draft.scope, draft.item));
    live.current.onGuardsChange(keys);
  }, []);
  const writeDrafts = useCallback((next: ReadonlyMap<string, LocalQueueDraft>) => {
    live.current.drafts = next; if (mounted.current) setDrafts(next); publish();
  }, [publish]);
  const updateDraft = useCallback((root: string, agent: string, change: Partial<LocalQueueDraft>) => {
    const key = ownerKey(root, agent), draft = live.current.drafts.get(key);
    if (!draft) return;
    writeDrafts(new Map(live.current.drafts).set(key, { ...draft, ...change }));
  }, [writeDrafts]);
  const error = useCallback((scope: Pick<LocalScope, 'projectRoot' | 'agentId'>, text: string | null) => {
    if (mounted.current) setErrors(prev => { const next = new Map(prev); const key = ownerKey(scope.projectRoot, scope.agentId);
      if (text) next.set(key, text); else next.delete(key); return next; });
  }, []);
  const writeAddDrafts = useCallback((next: ReadonlyMap<string, LocalQueueAddDraft>) => {
    live.current.addDrafts = next; if (mounted.current) setAddDrafts(next);
  }, []);
  const add = useCallback((root: string, agent: AgentId) => {
    const key = ownerKey(root, agent);
    if (root !== live.current.projectRoot || live.current.drafts.has(key) || live.current.addDrafts.has(key) || takingOut.current.has(key)) return;
    writeAddDrafts(new Map(live.current.addDrafts).set(key, { kind: 'add', instance: crypto.randomUUID(),
      scope: { projectRoot: root, agentId: agent }, target: agent, text: '', valid: true, busy: false }));
    error({ projectRoot: root, agentId: agent }, null);
  }, [error, writeAddDrafts]);
  const updateAddDraft = useCallback((root: string, agent: AgentId, text: string) => {
    const key = ownerKey(root, agent), draft = live.current.addDrafts.get(key);
    if (draft && !draft.busy) writeAddDrafts(new Map(live.current.addDrafts).set(key, { ...draft, text }));
  }, [writeAddDrafts]);
  const removeAddDraft = useCallback((root: string, agent: AgentId) => {
    const key = ownerKey(root, agent); if (live.current.addDrafts.get(key)?.busy) return;
    const next = new Map(live.current.addDrafts); next.delete(key); writeAddDrafts(next);
  }, [writeAddDrafts]);
  const finishAdd = useCallback(async (root: string, agent: AgentId, payload?: string, mentions?: ComposerMention[], eligibilityText?: string) => {
    const key = ownerKey(root, agent), draft = live.current.addDrafts.get(key), text = payload ?? draft?.text ?? '';
    if (!draft || draft.busy || root !== live.current.projectRoot || !text.trim()) return;
    const owns = () => live.current.addDrafts.get(key)?.instance === draft.instance;
    writeAddDrafts(new Map(live.current.addDrafts).set(key, { ...draft, busy: true })); error(draft.scope, null);
    try {
      // A new explicit user send owns its normal room ID, privacy and lifecycle.
      // There is no cancelled item, edit token, or queue-requeue operation.
      const sent = await live.current.onSend(agent, text, { broadcast: false, privacy: live.current.privateAgents.has(agent), mentions,
        ...(eligibilityText === undefined ? {} : { eligibilityText }) });
      if (sent !== false && owns()) { const next = new Map(live.current.addDrafts); next.delete(key); writeAddDrafts(next); }
    } catch { if (owns()) error(draft.scope, "Couldn't send this message. Your draft is kept."); }
    finally {
      if (owns()) { const next = new Map(live.current.addDrafts); next.set(key, { ...next.get(key)!, busy: false }); writeAddDrafts(next); }
    }
  }, [error, writeAddDrafts]);
  const release = useCallback((draft: LocalQueueDraft) => {
    void controlLocalWorkbench(draft.scope, { type: 'queue_release', kotaId: draft.item.kotaId, editToken: draft.editToken }).catch(() => {});
  }, []);
  const invalidate = useCallback((scope?: LocalScope) => {
    const next = new Map(live.current.drafts); let changed = false;
    for (const [key, draft] of next) if (draft.valid && (!scope || sameLocalScope(draft.scope, scope))) {
      release(draft); next.set(key, { ...draft, valid: false, busy: false }); changed = true;
    }
    if (changed) writeDrafts(next);
  }, [release, writeDrafts]);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    // Switching projects expires the token, not the user's draft or C keys.
    for (const draft of live.current.drafts.values()) if (draft.valid && draft.scope.projectRoot !== projectRoot) invalidate(draft.scope);
  }, [projectRoot, invalidate]);
  const validDraftsKey = [...drafts.values()].filter(d => d.valid).map(d => d.instance).sort().join('|');
  useEffect(() => {
    const valid = [...live.current.drafts.values()].filter(d => d.valid);
    let current = true; const cleanups: (() => void)[] = [];
    for (const draft of valid) {
      void onLocalNotice(draft.scope.agentId, notice => {
        if (!current || notice.kind !== 'status' || notice.scope.projectRoot !== draft.scope.projectRoot) return;
        if (!sameLocalScope(notice.scope, draft.scope) || notice.state === 'offline' || notice.state === 'error') { invalidate(draft.scope); return; }
        // This control reads owner memory; it neither talks to Pi nor renders a stream.
        void controlLocalWorkbench<{ valid: boolean }>(draft.scope, { type: 'queue_edit_valid', kotaId: draft.item.kotaId, editToken: draft.editToken })
          .then(result => { if (current && !result.valid) invalidate(draft.scope); }).catch(() => { if (current) invalidate(draft.scope); });
      }).then(unlisten => { if (current) cleanups.push(unlisten); else unlisten(); }).catch(() => {});
    }
    return () => { current = false; cleanups.forEach(f => f()); };
  }, [validDraftsKey, invalidate]);

  const cancel = useCallback(async (scope: LocalScope, item: LocalQueueItem, edit: boolean) => {
    const key = itemKey(scope, item), owner = ownerKey(scope.projectRoot, scope.agentId);
    if (live.current.pending.has(key)) return;
    if (edit && (live.current.drafts.has(owner) || live.current.addDrafts.has(owner) || takingOut.current.has(owner))) { error(scope, 'Finish or remove the open draft before editing another message.'); return; }
    if (edit) takingOut.current.add(owner);
    const next = new Map(live.current.pending).set(key, edit ? 'edit' as const : 'remove' as const);
    live.current.pending = next; setPending(next); error(scope, null);
    try {
      const original = edit ? await readLocalQueueOriginal(scope, item) : null;
      if (edit && !original) { error(scope, "Couldn't find the complete original message. Nothing was removed."); return; }
      if (edit && live.current.projectRoot !== scope.projectRoot) return;
      const result = await controlLocalWorkbench<{ cancelled: boolean; reason?: string; editToken?: string; valid?: boolean }>(scope,
        { type: 'queue_cancel', kotaId: item.kotaId, edit });
      if (!result.cancelled) { error(scope, 'Already sent to Pi. This message can no longer be removed.'); return; }
      if (edit && original) {
        const draft: LocalQueueDraft = { instance: crypto.randomUUID(), scope, item, original, text: original.text, target: scope.agentId,
          editToken: result.editToken ?? '', valid: !!result.editToken && result.valid !== false && live.current.projectRoot === scope.projectRoot,
          changed: false, busy: false };
        if (!draft.valid) release(draft);
        writeDrafts(new Map(live.current.drafts).set(owner, draft));
      } else { withdrawn.current.add(guardKey(scope, item)); publish(); }
    } catch { error(scope, edit ? "Couldn't take out this message. Nothing was edited." : "Couldn't remove this message."); }
    finally { if (edit) takingOut.current.delete(owner); const next = new Map(live.current.pending); next.delete(key); live.current.pending = next; if (mounted.current) setPending(next); }
  }, [error, publish, release, writeDrafts]);

  const finish = useCallback(async (root: string, agent: string, restore: boolean, payload?: string, mentions?: ComposerMention[], eligibilityText?: string) => {
    const key = ownerKey(root, agent), draft = live.current.drafts.get(key);
    if (!draft || draft.busy || !draft.valid || live.current.projectRoot !== root) return;
    const owns = () => mounted.current && live.current.projectRoot === root
      && live.current.drafts.get(key)?.instance === draft.instance && live.current.drafts.get(key)?.valid;
    updateDraft(root, agent, { busy: true }); error(draft.scope, null);
    try {
      const valid = await controlLocalWorkbench<{ valid: boolean }>(draft.scope,
        { type: 'queue_edit_valid', kotaId: draft.item.kotaId, editToken: draft.editToken });
      if (!owns()) return;
      if (!valid.valid) { invalidate(draft.scope); return; }
      const unchanged = draft.target === draft.scope.agentId && (!draft.changed || payload === draft.original.text || payload === draft.original.text.trimEnd());
      if (restore || unchanged) {
        const result = await controlLocalWorkbench<{ queued: boolean }>(draft.scope,
          { type: 'queue_requeue', kotaId: draft.item.kotaId, editToken: draft.editToken });
        if (!owns() || !result.queued) return;
      } else {
        const sent = await live.current.onSend(draft.target, payload ?? draft.text, { broadcast: false, privacy: draft.original.privacy, mentions,
          ...(eligibilityText === undefined ? {} : { eligibilityText }) });
        // Once handed to handleSend this is a normal, explicit user send.
        // Stop/Fresh/project changes do not cancel that newly sent message.
        if (sent === false || live.current.drafts.get(key)?.instance !== draft.instance) return;
        withdrawn.current.add(guardKey(draft.scope, draft.item)); release(draft);
      }
      const next = new Map(live.current.drafts); next.delete(key); writeDrafts(next);
    } catch { if (owns()) error(draft.scope, "Couldn't queue this message. Your draft is kept."); }
    finally { if (live.current.drafts.get(key)?.instance === draft.instance) updateDraft(root, agent, { busy: false }); }
  }, [error, invalidate, release, updateDraft, writeDrafts]);
  const removeDraft = useCallback((root: string, agent: string) => {
    const key = ownerKey(root, agent), draft = live.current.drafts.get(key); if (!draft || draft.busy) return;
    if (draft.valid) { withdrawn.current.add(guardKey(draft.scope, draft.item)); release(draft); }
    const next = new Map(live.current.drafts); next.delete(key); writeDrafts(next);
  }, [release, writeDrafts]);
  return { drafts, addDrafts, pending, errors, cancel, finish, updateDraft, removeDraft, invalidate, ownerKey, itemKey,
    add, finishAdd, updateAddDraft, removeAddDraft };
}
export type LocalQueueController = ReturnType<typeof useLocalQueue>;
