import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  inviteProjectAgentToTavern,
  listAccountSkills,
  loadProjectAgentDetail,
  openAccountSkillFolder,
  openProjectAgentNotes,
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
import { syncTavernHeroStorageFromDisk } from './TavernModal';
import type { AgentId } from '../types/scene';
import iconCommends from '../assets/tavern/icons/commends.svg';
import iconGhost from '../assets/tavern/icons/ghost.svg';
import iconShell from '../assets/tavern/icons/shell.svg';
import iconSkills from '../assets/tavern/icons/skills.svg';
import iconTurns from '../assets/tavern/icons/turns.svg';
import { skillLoomEntries } from '../lib/account-skills';
import { ShellComboBox, uniqueShellComboOptions, type ShellComboOption } from './ShellComboBox';
import { LocalModelCredit, localModelProvider, localModelOptionCredit } from './LocalModelCredit';
import { KOTA_LOCAL_ENABLED } from '../lib/local-rpc';
import { onLocalConfiguration, localConfigurationSaveFailure } from '../lib/local-configuration';

import { catalogModel, linkedCatalogProviders, localThinkingForModel, requestKotaLocalLink } from '../lib/local-auth';
import { useTavernLocal } from '../lib/tavern-local';
import { localModelLabel, localModelOption } from '../lib/local-magpie';
import { useJevSettings } from '../lib/jev';
import { AUTO_ROUTE_MODEL, stringDraftComplete, stringGroupToDraft, stringErrorText, type AutoRouteGroup } from '../lib/string-routing';
import { StringLocalModelPicker } from './StringLocalModelPicker';

type ProjectAgentModelCache = Record<string, {
  updatedAt: number;
  models: SupportedProviderModel[];
}>;

const SHELL_PROVIDERS = [
  ['claude', 'Claude Code'], ['codex', 'Codex'], ['antigravity', 'Antigravity'],
  ['opencode', 'OpenCode'], ['pi', 'Pi'], ['kimi', 'Kimi Code'],
  ...(KOTA_LOCAL_ENABLED ? [['kota-local', 'Kota Local']] as const : []),
] as const;

const PROJECT_AGENT_MODEL_CACHE_KEY = 'kota-v2.project-agent.model-catalog-cache.v2';

interface ProjectAgentProfileOverlayProps {
  agentId: AgentId;
  projectRoot?: string | null;
  existingNames: ReadonlyArray<{ id: AgentId; name: string }>;
  onClose: () => void;
  onSaved: (detail: ProjectAgentDetail, previousProvider: string) => void;
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
  const [provider, setProvider] = useState('');
  const [model, setModel] = useState('');
  const [effort, setEffort] = useState('');
  const [autoRoute, setAutoRoute] = useState<AutoRouteGroup | null>(null);
  const [stringValid, setStringValid] = useState(true);
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
  const [openingNotes, setOpeningNotes] = useState(false);
  const inviteInFlight = useRef(false);
  const saveInFlight = useRef(false);
  const [reload, setReload] = useState(0);
  const shellEditingNow = useRef(shellEditing); shellEditingNow.current = shellEditing;
  const shellReadSerial = useRef(0);
  const acceptSavedShell = useCallback((next: ProjectAgentDetail) => {
    setDetail(current => current ? { ...current, provider: next.provider, model: next.model,
      effort: next.effort, autoRoute: next.autoRoute, shellRevision: next.shellRevision } : current);
    setProvider(next.provider); setModel(next.model); setEffort(next.effort ?? '');
    setAutoRoute(next.autoRoute ?? null); setStringValid(true);
  }, []);
  useEffect(() => {
    if (!projectRoot || detail?.provider !== 'kota-local') return;
    let disposed = false;
    let off: (() => void) | undefined;
    void onLocalConfiguration(projectRoot, agentId, () => {
      if (disposed || shellEditingNow.current || saveInFlight.current) return;
      const serial = ++shellReadSerial.current;
      void loadProjectAgentDetail({ projectRoot, agentId }, { readOnly: true }).then(next => {
        if (!disposed && serial === shellReadSerial.current && !shellEditingNow.current && !saveInFlight.current) acceptSavedShell(next);
      }).catch(() => {
        if (!disposed && serial === shellReadSerial.current && !shellEditingNow.current && !saveInFlight.current) setError('Could not refresh saved settings. Reopen this profile.');
      });
    }).then(unsubscribe => { if (disposed) unsubscribe(); else off = unsubscribe; }).catch(() => {});
    return () => { disposed = true; ++shellReadSerial.current; off?.(); };
  }, [projectRoot, agentId, detail?.provider, acceptSavedShell]);

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
        setProvider(next.provider);
        setModel(next.model);
        setEffort(next.effort ?? '');
        setAutoRoute(next.autoRoute ?? null); setStringValid(true);
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
  }, [agentId, projectRoot, reload]);

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
    return shellStatuses.find((status) => status.id === provider) ?? null;
  }, [detail, provider, shellStatuses]);
  const editingLocal = provider === 'kota-local';
  const local = useTavernLocal(editingLocal && (shellEditing || model === AUTO_ROUTE_MODEL));
  const jev = useJevSettings(editingLocal && shellEditing);
  const automatic = editingLocal && model === AUTO_ROUTE_MODEL;
  const modelCacheEntry = detail ? modelCache[provider] : undefined;
  const localModels = linkedCatalogProviders(local.catalog.loading || local.catalog.failed ? null : local.catalog.data)
    .flatMap(provider => provider.models.map(model => localModelOption(provider.providerId, model.modelId, model.name)));
  const slash = model.indexOf('/');
  const localModel = catalogModel(local.catalog.data, model.slice(0, slash), model.slice(slash + 1));
  const terminalModelOptions = useMemo(() => projectAgentModelOptions(
    providerStatus?.modelOptions ?? [],
    modelCacheEntry?.models ?? [],
    model,
  ), [model, modelCacheEntry?.models, providerStatus?.modelOptions]);
  const terminalEffortOptions = useMemo(() => projectAgentEffortOptions(
    providerStatus?.effortOptions ?? [],
    effort,
  ), [effort, providerStatus?.effortOptions]);
  const modelOptions = editingLocal ? localModels : terminalModelOptions;
  const effortOptions = editingLocal ? (localModel?.thinkingLevels ?? []).map(level => ({ id: level, label: level, source: 'provider' })) : terminalEffortOptions;
  const modelCatalogStatus = editingLocal ? local.catalog.failed ? 'Model catalog unavailable'
    : model.startsWith('magpie/') && !localModels.some(option => option.id === model) ? 'Magpie model unavailable'
    : `${localModels.length} ${localModels.length === 1 ? 'model' : 'models'} from signed-in providers` : detail
    ? projectAgentModelCatalogStatus(providerStatus?.modelOptions?.length ?? 0, modelCacheEntry) : '';
  const modelRefreshing = editingLocal ? local.catalog.loading : !!detail && modelRefreshBusy === provider;
  const localSelectionReady = automatic ? stringValid && !!jev.settings?.ready && !jev.loading && !jev.failed
    && !!autoRoute && stringDraftComplete(stringGroupToDraft(autoRoute))
    : !local.catalog.loading && !local.catalog.failed
      && localModels.some(option => option.id === model.trim()) && !!localModel?.thinkingLevels.includes(effort);
  const localMissingModel = editingLocal && (!model.trim() || model.trim().toLowerCase() === 'default');
  useEffect(() => {
    if (shellEditing && editingLocal && localModel && !localModel.thinkingLevels.includes(effort)) {
      setEffort(localThinkingForModel(localModel.thinkingLevels, effort));
    }
  }, [shellEditing, editingLocal, localModel, effort]);
  const selectModel = (value: string) => {
    setModel(value); setStringValid(true);
    if (editingLocal) {
      const slash = value.indexOf('/');
      const selected = catalogModel(local.catalog.data, value.slice(0, slash), value.slice(slash + 1));
      if (selected) setEffort(localThinkingForModel(selected.thinkingLevels, effort));
    }
  };

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

  const openNotes = async () => {
    if (!detail?.notes?.exists || openingNotes) return;
    setOpeningNotes(true);
    setError(null);
    try {
      await openProjectAgentNotes({ agentId, projectRoot });
    } catch (err) {
      setError(String(err));
    } finally {
      setOpeningNotes(false);
    }
  };

  const refreshModelCatalog = async () => {
    if (!detail || modelRefreshBusy) return;
    setModelRefreshBusy(provider);
    setError(null);
    setNotice(null);
    try {
      const models = await refreshProviderModelOptions(provider);
      setModelCache((current) => {
        const next = {
          ...current,
          [provider]: {
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
      setModelRefreshBusy((current) => (current === provider ? null : current));
    }
  };

  const toggleShellEdit = async () => {
    if (!shellEditing) {
      if (detail) {
        setProvider(detail.provider);
        const selected = detail.provider === 'kota-local' && detail.model.trim().toLowerCase() === 'default' ? '' : detail.model;
        setModel(selected);
        setAutoRoute(detail.autoRoute ?? null); setStringValid(true);
        const slash = selected.indexOf('/');
        const localModel = catalogModel(local.catalog.data, selected.slice(0, slash), selected.slice(slash + 1));
        setEffort(detail.provider === 'kota-local' ? selected === AUTO_ROUTE_MODEL ? 'off' : localModel ? localThinkingForModel(localModel.thinkingLevels, detail.effort) : detail.effort ?? '' : detail.effort ?? '');
      }
      setShellEditing(true);
      setNotice(null);
      setError(null);
      return;
    }
    if (!detail || (!automatic && modelRefreshing) || (editingLocal && !localSelectionReady)) return;
    const nextModel = model.trim() || 'default';
    const nextEffort = effort.trim();
    const saved = await save({ provider, model: nextModel, effort: automatic ? 'off' : nextEffort || null,
      ...(automatic ? { autoRoute } : {}) });
    if (saved) {
      setShellEditing(false);
    }
  };

  const save = async (patch: {
    provider?: string;
    displayName?: string;
    nameFields?: ProjectAgentNameFields | null;
    ghost?: string;
    closeGhost?: boolean;
    avatarId?: string | null;
    skills?: string[];
    model?: string;
    effort?: string | null;
    autoRoute?: AutoRouteGroup | null;
  } = {}): Promise<boolean> => {
    if (!detail || saveInFlight.current) return false;
    const nextProvider = patch.provider ?? detail.provider;
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
    const nextAutoRoute = patch.autoRoute !== undefined ? patch.autoRoute : detail.autoRoute;
    const routeChanged = nextProvider === 'kota-local' && nextModel === AUTO_ROUTE_MODEL
      && JSON.stringify(nextAutoRoute) !== JSON.stringify(detail.autoRoute);
    const changedNext =
      nextProvider !== detail.provider ||
      nameChanged ||
      nameFieldsChanged ||
      modelChanged ||
      effortChanged ||
      routeChanged ||
      detail.ghost !== nextGhost ||
      (detail.avatarId ?? null) !== (nextAvatarId ?? null) ||
      detail.skills.join('\n') !== nextSkills.join('\n');
    if (!changedNext || duplicateName || !nextDisplayName.trim()) {
      if (patch.closeGhost) setGhostExpanded(false);
      return !duplicateName && !!nextDisplayName.trim();
    }
    saveInFlight.current = true;
    setBusy('save');
    setError(null);
    setNotice(null);
    try {
      const next = await saveProjectAgentDetail({
        agentId,
        projectRoot,
        displayName: nextDisplayName,
        nameFields: nextNameFields,
        provider: nextProvider,
        shellRevision: detail.shellRevision,
        model: nextModel,
        effort: nextEffort || null,
        ...(nextProvider === 'kota-local' && nextModel === AUTO_ROUTE_MODEL ? { autoRoute: nextAutoRoute } : {}),
        avatarId: nextAvatarId,
        skills: nextSkills,
        ghost: nextGhost,
      });
      setDetail(next);
      const savedDisplayName = fullProjectAgentName(next.displayName, next.projectName);
      setDisplayName(savedDisplayName);
      setNameDraft(projectAgentNameFieldsFromDetail(next, savedDisplayName));
      setNameEditing(false);
      if (patch.provider !== undefined) setProvider(next.provider);
      if (patch.model !== undefined) setModel(next.model);
      if (patch.effort !== undefined) setEffort(next.effort ?? '');
      if (patch.model !== undefined || patch.autoRoute !== undefined) { setAutoRoute(next.autoRoute ?? null); setStringValid(true); }
      setAvatarId(next.avatarId ?? null);
      setSkills((current) => (patch.skills !== undefined ? next.skills : current));
      setGhost(next.ghost);
      onSaved(next, detail.provider);
      setNotice(next.appliedToSession === true ? 'Saved and applied to the running session.'
        : next.appliedToSession === false ? 'Saved. Applies on next launch.' : 'Saved');
      if (patch.closeGhost) setGhostExpanded(false);
      if (patch.skills !== undefined) {
        window.dispatchEvent(new Event('kota:file-tree-refresh'));
      }
      return true;
    } catch (err) {
      const failure = localConfigurationSaveFailure(err);
      let refreshFailed = false;
      if (failure?.reload) {
        try { acceptSavedShell(await loadProjectAgentDetail({ projectRoot, agentId }, { readOnly: true })); }
        catch { refreshFailed = true; }
      }
      setError(failure ? `${failure.message}${refreshFailed ? ' Could not reload saved settings. Reopen this profile.' : ''}`
        : String(err) === 'local_config_changed' ? 'Settings changed outside this editor. Reload before saving.'
        : nextProvider === 'kota-local' && nextModel === AUTO_ROUTE_MODEL ? stringErrorText(err) : String(err));
      return false;
    } finally {
      saveInFlight.current = false;
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
    if (inviteInFlight.current || busy != null || !detail?.inviteEligibility.eligible) return;
    inviteInFlight.current = true;
    setBusy('invite');
    setError(null);
    setNotice(null);
    try {
      const result = await inviteProjectAgentToTavern({
        agentId,
        projectRoot,
      });
      setDetail((current) => current?.agentYamlPath === detail.agentYamlPath ? {
        ...current,
        inviteEligibility: {
          eligible: false,
          reason: 'This incarnation is already in Tavern.',
          duplicateHeroId: result.heroId,
          proposedHeroId: result.heroId,
          proposedDisplayName: result.displayName,
        },
      } : current);
      setNotice(`Invited as ${result.displayName}`);
      try {
        await syncTavernHeroStorageFromDisk();
      } catch {
        // The invitation is already saved; a cache refresh failure must not invite again.
        setNotice(`Invited as ${result.displayName}. Recruit list could not refresh; open Tavern to refresh it.`);
      }
    } catch (err) {
      setError(String(err));
    } finally {
      inviteInFlight.current = false;
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
  const shellProvider = projectAgentShellProviderMeta(detail ? { ...detail, provider } : null);
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
                  {shellEditing ? (
                    <select className="project-agent-shell-provider-select" aria-label="Provider" value={provider} disabled={busy != null}
                      onChange={(event) => {
                        const next = event.target.value;
                        setProvider(next);
                        setModel(next === 'kota-local' ? next === detail?.provider && detail.model.toLowerCase() !== 'default' ? detail.model : '' : next === detail?.provider ? detail.model : 'default');
                        setEffort(next === detail?.provider ? detail.effort ?? '' : '');
                        setStringValid(true);
                      }}>
                      {SHELL_PROVIDERS.map(([id, label]) => <option key={id} value={id}>{label}</option>)}
                    </select>
                  ) : <span className="project-agent-shell-provider-name">{shellProvider.label}</span>}
                </span>
              </div>
              {shellEditing ? (
                <div className="project-agent-shell-edit-fields">
                  <div className="tavern-profile-field">
                    <span>Model</span>
                    {editingLocal ? <>
                      <StringLocalModelPicker key={`${agentId}:${projectRoot}`} ownerKey={`${agentId}:${projectRoot}`} owner={displayName} surface="profile"
                        applySource={{ projectRoot: projectRoot ?? undefined, agentId }}
                        ruleFile={projectRoot ? { projectRoot, agentId, kind: 'string' } : undefined} ruleFileDirty={JSON.stringify(autoRoute) !== JSON.stringify(detail?.autoRoute ?? null)}
                        model={model} group={autoRoute} catalog={local.catalog.data} jev={jev} disabled={!detail || busy != null}
                        onSelect={choice => selectModel(`${choice.providerId}/${choice.modelId}`)}
                        onAutoRouteChange={group => { setAutoRoute(group); setModel(AUTO_ROUTE_MODEL); setEffort('off'); }}
                        onValidityChange={setStringValid} onOpen={() => void local.openModels()}
                        onJevSettings={() => requestKotaLocalLink('puppeteer-jev')} />
                      {local.catalog.failed && <button type="button" className="local-model-text" onClick={() => void local.catalog.refresh()}>Update model list</button>}
                      {!automatic && <small role="status">{modelRefreshing ? 'Fetching model IDs...' : modelCatalogStatus}</small>}
                    </> : <ShellComboBox
                      value={model}
                      options={modelOptions}
                      optionSuffix={editingLocal ? localModelOptionCredit : undefined}
                      placeholder={editingLocal ? 'Choose a model' : 'Exact model ID'}
                      disabled={!detail || busy != null}
                      status={modelRefreshing ? 'Fetching model IDs...' : modelCatalogStatus}
                      refreshing={modelRefreshing}
                      onChange={selectModel}
                      onOpen={editingLocal ? () => void local.openModels() : undefined}
                      onRefresh={() => void (editingLocal ? local.catalog.refresh() : refreshModelCatalog())}
                    />}
                  </div>
                  <div className="tavern-profile-field">
                    <span>{editingLocal ? 'Thinking' : 'Effort'}</span>
                    {editingLocal ? <select aria-label="Thinking" value={automatic ? 'off' : effort}
                      disabled={!detail || busy != null || automatic || !localModel || local.catalog.loading || local.catalog.failed}
                      onChange={event => setEffort(event.target.value)}>
                      {automatic ? <option value="off">By Auto String</option> : <>
                        {!localModel?.thinkingLevels.includes(effort) && <option value={effort}>{localModel ? effort : 'Choose a model first'}</option>}
                        {localModel?.thinkingLevels.map(level => <option key={level} value={level}>{level}</option>)}
                      </>}
                    </select> : <ShellComboBox
                      value={editingLocal && !localModel ? '' : effort}
                      options={effortOptions}
                      placeholder={editingLocal ? localModel ? '' : 'Choose a model first' : 'default'}
                      disabled={!detail || busy != null || (editingLocal && (!localModel || local.catalog.loading || local.catalog.failed))}
                      onChange={setEffort}
                    />}
                  </div>
                </div>
              ) : (
                <div className="project-agent-shell-summary" aria-label="Current SHELL settings">
                  <code title={model || 'default'}>{automatic ? 'Auto String' : editingLocal && model.startsWith('magpie/') ? localModelLabel('magpie', model.slice(7), localModel?.name) : model || 'default'}{editingLocal && !automatic && <LocalModelCredit providerId={localModelProvider(model)} />}</code>
                  <code title={effort || 'default'}>{automatic ? 'By Auto String' : effort || 'default'}</code>
                </div>
              )}
            </div>
            {shellEditing && (
              <div className="project-agent-shell-apply-note">{editingLocal && !stringValid ? 'Finish the Auto String configuration before saving.' : localMissingModel ? 'Choose a model before saving.' : editingLocal ? 'Save applies to a running Local session, or to its next launch.' : 'Apply on Next Launch'}</div>
            )}
            <div className="tavern-profile-actions">
              <button type="button" onClick={() => void toggleShellEdit()} disabled={!detail || busy != null || (shellEditing && editingLocal && !localSelectionReady)}>
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
              <div className="project-agent-ghost-footer">
                <div className="tavern-profile-actions">
                  <button type="button"
                    onClick={() => {
                      if (ghostExpanded) void save({ closeGhost: true });
                      else setGhostExpanded(true);
                    }}
                    disabled={ghostExpanded && busy != null}>
                    {ghostExpanded ? 'Save GHOST' : 'Edit GHOST'}
                  </button>
                </div>
                <div className="project-agent-notes-entry">
                  <button type="button" className="tavern-user-dreams"
                    disabled={!detail?.notes?.exists || openingNotes || busy === 'load'}
                    title="Open notes in your default app" onClick={() => void openNotes()}>
                    View notes
                  </button>
                  {detail?.notes && !detail.notes.exists && <span>No notes yet</span>}
                </div>
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
            {error === 'Settings changed outside this editor. Reload before saving.' && <button type="button" disabled={busy != null} onClick={() => setReload(value => value + 1)}>Reload</button>}
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
  if (provider === 'kota-local') return { label: 'Kota Local', className: 'provider-kota-local' };
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
