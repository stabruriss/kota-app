import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  deleteProjectRule,
  listProjectRules,
  saveProjectRule,
  type ProjectRuleDraft,
  type ProjectRuleSaveRequest,
  type ProjectRulesRequest,
} from '../pty-client';
import { normalizedRuleTrigger } from '../lib/rule-trigger';

type RuleAutoStatus = 'idle' | 'editing' | 'saving' | 'saved';

export interface ProjectRulesMedalProps {
  projectName?: string | null;
  projectRoot?: string | null;
  rulesDir?: string | null;
  onClose: () => void;
}

function sameRuleDraft(a: ProjectRuleDraft, b: ProjectRuleDraft): boolean {
  return (
    a.title.trim() === b.title.trim() &&
    a.loadPolicy === b.loadPolicy &&
    normalizedRuleTrigger(a) === normalizedRuleTrigger(b) &&
    a.body.trim() === b.body.trim()
  );
}

function ruleSaveRequest(
  rule: ProjectRuleDraft,
  request: ProjectRulesRequest,
): ProjectRuleSaveRequest {
  return {
    ...request,
    fileName: rule.fileName || null,
    title: rule.title,
    loadPolicy: rule.loadPolicy,
    taskTrigger: normalizedRuleTrigger(rule),
    body: rule.body,
  };
}

export function ProjectRulesMedal({
  projectName,
  projectRoot,
  rulesDir,
  onClose,
}: ProjectRulesMedalProps) {
  const projectLabel = projectName?.trim() || 'this project';
  const request = useMemo<ProjectRulesRequest>(() => ({
    projectRoot: projectRoot ?? null,
    rulesDir: rulesDir ?? null,
  }), [projectRoot, rulesDir]);
  const [rules, setRules] = useState<ProjectRuleDraft[]>([]);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [draft, setDraft] = useState<ProjectRuleDraft | null>(null);
  const [busy, setBusy] = useState(false);
  const [autoStatus, setAutoStatus] = useState<RuleAutoStatus>('idle');
  const [deleteTarget, setDeleteTarget] = useState<ProjectRuleDraft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const saveTimerRef = useRef<number | null>(null);
  const saveSeqRef = useRef(0);

  const refreshRules = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const next = await listProjectRules(request);
      setRules(next);
      setSelectedFile((current) => current ?? next[0]?.fileName ?? null);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }, [request]);

  useEffect(() => {
    void refreshRules();
  }, [refreshRules]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      if (deleteTarget) setDeleteTarget(null);
      else onClose();
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, [deleteTarget, onClose]);

  useEffect(() => () => {
    if (saveTimerRef.current != null) {
      window.clearTimeout(saveTimerRef.current);
      saveTimerRef.current = null;
    }
  }, []);

  useEffect(() => {
    const selected = rules.find((rule) => rule.fileName === selectedFile) ?? rules[0] ?? null;
    setDraft((current) => {
      if (!selected) return null;
      if (current?.fileName === selected.fileName && !sameRuleDraft(current, selected)) return current;
      return { ...selected };
    });
  }, [rules, selectedFile]);

  useEffect(() => {
    if (!draft?.fileName) return;
    const saved = rules.find((rule) => rule.fileName === draft.fileName);
    if (!saved) return;
    if (sameRuleDraft(draft, saved)) {
      setAutoStatus((current) => (current === 'editing' || current === 'saving' ? 'saved' : current));
      return;
    }
    if (saveTimerRef.current != null) {
      window.clearTimeout(saveTimerRef.current);
    }
    setAutoStatus('editing');
    const snapshot = ruleSaveRequest(draft, request);
    saveTimerRef.current = window.setTimeout(() => {
      saveTimerRef.current = null;
      const seq = saveSeqRef.current + 1;
      saveSeqRef.current = seq;
      setAutoStatus('saving');
      void saveProjectRule(snapshot)
        .then((next) => {
          setRules(next);
          if (selectedFile !== snapshot.fileName) return;
          if (saveSeqRef.current === seq) setAutoStatus('saved');
        })
        .catch((err) => {
          setAutoStatus('idle');
          setError(String(err));
        });
    }, 500);
    return () => {
      if (saveTimerRef.current != null) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
      }
    };
  }, [draft, request, rules, selectedFile]);

  const updateDraft = (patch: Partial<ProjectRuleDraft>) => {
    setDraft((current) => (current ? { ...current, ...patch } : current));
  };

  const createRule = async () => {
    const before = new Set(rules.map((rule) => rule.fileName));
    setBusy(true);
    setError(null);
    try {
      const next = await saveProjectRule({
        ...request,
        fileName: null,
        title: 'New Project Rule',
        loadPolicy: 'on-demand',
        taskTrigger: 'project-specific task',
        body: '- Describe the project rule here.',
      });
      setRules(next);
      const created = next.find((rule) => !before.has(rule.fileName)) ?? next.at(-1) ?? next[0] ?? null;
      setSelectedFile(created?.fileName ?? null);
      setDraft(created ? { ...created } : null);
      setAutoStatus('saved');
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const confirmDelete = async () => {
    if (!deleteTarget) return;
    setBusy(true);
    setError(null);
    try {
      const next = await deleteProjectRule({ ...request, fileName: deleteTarget.fileName });
      setRules(next);
      setSelectedFile(next[0]?.fileName ?? null);
      setDraft(next[0] ? { ...next[0] } : null);
      setDeleteTarget(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="project-rules-medal-layer" role="presentation" onMouseDown={onClose}>
      <section
        className="project-rules-medal"
        role="dialog"
        aria-modal="true"
        aria-label="Project rules"
        onMouseDown={(event) => event.stopPropagation()}
        data-testid="project-rules-medal"
      >
        <header className="project-rules-medal-head">
          <div>
            <div className="project-rules-medal-kicker">Project Rules</div>
            <h2>Rules for all agents in {projectLabel}</h2>
            <small title={rulesDir ?? projectRoot ?? undefined}>{rulesDir || 'project-rules'}</small>
          </div>
          <button type="button" className="project-rules-close" onClick={onClose} aria-label="Close project rules">
            ×
          </button>
        </header>

        <div className="project-rules-medal-body">
          <section className="tavern-rule-list project-rule-list" aria-label="Project rules list">
            <div className="tavern-rule-list-head">
              <div>
                <div className="tavern-section-title">Rules</div>
                <div className="tavern-identity">{rules.length} file{rules.length === 1 ? '' : 's'}</div>
              </div>
              <div className="tavern-rule-list-actions">
                <button type="button" className="tavern-raw-button" disabled={busy} onClick={() => void createRule()}>
                  New
                </button>
                <button type="button" className="tavern-raw-button" disabled={busy} onClick={() => void refreshRules()}>
                  Refresh
                </button>
              </div>
            </div>
            {rules.map((rule) => (
              <button
                key={rule.fileName}
                type="button"
                className={[
                  'tavern-rule-row',
                  selectedFile === rule.fileName ? 'active' : '',
                ].filter(Boolean).join(' ')}
                onClick={() => setSelectedFile(rule.fileName)}
              >
                <span>
                  <b>{rule.title}</b>
                  <small>{rule.fileName}</small>
                </span>
                <span className="tavern-rule-badges">
                  <i>{rule.loadPolicy}</i>
                </span>
              </button>
            ))}
            {rules.length === 0 && (
              <div className="tavern-rule-empty">No project rules found.</div>
            )}
          </section>

          <section className="tavern-rule-editor project-rule-editor">
            {draft ? (
              <>
                <div className="tavern-rule-editor-scroll" data-testid="project-rule-editor-scroll">
                  <label className="tavern-profile-field">
                    Title
                    <input
                      value={draft.title}
                      onChange={(event) => updateDraft({ title: event.currentTarget.value })}
                    />
                  </label>
                  <label className="tavern-profile-field">
                    Load policy
                    <select
                      value={draft.loadPolicy}
                      onChange={(event) => updateDraft({ loadPolicy: event.currentTarget.value })}
                    >
                      <option value="always">always</option>
                      <option value="on-demand">on-demand</option>
                    </select>
                  </label>
                  {draft.loadPolicy === 'on-demand' && (
                    <label className="tavern-profile-field">
                      On-demand trigger
                      <textarea
                        className="tavern-rule-trigger"
                        rows={2}
                        wrap="soft"
                        value={draft.taskTrigger}
                        onChange={(event) => updateDraft({ taskTrigger: event.currentTarget.value })}
                        placeholder="coding, debugging, design review..."
                      />
                    </label>
                  )}
                  <label className="tavern-profile-field tavern-rule-body">
                    Body
                    <textarea
                      value={draft.body}
                      spellCheck={false}
                      onChange={(event) => updateDraft({ body: event.currentTarget.value })}
                    />
                  </label>
                </div>
                <div className="tavern-rule-editor-footer" data-testid="project-rule-editor-footer">
                  <div className="tavern-rule-meta">
                    <span title={draft.path}>{draft.path || 'New project rule file'}</span>
                    <b>{autoStatus === 'saving' ? 'Saving' : autoStatus === 'editing' ? 'Unsaved changes' : autoStatus === 'saved' ? 'Saved' : 'Auto-save'}</b>
                  </div>
                  <div className="tavern-rule-actions">
                    <button
                      type="button"
                      className="tavern-raw-button danger"
                      disabled={busy || !draft.fileName}
                      onClick={() => setDeleteTarget({ ...draft })}
                    >
                      Delete
                    </button>
                  </div>
                </div>
              </>
            ) : (
              <div className="tavern-rule-empty">Create a project rule to begin.</div>
            )}
          </section>
        </div>

        {error && <div className="project-rules-error">{error}</div>}

        {deleteTarget && (
          <div className="project-rules-delete" role="alertdialog" aria-modal="true" aria-label="Delete project rule">
            <div className="project-rules-delete-card">
              <b>Delete project rule?</b>
              <pre>{`${deleteTarget.title}\n\n${deleteTarget.fileName}`}</pre>
              <div>
                <button type="button" className="tavern-raw-button" onClick={() => setDeleteTarget(null)}>
                  Cancel
                </button>
                <button type="button" className="tavern-raw-button danger" disabled={busy} onClick={() => void confirmDelete()}>
                  Delete
                </button>
              </div>
            </div>
          </div>
        )}
      </section>
    </div>
  );
}
