import type { ReactNode } from 'react';
import { extractTitle, getTitleDef, type TitleDef } from '../lib/agent-titles';

export interface ProjectAgentNameParts {
  title: TitleDef | null;
  base: string;
  bunshin: string | null;
  project: string | null;
}

export interface ProjectAgentNameFields {
  titleId: string | null;
  given: string;
  middle: string;
  surname: string;
}

export function fullProjectAgentName(name: string, projectName?: string | null): string {
  void projectName;
  const trimmed = safeText(name).trim() || 'Agent';
  return trimmed;
}

export function nameWithProjectSurname(name: string, projectName?: string | null): string {
  const trimmed = fullProjectAgentName(name);
  const project = projectSurnameLabel(projectName);
  if (!project) return trimmed;
  return composeProjectAgentName({
    ...projectAgentNameFields(trimmed),
    surname: `v. ${project}`,
  });
}

export function projectSurnameLabel(projectName?: string | null): string {
  const project = safeText(projectName).trim();
  if (!project) return '';
  const parts = project.split('/').map((part) => part.trim()).filter(Boolean);
  return parts.at(-1) ?? project;
}

export function projectAgentNameFields(name: string, projectName?: string | null): ProjectAgentNameFields {
  const parts = splitProjectAgentName(name, projectName);
  return {
    titleId: parts.title?.id ?? null,
    given: parts.base,
    middle: parts.bunshin ?? '',
    surname: parts.project ?? '',
  };
}

export function composeProjectAgentName(fields: ProjectAgentNameFields): string {
  const given = fields.given.trim();
  const middle = fields.middle.trim();
  const surname = fields.surname.trim();
  const title = getTitleDef(fields.titleId);
  const identity = given + (middle ? `-${middle}` : '');
  const body = [identity, surname].filter(Boolean).join(' ');
  return title ? `${title.abbr} ${body}` : body;
}

export function splitProjectAgentName(name: string, projectName?: string | null): ProjectAgentNameParts {
  const completed = fullProjectAgentName(name, projectName);
  const { title, rest } = extractTitle(completed);
  const { identityRaw, surname } = splitIdentityAndSurname(rest);
  const bunshinMatches = identityRaw.match(/(?:-Bunshin)+$/i)?.[0] ?? '';
  const bunshinCount = (bunshinMatches.match(/-Bunshin/gi) ?? []).length;
  const middleFromBunshin = bunshinCount > 0
    ? `Bunshin${bunshinCount > 1 ? ` ${romanSuffix(bunshinCount)}` : ''}`
    : null;
  const fallbackHyphen = middleFromBunshin ? -1 : identityRaw.lastIndexOf('-');
  const base = middleFromBunshin
    ? identityRaw.replace(/(?:-Bunshin)+$/i, '').trim()
    : fallbackHyphen > 0
      ? identityRaw.slice(0, fallbackHyphen).trim()
      : identityRaw.trim();
  const genericMiddle = fallbackHyphen > 0 ? identityRaw.slice(fallbackHyphen + 1).trim() : null;
  return {
    title,
    base: base || identityRaw.trim() || rest,
    bunshin: middleFromBunshin ?? genericMiddle,
    project: surname,
  };
}

function splitIdentityAndSurname(rest: string): { identityRaw: string; surname: string | null } {
  const vMatch = rest.match(/\s+v\.\s+/i);
  if (vMatch) {
    const identityRaw = rest.slice(0, vMatch.index).trim();
    const surname = rest.slice((vMatch.index ?? 0) + vMatch[0].length).trim();
    return { identityRaw, surname: surname ? `v. ${surname}` : null };
  }

  const hyphen = rest.lastIndexOf('-');
  if (hyphen > 0) {
    const before = rest.slice(0, hyphen).trim();
    const after = rest.slice(hyphen + 1).trim();
    const middleAndSurname = splitMiddleAndGenericSurname(after);
    if (middleAndSurname.surname) {
      return {
        identityRaw: `${before}-${middleAndSurname.middle}`,
        surname: middleAndSurname.surname,
      };
    }
  }

  const generic = splitShortCodeGenericSurname(rest);
  return generic ?? { identityRaw: rest, surname: null };
}

function splitMiddleAndGenericSurname(value: string): { middle: string; surname: string | null } {
  const words = value.split(/\s+/).filter(Boolean);
  if (words.length < 2) return { middle: value, surname: null };
  const middleWordCount =
    words[0]?.toLowerCase() === 'bunshin' && words.length > 2 && isRomanSuffix(words[1])
      ? 2
      : 1;
  return {
    middle: words.slice(0, middleWordCount).join(' '),
    surname: words.slice(middleWordCount).join(' '),
  };
}

function splitShortCodeGenericSurname(rest: string): { identityRaw: string; surname: string | null } | null {
  const words = rest.split(/\s+/).filter(Boolean);
  if (words.length < 2) return null;
  const first = words[0] ?? '';
  if (!/^(CC|Dex|Gem|Op)$/i.test(first)) return null;
  if (isRomanSuffix(words[1])) {
    if (words.length === 2) return null;
    return {
      identityRaw: words.slice(0, 2).join(' '),
      surname: words.slice(2).join(' '),
    };
  }
  return {
    identityRaw: first,
    surname: words.slice(1).join(' '),
  };
}

export function ProjectAgentName({
  name,
  projectName,
  compact = false,
  titleLine = false,
  className = '',
}: {
  name: string;
  projectName?: string | null;
  compact?: boolean;
  titleLine?: boolean;
  className?: string;
}) {
  const parts = splitProjectAgentName(name, projectName);
  const titleNode = parts.title ? (
    <NamePart className={`project-agent-name-title title-${parts.title.id}`}>
      {compact ? parts.title.abbr : parts.title.full}
    </NamePart>
  ) : null;
  const bodyNode = (
    <>
      <span className="project-agent-name-base">{parts.base}</span>
      {parts.bunshin && <NamePart className="project-agent-name-bunshin">{parts.bunshin}</NamePart>}
      {parts.project && <NamePart className="project-agent-name-project">{parts.project}</NamePart>}
    </>
  );
  if (titleLine) {
    return (
      <span className={['project-agent-name', 'title-line', compact ? 'compact' : '', className].filter(Boolean).join(' ')}>
        {titleNode}
        <span className="project-agent-name-main">{bodyNode}</span>
      </span>
    );
  }
  return (
    <span className={['project-agent-name', compact ? 'compact' : '', className].filter(Boolean).join(' ')}>
      {titleNode}
      {bodyNode}
    </span>
  );
}

function NamePart({ className, children }: { className: string; children: ReactNode }) {
  return <span className={className}>{children}</span>;
}

function romanSuffix(value: number): string {
  if (value === 2) return 'II';
  if (value === 3) return 'III';
  if (value === 4) return 'IV';
  if (value === 5) return 'V';
  return String(value);
}

function isRomanSuffix(value: string | undefined): boolean {
  return /^(II|III|IV|V|VI|VII|VIII|IX|X)$/i.test(value ?? '');
}

function safeText(value: unknown): string {
  return typeof value === 'string' ? value : String(value ?? '');
}
