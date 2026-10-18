export type ToolName = 'kota_agent_bus' | 'kota_bbs' | 'kota_ember' | 'kota_bartender' | 'kota_my_soul';
export interface CommandInvocation { command: string; args: string[]; stdin: string }
export class InvalidToolArguments extends Error {}

type Fields = Record<string, unknown>;
function invalid(): never { throw new InvalidToolArguments(); }
const text = (value: unknown): string => typeof value === 'string' && value.length > 0 ? value : invalid();
// shell:false does not prevent the CLI from parsing a value as an option. Bodies use text(), not this guard.
const argument = (value: unknown): string => {
  const result = text(value);
  return result.startsWith('-') ? invalid() : result;
};
const texts = (value: unknown, minimum = 1, maximum = Number.MAX_SAFE_INTEGER): string[] => {
  if (!Array.isArray(value) || value.length < minimum || value.length > maximum) invalid();
  return value.map(argument);
};
function fields(value: unknown, required: string[], optional: string[] = []): Fields {
  if (!value || typeof value !== 'object' || Array.isArray(value)) invalid();
  const result = value as Fields;
  if (!required.every(key => Object.hasOwn(result, key)) || Object.keys(result).some(key => !required.includes(key) && !optional.includes(key))) invalid();
  return result;
}
function timing(value: unknown): string[] {
  const input = fields(value, ['kind'], ['value']);
  if (input.kind === 'idle') {
    if (Object.hasOwn(input, 'value')) invalid();
    return ['--idle'];
  }
  if (!['in', 'every', 'at', 'cron'].includes(input.kind as string)) invalid();
  return [`--${input.kind}`, argument(input.value)];
}
function endOptions(input: Fields): string[] {
  const args: string[] = [];
  if (Object.hasOwn(input, 'endAfter')) {
    if (!Number.isSafeInteger(input.endAfter) || (input.endAfter as number) < 1) invalid();
    args.push('--end-after', String(input.endAfter));
  }
  if (Object.hasOwn(input, 'endAt')) args.push('--end-at', argument(input.endAt));
  return args;
}
function postOptions(input: Fields): string[] {
  const args: string[] = [];
  if (Object.hasOwn(input, 'at')) args.push('--at', argument(input.at));
  if (Object.hasOwn(input, 'attachments')) for (const path of texts(input.attachments, 0, 9)) args.push('--attach', path);
  return args;
}

/** Builds only the fixed CLI actions in CONTRACT-INJECT.md; no shell or caller-selected context. */
export function buildInvocation(name: ToolName, value: unknown): CommandInvocation {
  if (name === 'kota_agent_bus') {
    const input = fields(value, ['to', 'body'], ['intent']);
    const args = ['send', '--to', argument(input.to)];
    if (Object.hasOwn(input, 'intent')) args.push('--intent', argument(input.intent));
    return { command: 'kota-agent-bus', args, stdin: text(input.body) };
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) invalid();
  const action = (value as Fields).action;
  if (name === 'kota_bbs') {
    if (action === 'agents') {
      fields(value, ['action']);
      return { command: 'kota-bbs', args: ['agents'], stdin: '' };
    }
    if (action === 'show') {
      const input = fields(value, ['action', 'threadId']);
      return { command: 'kota-bbs', args: ['show', argument(input.threadId)], stdin: '' };
    }
    if (action !== 'new' && action !== 'reply') invalid();
    const input = action === 'new'
      ? fields(value, ['action', 'body'], ['projects', 'broadcast', 'at', 'attachments'])
      : fields(value, ['action', 'threadId', 'body'], ['at', 'attachments']);
    if (typeof input.body !== 'string') invalid();
    const attachments = Object.hasOwn(input, 'attachments') ? texts(input.attachments, 0, 9) : [];
    if (input.body.length === 0 && attachments.length === 0) invalid();
    let args: string[];
    if (action === 'reply') args = ['reply', argument(input.threadId)];
    else {
      if (Object.hasOwn(input, 'broadcast') && typeof input.broadcast !== 'boolean') invalid();
      const projects = Object.hasOwn(input, 'projects') ? texts(input.projects) : [];
      if ((projects.length > 0) === (input.broadcast === true)) invalid();
      args = input.broadcast === true ? ['new', '--broadcast'] : ['new', '--projects', ...projects];
    }
    return { command: 'kota-bbs', args: [...args, ...postOptions(input)], stdin: input.body };
  }
  if (name === 'kota_ember') {
    if (action === 'list') {
      const input = fields(value, ['action'], ['draft']);
      if (Object.hasOwn(input, 'draft') && typeof input.draft !== 'boolean') invalid();
      return { command: 'kota-ember', args: ['list', '--json', ...(input.draft ? ['--draft'] : [])], stdin: '' };
    }
    if (action === 'show' || action === 'delete') {
      const input = fields(value, ['action', 'id']);
      return { command: 'kota-ember', args: [action, argument(input.id)], stdin: '' };
    }
    if (action === 'add' && (value as Fields).draft === true) {
      const input = fields(value, ['action', 'draft', 'body']);
      return { command: 'kota-ember', args: ['add', '--draft'], stdin: text(input.body) };
    }
    if (action === 'add') {
      const input = fields(value, ['action', 'to', 'timing', 'body'], ['endAfter', 'endAt']);
      return { command: 'kota-ember', args: ['add', '--to', texts(input.to).join(','), ...timing(input.timing), ...endOptions(input)], stdin: text(input.body) };
    }
    if (action === 'update') {
      const input = fields(value, ['action', 'id'], ['body', 'to', 'timing', 'endAfter', 'endAt']);
      const args = ['update', argument(input.id)];
      if (Object.hasOwn(input, 'to')) args.push('--to', texts(input.to).join(','));
      if (Object.hasOwn(input, 'timing')) args.push(...timing(input.timing));
      args.push(...endOptions(input));
      return { command: 'kota-ember', args, stdin: Object.hasOwn(input, 'body') ? text(input.body) : '' };
    }
    invalid();
  }
  if (name === 'kota_my_soul') {
    // Texts go after `--` so a mark or query that starts with `-` is never read as an option.
    if (action === 'add-mark') {
      const input = fields(value, ['action', 'text']);
      return { command: 'kota-my-soul', args: ['add-mark', '--', text(input.text)], stdin: '' };
    }
    if (action === 'edit-mark') {
      const input = fields(value, ['action', 'id', 'text']);
      return { command: 'kota-my-soul', args: ['edit-mark', argument(input.id), '--', text(input.text)], stdin: '' };
    }
    if (action === 'delete-mark' || action === 'clear-heat') {
      const input = fields(value, ['action', 'id']);
      return { command: 'kota-my-soul', args: [action, argument(input.id)], stdin: '' };
    }
    if (action === 'read-mark') {
      fields(value, ['action']);
      return { command: 'kota-my-soul', args: ['read-mark'], stdin: '' };
    }
    if (action === 'search-episode') {
      if (Object.hasOwn(value, 'id')) {
        const input = fields(value, ['action', 'id']);
        return { command: 'kota-my-soul', args: ['search-episode', '--id', argument(input.id)], stdin: '' };
      }
      const input = fields(value, ['action', 'query'], ['project']);
      const args = ['search-episode'];
      if (Object.hasOwn(input, 'project')) args.push('--project', argument(input.project));
      return { command: 'kota-my-soul', args: [...args, '--', text(input.query)], stdin: '' };
    }
    invalid();
  }
  if (name === 'kota_bartender') {
    fields(value, ['action']);
    if (action !== 'sync') invalid();
    return { command: 'kota-bartender', args: ['sync', '--json'], stdin: '' };
  }
  return invalid();
}
