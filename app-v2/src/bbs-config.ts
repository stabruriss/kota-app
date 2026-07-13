import postPromptTemplate from '../src-tauri/prompts/bbs-post-wrapper.md?raw';
import replyPromptTemplate from '../src-tauri/prompts/bbs-reply-wrapper.md?raw';
import { readSystemPrompt } from './pty-client';

export const BBS_POST_PROMPT_PATH = '$KOTA_HOME/heroes/system-bbs/bbs-post-wrapper.md';
export const BBS_REPLY_PROMPT_PATH = '$KOTA_HOME/heroes/system-bbs/bbs-reply-wrapper.md';
export const BBS_POST_PROMPT_TEMPLATE = postPromptTemplate.trimEnd();
export const BBS_REPLY_PROMPT_TEMPLATE = replyPromptTemplate.trimEnd();

export interface BbsPromptProject {
  projectId: string;
  displayName: string;
}

export interface BbsPostPromptRequest {
  currentProject: BbsPromptProject;
  knownProjects: readonly BbsPromptProject[];
  broadcast: boolean;
  targets: readonly BbsPromptProject[];
}

export interface BbsReplyPromptRequest {
  currentProject: BbsPromptProject;
  threadId: string;
  sourceProject: BbsPromptProject;
  latestAuthor: string;
}

function shellArg(value: string): string {
  if (/^[A-Za-z0-9._/-]+$/.test(value)) return value;
  return `'${value.replaceAll("'", "'\\''")}'`;
}

function projectLine(project: BbsPromptProject): string {
  return `- ${project.displayName} (id: ${project.projectId})`;
}

function projectBlock(projects: readonly BbsPromptProject[], empty: string): string {
  if (projects.length === 0) return empty;
  return projects.map(projectLine).join('\n');
}

export function renderBbsPostPrompt(
  request: BbsPostPromptRequest,
  template = BBS_POST_PROMPT_TEMPLATE,
): string {
  const command = request.broadcast
    ? [
      "kota-bbs new --broadcast <<'EOF'",
      '<Markdown post>',
      'EOF',
    ].join('\n')
    : [
      `kota-bbs new --projects ${request.targets.map((project) => shellArg(project.projectId)).join(' ')} <<'EOF'`,
      '<Markdown post>',
      'EOF',
    ].join('\n');

  return template
    .replaceAll('{{current_project}}', projectLine(request.currentProject))
    .replaceAll('{{known_projects}}', projectBlock(request.knownProjects, '- No other open projects.'))
    .replaceAll('{{audience}}', request.broadcast ? '- Broadcast' : projectBlock(request.targets, '- No target selected.'))
    .replaceAll('{{command}}', command);
}

export function renderBbsReplyPrompt(
  request: BbsReplyPromptRequest,
  template = BBS_REPLY_PROMPT_TEMPLATE,
): string {
  return template
    .replaceAll('{{thread_id}}', request.threadId)
    .replaceAll('{{current_project}}', projectLine(request.currentProject))
    .replaceAll('{{source_project}}', projectLine(request.sourceProject))
    .replaceAll('{{latest_author}}', request.latestAuthor);
}

export async function loadBbsPostPromptTemplate(): Promise<string> {
  const result = await readSystemPrompt(
    { path: BBS_POST_PROMPT_PATH },
    BBS_POST_PROMPT_TEMPLATE,
  );
  return result.content;
}

export async function loadBbsReplyPromptTemplate(): Promise<string> {
  const result = await readSystemPrompt(
    { path: BBS_REPLY_PROMPT_PATH },
    BBS_REPLY_PROMPT_TEMPLATE,
  );
  return result.content;
}

export async function renderBbsPostPromptFromFile(request: BbsPostPromptRequest): Promise<string> {
  return renderBbsPostPrompt(request, await loadBbsPostPromptTemplate());
}

export async function renderBbsReplyPromptFromFile(request: BbsReplyPromptRequest): Promise<string> {
  return renderBbsReplyPrompt(request, await loadBbsReplyPromptTemplate());
}

export function renderBbsPostPromptPreview(template = BBS_POST_PROMPT_TEMPLATE): string {
  return renderBbsPostPrompt(
    {
      currentProject: { projectId: 'account-current-project', displayName: 'Current Project' },
      knownProjects: [
        { projectId: 'account-alpha', displayName: 'Alpha' },
        { projectId: 'account-beta', displayName: 'Beta' },
      ],
      broadcast: false,
      targets: [{ projectId: 'account-beta', displayName: 'Beta' }],
    },
    template,
  );
}

export function renderBbsReplyPromptPreview(template = BBS_REPLY_PROMPT_TEMPLATE): string {
  return renderBbsReplyPrompt(
    {
      currentProject: { projectId: 'account-current-project', displayName: 'Current Project' },
      threadId: 'thread-example',
      sourceProject: { projectId: 'account-alpha', displayName: 'Alpha' },
      latestAuthor: 'Agent Name',
    },
    template,
  );
}
