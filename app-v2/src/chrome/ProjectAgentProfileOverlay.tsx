import {
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react';
import {
  inviteProjectAgentToTavern,
  listAccountSkills,
  loadProjectAgentDetail,
  openAccountSkillFolder,
  refreshProviderModelOptions,
  saveProjectAgentDetail,
  supportedShellsStatus,
  type AccountSkillDraft,
  type ProjectAgentDetail,
  type SupportedProviderModel,
  type SupportedShellStatus,
} from '../pty-client';
import {
  composeProjectAgentName,
  fullProjectAgentName,
  projectAgentNameFields,
  ProjectAgentName,
  type ProjectAgentNameFields,
} from './ProjectAgentName';
import { ProjectAgentTitlePicker } from './ProjectAgentTitlePicker';
import { HeroAvatarPicker } from './HeroAvatarPicker';
import { SkillActivationList } from './SkillActivationList';
import type { AgentId } from '../types/scene';
import iconCommends from '../assets/tavern/icons/commends.svg';
import iconGhost from '../assets/tavern/icons/ghost.svg';
import iconShell from '../assets/tavern/icons/shell.svg';
import iconSkills from '../assets/tavern/icons/skills.svg';
import iconTurns from '../assets/tavern/icons/turns.svg';
import { skillLoomEntries } from '../lib/account-skills';
import { ShellComboBox, uniqueShellComboOptions, type ShellComboOption } from './ShellComboBox';

type ProjectAgentModelCache = Record<string, {
  updatedAt: number;
  models: SupportedProviderModel[];
}>;

const PROJECT_AGENT_MODEL_CACHE_KEY = 'kota-v2.project-agent.model-catalog-cache.v2';

interface ProjectAgentProfileOverlayProps {
  agentId: AgentId;
  projectRoot?: string | null;
  existingNames: ReadonlyArray<{ id: AgentId; name: string }>;
  onClose: () => void;
  onSaved: (detail: ProjectAgentDetail) => void;
  onRemoveFromProject: (detail: ProjectAgentDetail) => void | Promise<void>;
}

export function ProjectAgentProfileOverlay({
  agentId,
  projectRoot,
  existingNames,
  onClose,
  onSaved,
  onRemoveFromProject,
}: ProjectAgentProfileOverlayProps) {
  const [detail, setDetail] = useState<ProjectAgentDetail | null>(null);
  const [displayName, setDisplayName] = useState('');
  const [model, setModel] = useState('');
  const [effort, setEffort] = useState('');
  const [shellStatuses, setShellStatuses] = useState<SupportedShellStatus[]>([]);
  const [modelCache, setModelCache] = useState<ProjectAgentModelCache>(() => loadProjectAgentModelCache());
  const [modelRefreshBusy, setModelRefreshBusy] = useState<string | null>(null);
  const [shellEditing, setShellEditing] = useState(false);
  const [avatarId, setAvatarId] = useState<string | null>(null);
  const [skills, setSkills] = useState<string[]>([]);
  const [accountSkills, setAccountSkills] = useState<AccountSkillDraft[]>([]);
  const [ghost, setGhost] = useState('');
  const [ghostExpanded, setGhostExpanded] = useState(false);
  const [nameEditing, setNameEditing] = useState(false);
  const [nameDraft, setNameDraft] = useState<ProjectAgentNameFields>({ titleId: null, given: '', middle: '', surname: '' });
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setBusy('load');
    setError(null);
    void Promise.all([
      loadProjectAgentDetail({ agentId, projectRoot }),
      listAccountSkills(),
      supportedShellsStatus().catch(() => [] as SupportedShellStatus[]),
    ])
      .then(([next, nextSkills, nextShellStatuses]) => {
        if (cancelled) return;
        setDetail(next);
        setAccountSkills(nextSkills);
        setShellStatuses(nextShellStatuses);
        const nextDisplayName = fullProjectAgentName(next.displayName, next.projectName);
        setDisplayName(nextDisplayName);
        setNameDraft(projectAgentNameFieldsFromDetail(next, nextDisplayName));
        setModel(next.model);
        setEffort(next.effort ?? '');
        setAvatarId(next.avatarId ?? null);
        setSkills(next.skills);
        setGhost(next.ghost);
        setShellEditing(false);
      })
      .catch((err) => {
        if (!cancelled) setError(String(err));
      })
      .finally(() => {
        if (!cancelled) setBusy(null);
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, projectRoot]);

  const editedDisplayName = nameEditing ? composeProjectAgentName(nameDraft) : displayName;
  const skillEntries = skillLoomEntries(accountSkills, skills);
  const duplicateName = useMemo(() => {
    const normalized = normalizedNameVariants(editedDisplayName, detail?.projectName);
    if (normalized.size === 0) return null;
    return existingNames.find(
      (item) => item.id !== agentId && intersects(normalized, normalizedNameVariants(item.name, detail?.projectName)),
    ) ?? null;
  }, [agentId, detail?.projectName, editedDisplayName, existingNames]);

  const canSaveName = !!detail && nameDraft.given.trim().length > 0 && !duplicateName && busy == null;
  const providerStatus = useMemo(() => {
    if (!detail) return null;
    return shellStatuses.find((status) => status.id === detail.provider) ?? null;
  }, [detail, shellStatuses]);
  const modelCacheEntry = detail ? modelCache[detail.provider] : undefined;
  const modelOptions = useMemo(() => projectAgentModelOptions(
    providerStatus?.modelOptions ?? [],
    modelCacheEntry?.models ?? [],
    model,
  ), [model, modelCacheEntry?.models, providerStatus?.modelOptions]);
  const effortOptions = useMemo(() => projectAgentEffortOptions(
    providerStatus?.effortOptions ?? [],
    effort,
  ), [effort, providerStatus?.effortOptions]);
  const modelCatalogStatus = detail
    ? projectAgentModelCatalogStatus(providerStatus?.modelOptions?.length ?? 0, modelCacheEntry)
    : '';
  const modelRefreshing = !!detail && modelRefreshBusy === detail.provider;

  const setSkillActive = (skillId: string, active: boolean) => {
    const selected = skills.includes(skillId);
    if (selected === active) return;
    const nextSkills = active
      ? [...skills, skillId]
      : skills.filter((id) => id !== skillId);
    setSkills(nextSkills);
  };

  const openSkillFolder = async (skill: AccountSkillDraft) => {
    setError(null);
    try {
      await openAccountSkillFolder(skill.id);
    } catch (err) {
      setError(String(err));
    }
  };

  const refreshModelCatalog = async () => {
    if (!detail || modelRefreshBusy) return;
    setModelRefreshBusy(detail.provider);
    setError(null);
    setNotice(null);
    try {
      const models = await refreshProviderModelOptions(detail.provider);
      setModelCache((current) => {
        const next = {
          ...current,
          [detail.provider]: {
            updatedAt: Date.now(),
            models,
          },
        };
        storeProjectAgentModelCache(next);
        return next;
      });
    } catch (err) {
      setError(String(err));
    } finally {
      setModelRefreshBusy((current) => (current === detail.provider ? null : current));
    }
  };

  const toggleShellEdit = async () => {
    if (!shellEditing) {
      if (detail) {
        setModel(detail.model);
        setEffort(detail.effort ?? '');
      }
      setShellEditing(true);
      setNotice(null);
      setError(null);
      return;
    }
    if (!detail || modelRefreshing) return;
    const nextModel = model.trim() || 'default';
    const nextEffort = effort.trim();
    const saved = await save({ model: nextModel, effort: nextEffort || null });
    if (saved) {
      setShellEditing(false);
    }
  };

  const save = async (patch: {
    displayName?: string;
    nameFields?: ProjectAgentNameFields | null;
    ghost?: string;
    closeGhost?: boolean;
    avatarId?: string | null;
    skills?: string[];
    model?: string;
    effort?: string | null;
  } = {}): Promise<boolean> => {
    if (!detail) return false;
    const nextDisplayName = (patch.displayName ?? displayName).trim();
    const nextGhost = patch.ghost ?? ghost;
    const nextAvatarId = patch.avatarId !== undefined ? patch.avatarId : avatarId;
    const nextSkills = patch.skills ?? detail.skills;
    const nextModel = (patch.model ?? detail.model).trim() || 'default';
    const nextEffort = patch.effort !== undefined
      ? (patch.effort ?? '').trim()
      : (detail.effort ?? '').trim();
    const nameChanged = patch.displayName !== undefined && displayName.trim() !== nextDisplayName;
    const currentNameFields = projectAgentNameFieldsFromDetail(detail, displayName);
    const nextNameFields = patch.nameFields ?? (nameChanged ? nameDraft : currentNameFields);
    const nameFieldsChanged =
      patch.nameFields !== undefined &&
      !projectAgentNameFieldsEqual(nextNameFields, currentNameFields);
    const modelChanged = detail.model !== nextModel;
    const effortChanged = (detail.effort ?? '') !== nextEffort;
    const changedNext =
      nameChanged ||
      nameFieldsChanged ||
      modelChanged ||
      effortChanged ||
      detail.ghost !== nextGhost ||
      (detail.avatarId ?? null) !== (nextAvatarId ?? null) ||
      detail.skills.join('\n') !== nextSkills.join('\n');
    if (!changedNext || duplicateName || !nextDisplayName.trim()) {
      if (patch.closeGhost) setGhostExpanded(false);
      return !duplicateName && !!nextDisplayName.trim();
    }
    setBusy('save');
    setError(null);
    setNotice(null);
    try {
      const next = await saveProjectAgentDetail({
        agentId,
        projectRoot,
        displayName: nextDisplayName,
        nameFields: nextNameFields,
        model: nextModel,
        effort: nextEffort || null,
        avatarId: nextAvatarId,
        skills: nextSkills,
        ghost: nextGhost,
      });
      setDetail(next);
      const savedDisplayName = fullProjectAgentName(next.displayName, next.projectName);
      setDisplayName(savedDisplayName);
      setNameDraft(projectAgentNameFieldsFromDetail(next, savedDisplayName));
      setNameEditing(false);
      if (patch.model !== undefined) setModel(next.model);
      if (patch.effort !== undefined) setEffort(next.effort ?? '');
      setAvatarId(next.avatarId ?? null);
      setSkills((current) => (patch.skills !== undefined ? next.skills : current));
      setGhost(next.ghost);
      onSaved(next);
      setNotice('Saved');
      if (patch.closeGhost) setGhostExpanded(false);
      if (patch.skills !== undefined) {
        window.dispatchEvent(new Event('kota:file-tree-refresh'));
      }
      return true;
    } catch (err) {
      setError(String(err));
      return false;
    } finally {
      setBusy(null);
    }
  };

  const flushSkillsAndClose = useCallback(async () => {
    if (busy != null) return;
    if (!detail || detail.skills.join('\n') === skills.join('\n')) {
      onClose();
      return;
    }
    const saved = await save({ skills });
    if (saved) onClose();
  }, [busy, detail, onClose, save, skills]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      if (event.target instanceof Element && event.target.closest('.project-agent-combo[data-open="true"]')) return;
      event.preventDefault();
      event.stopPropagation();
      void flushSkillsAndClose();
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, [flushSkillsAndClose]);

  const invite = async () => {
    if (!detail || !detail.inviteEligibility.eligible) return;
    setBusy('invite');
    setError(null);
    setNotice(null);
    try {
      const result = await inviteProjectAgentToTavern({
        agentId,
        projectRoot,
        displayName: detail.inviteEligibility.proposedDisplayName,
      });
      setNotice(`Invited as ${result.displayName}`);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  };

  const removeFromProject = async () => {
    if (!detail) return;
    setBusy('remove');
    setError(null);
    setNotice(null);
    try {
      await onRemoveFromProject(detail);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  };

  const inviteLabel = inviteButtonLabel(detail, busy);
  const inviteReason = detail?.inviteEligibility.reason
    ?? (detail?.inviteEligibility.eligible ? 'Ready to invite this GHOST into Tavern.' : null);
  const profileDisplayName = displayName || detail?.displayName || agentId;
  const shellProvider = projectAgentShellProviderMeta(detail);
  const startNameEdit = () => {
    setNameDraft(detail ? projectAgentNameFieldsFromDetail(detail, displayName) : projectAgentNameFields(displayName));
    setNameEditing(true);
  };
  const cancelNameEdit = () => {
    setNameDraft(detail ? projectAgentNameFieldsFromDetail(detail, displayName) : projectAgentNameFields(displayName));
    setNameEditing(false);
  };
  const confirmNameEdit = () => {
    const nextName = composeProjectAgentName(nameDraft);
    if (!canSaveName || !nextName.trim()) return;
    const currentNameFields = detail ? projectAgentNameFieldsFromDetail(detail, displayName) : projectAgentNameFields(displayName);
    if (
      nextName.trim() === displayName.trim() &&
      projectAgentNameFieldsEqual(nameDraft, currentNameFields)
    ) {
      setNameEditing(false);
      return;
    }
    void save({ displayName: nextName, nameFields: nameDraft });
  };

  return (
    <section className="tavern-profile-overlay" role="dialog" aria-label="Project agent detail">
      <div className="tavern-profile-card project-agent-profile">
        <button type="button" className="tavern-profile-back" onClick={() => void flushSkillsAndClose()}>
          Back
        </button>
        <div className="tavern-merit-bar project-agent-merits">
          <MeritBadge image={iconTurns} label="Turns" value={String(detail?.record.turns ?? 0)} />
          <MeritBadge
            image={iconCommends}
            label="Commends"
            value={formatCompact(detail?.record.commends ?? 0)}
          />
        </div>

        <div className="tavern-profile-layout">
          <section className="tavern-profile-panel shell-panel project-agent-shell-panel" aria-label="SHELL">
            <PanelTitle icon={iconShell}>SHELL</PanelTitle>
            <div className="project-agent-shell-core">
              <div
                className={`project-agent-shell-provider ${shellProvider.className}`}
                aria-label={`Provider ${shellProvider.label}`}
              >
                <span className="project-agent-shell-provider-icon" aria-hidden="true" />
                <span className="project-agent-shell-provider-copy">
                  <span className="project-agent-shell-provider-label">Provider</span>
                  <span className="project-agent-shell-provider-name">{shellProvider.label}</span>
                </span>
              </div>
              {shellEditing ? (
                <div className="project-agent-shell-edit-fields">
                  <div className="tavern-profile-field">
                    <span>Model</span>
                    <ShellComboBox
                      value={model}
                      options={modelOptions}
                      placeholder="Exact model ID"
                      disabled={!detail || busy != null}
                      status={modelRefreshing ? 'Fetching model IDs...' : modelCatalogStatus}
                      refreshing={modelRefreshing}
                      onChange={setModel}
                      onRefresh={() => void refreshModelCatalog()}
                    />
                  </div>
                  <div className="tavern-profile-field">
                    <span>Effort</span>
                    <ShellComboBox
                      value={effort}
                      options={effortOptions}
                      placeholder="default"
                      disabled={!detail || busy != null}
                      onChange={setEffort}
                    />
                  </div>
                </div>
              ) : (
                <div className="project-agent-shell-summary" aria-label="Current SHELL settings">
                  <code title={model || 'default'}>{model || 'default'}</code>
                  <code title={effort || 'default'}>{effort || 'default'}</code>
                </div>
              )}
            </div>
            {shellEditing && (
              <div className="project-agent-shell-apply-note">Apply on Next Launch</div>
            )}
            <div className="tavern-profile-actions">
              <button type="button" onClick={() => void toggleShellEdit()} disabled={!detail || busy != null}>
                {shellEditing ? 'Save SHELL' : 'Edit SHELL'}
              </button>
            </div>
          </section>

          <section className="tavern-profile-center" aria-label="Project agent identity">
            {nameEditing && (
              <ProjectAgentTitlePicker
                titleId={nameDraft.titleId}
                disabled={busy != null}
                onChange={(nextId) => setNameDraft((prev) => ({ ...prev, titleId: nextId }))}
              />
            )}
            {detail && (
              <HeroAvatarPicker
                provider={detail.provider || detail.cli}
                value={avatarId}
                className="profile"
                disabled={busy != null}
                onChange={(nextAvatarId) => {
                  setAvatarId(nextAvatarId);
                  void save({ avatarId: nextAvatarId });
                }}
              />
            )}
            {nameEditing ? (
              <div className="project-agent-name-editor" aria-label="Edit agent name">
                <label>
                  <span>Given name</span>
                  <input
                    value={nameDraft.given}
                    onChange={(event) => {
                      const value = event.currentTarget.value;
                      setNameDraft((prev) => ({ ...prev, given: value }));
                    }}
                    autoFocus
                  />
                </label>
                <label>
                  <span>Middle name</span>
                  <input
                    value={nameDraft.middle}
                    onChange={(event) => {
                      const value = event.currentTarget.value;
                      setNameDraft((prev) => ({ ...prev, middle: value }));
                    }}
                  />
                </label>
                <label className="surname-field">
                  <span>Surname</span>
                  <input
                    value={nameDraft.surname}
                    onChange={(event) => {
                      const value = event.currentTarget.value;
                      setNameDraft((prev) => ({ ...prev, surname: value }));
                    }}
                  />
                </label>
                <div className="project-agent-name-editor-actions">
                  <button
                    type="button"
                    className="accept"
                    disabled={!canSaveName}
                    onClick={confirmNameEdit}
                    aria-label="Save agent name"
                  >
                    ✓
                  </button>
                  <button type="button" onClick={cancelNameEdit} aria-label="Cancel agent name edit">
                    x
                  </button>
                </div>
              </div>
            ) : (
              <button
                type="button"
                className="profile-name-button"
                onClick={startNameEdit}
                disabled={!detail}
                title={profileDisplayName}
                data-full-name={profileDisplayName}
              >
                <ProjectAgentName
                  name={profileDisplayName}
                  projectName={detail?.projectName}
                  titleLine
                  className="profile-name-display"
                />
              </button>
            )}
            {duplicateName && (
              <div className="project-agent-error">Name already exists in this project.</div>
            )}
            <section className={`tavern-ghost-scroll ${ghostExpanded ? 'expanded' : ''}`}>
              <PanelTitle icon={iconGhost}>GHOST</PanelTitle>
              {ghostExpanded ? (
                <textarea
                  value={ghost}
                  onChange={(event) => setGhost(event.currentTarget.value)}
                  spellCheck={false}
                  autoFocus
                />
              ) : (
                <button type="button" className="tavern-ghost-preview" onClick={() => setGhostExpanded(true)}>
                  {ghost.trim() || 'No GHOST configured.'}
                </button>
              )}
              <div className="tavern-profile-actions">
                <button
                  type="button"
                  onClick={() => {
                    if (ghostExpanded) void save({ closeGhost: true });
                    else setGhostExpanded(true);
                  }}
                  disabled={ghostExpanded && busy != null}
                >
                  {ghostExpanded ? 'Save GHOST' : 'Edit GHOST'}
                </button>
              </div>
            </section>
          </section>

          <section className="tavern-profile-panel skills-panel" aria-label="SKILLS">
            <PanelTitle icon={iconSkills}>SKILLS</PanelTitle>
            {skillEntries.length === 0 ? (
              <div className="tavern-rule-empty">No skills in $KOTA_HOME/skills.</div>
            ) : (
              <SkillActivationList
                entries={skillEntries}
                disabled={!detail}
                onChange={setSkillActive}
                onOpenSkillFolder={openSkillFolder}
              />
            )}
          </section>
        </div>
        <div className="project-agent-invite-row">
          <div className="project-agent-invite-copy">
            <span>Invite to Tavern</span>
            <small>{inviteReason ?? 'GHOST is not ready for Tavern.'}</small>
          </div>
          <button
            type="button"
            className="project-agent-invite"
            disabled={!detail?.inviteEligibility.eligible || busy != null}
            title={detail?.inviteEligibility.reason ?? undefined}
            onClick={() => void invite()}
          >
            {inviteLabel}
          </button>
        </div>
        <button
          type="button"
          className="tavern-remove-quiet project-agent-remove"
          disabled={!detail || busy != null}
          onClick={() => void removeFromProject()}
        >
          {busy === 'remove' ? 'Removing' : 'Remove from project'}
        </button>
        {(busy || error || notice) && (
          <div className={`tavern-status ${error ? 'error' : ''}`}>
            {error ?? notice ?? (busy ? `Working: ${busy}` : null)}
          </div>
        )}
      </div>
    </section>
  );
}

function MeritBadge({ image, label, value }: { image: string; label: string; value: string }) {
  return (
    <div className="tavern-merit-badge">
      <img src={image} alt="" aria-hidden />
      <span>
        <b>{value}</b>
        <small>{label}</small>
      </span>
    </div>
  );
}

function PanelTitle({ icon, children }: { icon: string; children: string }) {
  return (
    <h3 className="tavern-profile-panel-title">
      <img src={icon} alt="" />
      <span>{children}</span>
    </h3>
  );
}

function formatCompact(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0';
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}m`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
  return String(value);
}

function inviteButtonLabel(detail: ProjectAgentDetail | null, busy: string | null): string {
  if (busy === 'invite') return 'Inviting';
  if (!detail) return 'Invite to Tavern';
  if (!detail.inviteEligibility.eligible && detail.inviteEligibility.duplicateHeroId) {
    return 'Already in Tavern';
  }
  return 'Invite to Tavern';
}

function projectAgentShellProviderMeta(detail: ProjectAgentDetail | null): { label: string; className: string } {
  const provider = (detail?.provider || detail?.cli || 'provider').trim().toLowerCase();
  if (['claude', 'claude-code', 'claude_code'].includes(provider)) {
    return { label: 'Claude Code', className: 'provider-claude' };
  }
  if (['codex', 'openai'].includes(provider)) {
    return { label: 'Codex', className: 'provider-codex' };
  }
  if (['antigravity', 'agy'].includes(provider)) {
    return { label: 'Antigravity', className: 'provider-gemini' };
  }
  if (['gemini', 'gemini-cli', 'googlegemini'].includes(provider)) {
    return { label: 'Gemini', className: 'provider-gemini' };
  }
  if (['opencode', 'open-code'].includes(provider)) {
    return { label: 'OpenCode', className: 'provider-opencode' };
  }
  if (provider === 'pi') {
    return { label: 'Pi', className: 'provider-pi' };
  }
  if (['kimi', 'kimi-code'].includes(provider)) {
    return { label: 'Kimi Code', className: 'provider-kimi' };
  }
  if (provider === 'github') {
    return { label: 'GitHub', className: 'provider-github' };
  }
  return {
    label: detail?.provider || detail?.cli || 'Provider',
    className: 'provider-generic',
  };
}

function normalizedNameVariants(name: unknown, projectName?: string | null): Set<string> {
  const trimmed = (typeof name === 'string' ? name : String(name ?? '')).trim();
  if (!trimmed) return new Set();
  return new Set([
    trimmed.toLowerCase(),
    fullProjectAgentName(trimmed, projectName).toLowerCase(),
  ]);
}

function intersects(a: ReadonlySet<string>, b: ReadonlySet<string>): boolean {
  for (const value of a) {
    if (b.has(value)) return true;
  }
  return false;
}

function projectAgentNameFieldsFromDetail(
  detail: Pick<ProjectAgentDetail, 'nameFields' | 'projectName'>,
  displayName: string,
): ProjectAgentNameFields {
  const parsed = projectAgentNameFields(displayName, detail.projectName);
  return {
    titleId: detail.nameFields?.titleId ?? parsed.titleId,
    given: detail.nameFields?.given?.trim() || parsed.given,
    middle: detail.nameFields?.middle ?? parsed.middle,
    surname: detail.nameFields?.surname ?? parsed.surname,
  };
}

function projectAgentNameFieldsEqual(
  left: ProjectAgentNameFields | null | undefined,
  right: ProjectAgentNameFields | null | undefined,
): boolean {
  return (
    (left?.titleId ?? null) === (right?.titleId ?? null) &&
    (left?.given ?? '').trim() === (right?.given ?? '').trim() &&
    (left?.middle ?? '').trim() === (right?.middle ?? '').trim() &&
    (left?.surname ?? '').trim() === (right?.surname ?? '').trim()
  );
}

function loadProjectAgentModelCache(): ProjectAgentModelCache {
  if (typeof window === 'undefined') return {};
  try {
    const raw = window.localStorage.getItem(PROJECT_AGENT_MODEL_CACHE_KEY);
    if (!raw) return {};
    return normalizeProjectAgentModelCache(JSON.parse(raw));
  } catch {
    return {};
  }
}

function storeProjectAgentModelCache(cache: ProjectAgentModelCache) {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(PROJECT_AGENT_MODEL_CACHE_KEY, JSON.stringify(cache));
  } catch {
    // Cache failure should not block editing the launch config.
  }
}

function normalizeProjectAgentModelCache(value: unknown): ProjectAgentModelCache {
  if (!value || typeof value !== 'object') return {};
  const cache: ProjectAgentModelCache = {};
  for (const [provider, entry] of Object.entries(value as Record<string, unknown>)) {
    if (!provider || !entry || typeof entry !== 'object') continue;
    const record = entry as { updatedAt?: unknown; models?: unknown };
    const models = Array.isArray(record.models)
      ? record.models.flatMap((item) => normalizeProjectAgentModelOption(item))
      : [];
    if (models.length === 0) continue;
    cache[provider] = {
      updatedAt: typeof record.updatedAt === 'number' && Number.isFinite(record.updatedAt)
        ? record.updatedAt
        : 0,
      models,
    };
  }
  return cache;
}

function normalizeProjectAgentModelOption(value: unknown): SupportedProviderModel[] {
  if (!value || typeof value !== 'object') return [];
  const record = value as { id?: unknown; label?: unknown; source?: unknown };
  const id = typeof record.id === 'string' ? record.id.trim() : '';
  if (!id) return [];
  return [{
    id,
    label: typeof record.label === 'string' && record.label.trim() ? record.label.trim() : id,
    source: typeof record.source === 'string' && record.source.trim() ? record.source.trim() : 'cached',
  }];
}

function projectAgentModelOptions(
  seed: SupportedProviderModel[],
  cached: SupportedProviderModel[],
  selected: string,
): ShellComboOption[] {
  return uniqueShellComboOptions([
    ...seed.map((option) => ({
      id: option.id,
      label: option.label || option.id,
      source: option.source || 'seed',
    })),
    ...cached.map((option) => ({
      id: option.id,
      label: option.label || option.id,
      source: option.source || 'cached',
    })),
    selected.trim() ? { id: selected.trim(), label: selected.trim(), source: 'current' } : null,
  ]);
}

function projectAgentEffortOptions(
  seed: SupportedShellStatus['effortOptions'],
  selected: string,
): ShellComboOption[] {
  return uniqueShellComboOptions([
    ...seed.map((option) => ({
      id: option.value,
      label: option.label || option.value,
      source: 'provider',
    })),
    selected.trim() ? { id: selected.trim(), label: selected.trim(), source: 'current' } : null,
  ]);
}

function projectAgentModelCatalogStatus(seedCount: number, cacheEntry?: ProjectAgentModelCache[string]): string {
  const cachedCount = cacheEntry?.models.length ?? 0;
  if (cacheEntry) {
    const age = projectAgentCatalogAge(cacheEntry.updatedAt);
    return `${seedCount} seed · ${cachedCount} cached${age ? ` · ${age}` : ''}`;
  }
  return `${seedCount} seed · never updated`;
}

function projectAgentCatalogAge(updatedAt: number): string {
  if (!Number.isFinite(updatedAt) || updatedAt <= 0) return '';
  const minutes = Math.max(0, Math.floor((Date.now() - updatedAt) / 60_000));
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}
