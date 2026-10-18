import { invoke } from '@tauri-apps/api/core';
import { hasTauriRuntime, type ProjectAgentDetail, type ProjectAgentNameFieldsPayload, type TavernHeroSourceState, type TavernIncarnateHeroResult } from '../pty-client';

export interface RecruitPreview {
  state: TavernHeroSourceState;
  heroName: string;
  sourceProjectName?: string;
  sourceAgentName?: string;
  marks: number;
  episodes: number;
  reason?: string;
}

export interface RecruitPreviewRequest {
  heroId?: string;
  sourceProjectRoot?: string;
  sourceAgentId?: string;
}

export interface IncarnationRecruitSource {
  projectRoot: string;
  agent: ProjectAgentDetail;
}

export async function readRecruitPreview(request: RecruitPreviewRequest): Promise<RecruitPreview> {
  // The browser-only demo has native factory heroes and no cross-project store.
  if (!hasTauriRuntime() && request.heroId) return { state: 'native', heroName: request.heroId, marks: 0, episodes: 0 };
  return invoke<RecruitPreview>('tavern_recruit_preview', { request });
}

export function recruitFromIncarnation(request: {
  sourceProjectRoot: string;
  sourceAgentId: string;
  agentId: string;
  displayName: string;
  nameFields?: ProjectAgentNameFieldsPayload | null;
  projectRoot?: string | null;
  progressId?: string | null;
}): Promise<TavernIncarnateHeroResult> {
  return invoke<TavernIncarnateHeroResult>('project_agent_recruit_from_incarnation', { request });
}
