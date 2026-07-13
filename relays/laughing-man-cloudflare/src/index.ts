/// <reference types="@cloudflare/workers-types" />

import { DurableObject } from 'cloudflare:workers';

const RELAY_VERSION = '0.1.2';
const PROTOCOL_VERSION = 'kota-lm-standby.v1';
const ONLINE_GRACE_MS = 90_000;
const MAX_PULL_LIMIT = 50;
const QUEUE_PREFIX = 'queue:';

interface Env {
  RELAY_STATE: DurableObjectNamespace<RelayState>;
}

interface TelegramUpdate {
  update_id?: number;
  message?: {
    text?: string;
    caption?: string;
    chat?: { id?: number; type?: string };
    from?: { id?: number };
    photo?: unknown;
    document?: unknown;
  };
  callback_query?: {
    id?: string;
    data?: string;
    from?: { id?: number };
    message?: { message_id?: number; chat?: { id?: number } };
  };
}

interface DesktopState {
  ownerUserId?: number | null;
  selected?: {
    projectId: string;
    projectName: string;
    agentId: string;
    agentName: string;
    muted?: boolean;
  } | null;
  catalog?: CatalogProject[];
  catalogUpdatedAt?: string | null;
}

interface CatalogProject {
  projectId: string;
  projectRoot?: string;
  projectName: string;
  agents: [string, string][];
}

type TargetSelection = NonNullable<DesktopState['selected']>;

interface QueueRow {
  id: string;
  updateId?: number | null;
  receivedAt: string;
  update: TelegramUpdate;
  preview: string;
  target: DesktopState['selected'] | null;
  offline: boolean;
  status: 'queued' | 'sent' | 'discarded';
  sentAt?: string | null;
}

function json(value: unknown, init: ResponseInit = {}): Response {
  return new Response(JSON.stringify(value), {
    ...init,
    headers: {
      'content-type': 'application/json; charset=utf-8',
      ...(init.headers ?? {}),
    },
  });
}

function html(value: string, init: ResponseInit = {}): Response {
  return new Response(value, {
    ...init,
    headers: {
      'content-type': 'text/html; charset=utf-8',
      ...(init.headers ?? {}),
    },
  });
}

function nowIso(): string {
  return new Date().toISOString().replace(/\.\d{3}Z$/, 'Z');
}

function queueKey(id: string): string {
  return `${QUEUE_PREFIX}${id}`;
}

function updatePreview(update: TelegramUpdate): string {
  const text = update.message?.text ?? update.message?.caption ?? update.callback_query?.data ?? '';
  return text.split(/\s+/).join(' ').slice(0, 160);
}

function updateQueueId(update: TelegramUpdate): string {
  if (typeof update.update_id === 'number') return `tg-${update.update_id}`;
  return `tg-${crypto.randomUUID()}`;
}

function updateSenderId(update: TelegramUpdate): number | null {
  return update.message?.from?.id ?? update.callback_query?.from?.id ?? null;
}

function isReplyKeyboardControl(update: TelegramUpdate): boolean {
  const text = (update.message?.text ?? update.message?.caption ?? '').trim();
  return text === '/start' || text === 'Switch' || text.startsWith('🔌');
}

function parseCallbackData(data: string): { version: string; action: string; index: string } | null {
  const parts = data.split(':');
  if (parts.length !== 4 || parts[0] !== 'lm') return null;
  return { version: parts[1] || '', action: parts[2] || '', index: parts[3] || '' };
}

function replyKeyboard(selected: TargetSelection | null): object {
  const text = selected ? `🔌 ${selected.projectName} / ${selected.agentName}` : '🔌 Not connected';
  return {
    keyboard: [[{ text }]],
    resize_keyboard: true,
    is_persistent: true,
  };
}

function inlineButton(text: string, callbackData: string): object {
  return { text, callback_data: callbackData };
}

function normalizeTarget(value: unknown): TargetSelection | null {
  if (!value || typeof value !== 'object') return null;
  const source = value as Record<string, unknown>;
  const projectId = typeof source.projectId === 'string' ? source.projectId : '';
  const projectName = typeof source.projectName === 'string' ? source.projectName : '';
  const agentId = typeof source.agentId === 'string' ? source.agentId : '';
  const agentName = typeof source.agentName === 'string' ? source.agentName : '';
  if (!projectId || !projectName || !agentId || !agentName) return null;
  return {
    projectId,
    projectName,
    agentId,
    agentName,
    muted: source.muted === true,
  };
}

function normalizeCatalog(value: unknown): CatalogProject[] {
  if (!Array.isArray(value)) return [];
  const out: CatalogProject[] = [];
  for (const item of value) {
    if (!item || typeof item !== 'object') continue;
    const source = item as Record<string, unknown>;
    const projectId = typeof source.projectId === 'string' ? source.projectId : '';
    const projectName = typeof source.projectName === 'string' ? source.projectName : '';
    if (!projectId || !projectName) continue;
    const agents: [string, string][] = [];
    if (Array.isArray(source.agents)) {
      for (const agent of source.agents) {
        if (!Array.isArray(agent) || agent.length < 2) continue;
        const id = typeof agent[0] === 'string' ? agent[0] : '';
        const name = typeof agent[1] === 'string' ? agent[1] : '';
        if (id && name) agents.push([id, name]);
      }
    }
    out.push({
      projectId,
      projectRoot: typeof source.projectRoot === 'string' ? source.projectRoot : undefined,
      projectName,
      agents,
    });
  }
  return out;
}

function normalizeDesktopState(value: DesktopState | undefined): DesktopState {
  return {
    ownerUserId: typeof value?.ownerUserId === 'number' ? value.ownerUserId : null,
    selected: normalizeTarget(value?.selected),
    catalog: normalizeCatalog(value?.catalog),
    catalogUpdatedAt: typeof value?.catalogUpdatedAt === 'string' ? value.catalogUpdatedAt : null,
  };
}

function stableCatalogFingerprint(catalog: CatalogProject[]): string {
  return JSON.stringify(catalog.map((project) => ({
    id: project.projectId,
    root: project.projectRoot ?? '',
    name: project.projectName,
    agents: project.agents,
  })));
}

function hashVersion(input: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < input.length; index += 1) {
    hash ^= input.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash.toString(16).padStart(8, '0');
}

function catalogVersion(catalog: CatalogProject[]): string {
  return hashVersion(stableCatalogFingerprint(catalog));
}

function reconcileTarget(catalog: CatalogProject[], target: TargetSelection | null): TargetSelection | null {
  if (!target) return null;
  const project = catalog.find((candidate) => candidate.projectId === target.projectId);
  if (!project) return target;
  const agent = project.agents.find(([agentId]) => agentId === target.agentId);
  if (!agent) return { ...target, projectName: project.projectName };
  return {
    ...target,
    projectName: project.projectName,
    agentName: agent[1],
  };
}

function projectPanel(catalog: CatalogProject[], version: string): { text: string; reply_markup: object } {
  return {
    text: 'Kota Laughing Man\nChoose a project to connect.',
    reply_markup: {
      inline_keyboard: catalog.map((project, index) => [
        inlineButton(project.projectName, `lm:${version}:p:${index}`),
      ]),
    },
  };
}

function agentPanel(catalog: CatalogProject[], version: string, projectIndex: number): { text: string; reply_markup: object } {
  const project = catalog[projectIndex];
  if (!project) return projectPanel(catalog, version);
  const rows: object[][] = [];
  for (let index = 0; index < project.agents.length; index += 2) {
    const row: object[] = [];
    for (const [offset, agent] of project.agents.slice(index, index + 2).entries()) {
      row.push(inlineButton(agent[1], `lm:${version}:a:${projectIndex}.${index + offset}`));
    }
    rows.push(row);
  }
  rows.push([inlineButton('‹ Projects', `lm:${version}:back:0`)]);
  return {
    text: `Kota Laughing Man\nProject: ${project.projectName}\nChoose an agent.`,
    reply_markup: { inline_keyboard: rows },
  };
}

function connectedPanel(version: string, selected: TargetSelection): { text: string; reply_markup: object } {
  return {
    text: `Connected\nProject: ${selected.projectName}\nSending to: ${selected.agentName}\nSend any message to prompt this agent.`,
    reply_markup: {
      inline_keyboard: [[
        inlineButton('Switch agent', `lm:${version}:switch_agent:0`),
        inlineButton('Switch project', `lm:${version}:back:0`),
      ]],
    },
  };
}

function compareQueueRows(left: QueueRow, right: QueueRow): number {
  if (typeof left.updateId === 'number' && typeof right.updateId === 'number' && left.updateId !== right.updateId) {
    return left.updateId - right.updateId;
  }
  return left.receivedAt.localeCompare(right.receivedAt);
}

function setupPage(request: Request, paired: boolean): Response {
  const url = new URL(request.url);
  const escaped = `${url.origin}`.replace(/[&<>"']/g, (c) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
  })[c] ?? c);
  return html(`<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Kota 24/7 Standby Worker</title>
  <style>
    body{margin:0;min-height:100vh;display:grid;place-items:center;background:#111214;color:#eee9df;font:14px ui-sans-serif,system-ui;padding:28px}
    main{width:min(560px,100%);border:1px solid rgba(222,216,204,.16);border-radius:8px;background:#181a1d;box-shadow:0 28px 70px rgba(0,0,0,.44);overflow:hidden}
    header{padding:18px 20px;border-bottom:1px solid rgba(222,216,204,.16);background:linear-gradient(180deg,rgba(118,168,207,.10),transparent)}
    h1{margin:0;font:700 22px Georgia,serif}.sub{color:#8f897f;font:11px ui-monospace,Menlo,monospace;margin-top:4px}.body{padding:18px 20px 20px;display:grid;gap:14px}
    .ready{width:max-content;border:1px solid rgba(148,202,163,.38);color:#cde8d2;background:rgba(148,202,163,.08);border-radius:999px;padding:6px 10px;font:800 11px ui-monospace,Menlo,monospace}
    .note{color:#c9c2b6;line-height:1.55}.box{border:1px solid rgba(215,184,110,.34);border-radius:7px;background:rgba(0,0,0,.2);padding:12px;display:grid;gap:8px}
    label{color:#8f897f;font:850 10px ui-monospace,Menlo,monospace;text-transform:uppercase}.url{color:#dce8f1;font:12px/1.45 ui-monospace,Menlo,monospace;overflow-wrap:anywhere}
    button{min-height:32px;width:max-content;border:1px solid rgba(118,168,207,.45);border-radius:6px;background:rgba(118,168,207,.14);color:#e6f3fb;padding:0 12px;font:800 10.5px ui-monospace,Menlo,monospace;cursor:pointer}
  </style>
</head>
<body>
  <main>
    <header><h1>Kota 24/7 Standby Worker</h1><div class="sub">Cloudflare Worker · ${paired ? 'paired' : 'unpaired'}</div></header>
    <section class="body">
      <span class="ready">${paired ? 'Connected to Kota' : 'Ready to connect'}</span>
      <p class="note">${paired ? 'This Worker is already paired. Manage it from Kota Laughing Man settings.' : 'Return to Kota and paste this Worker URL into Laughing Man settings.'}</p>
      <div class="box"><label>Worker URL</label><div class="url" id="url">${escaped}</div><button id="copy" type="button">Copy URL</button></div>
    </section>
  </main>
  <script>document.getElementById('copy').onclick=async()=>{await navigator.clipboard?.writeText(document.getElementById('url').textContent.trim());document.getElementById('copy').textContent='Copied'}</script>
</body>
</html>`);
}

export class RelayState extends DurableObject<Env> {
  private async getConfig(key: string): Promise<string | null> {
    return (await this.ctx.storage.get<string>(`config:${key}`)) ?? null;
  }

  private async setConfig(key: string, value: string): Promise<void> {
    await this.ctx.storage.put(`config:${key}`, value);
  }

  private async setConfigIfChanged(key: string, value: string): Promise<void> {
    if ((await this.getConfig(key)) === value) return;
    await this.setConfig(key, value);
  }

  private async getJsonConfig<T>(key: string): Promise<T | null> {
    const raw = await this.getConfig(key);
    if (!raw) return null;
    try {
      return JSON.parse(raw) as T;
    } catch {
      return null;
    }
  }

  private async setJsonConfig(key: string, value: unknown): Promise<void> {
    await this.setConfig(key, JSON.stringify(value));
  }

  private async setJsonConfigIfChanged(key: string, value: unknown): Promise<void> {
    await this.setConfigIfChanged(key, JSON.stringify(value));
  }

  private async isPaired(): Promise<boolean> {
    return Boolean(await this.getConfig('desktopSecret'));
  }

  private async desktopOnline(): Promise<boolean> {
    const heartbeat = await this.getConfig('lastHeartbeatAt');
    if (!heartbeat) return false;
    const at = Date.parse(heartbeat);
    return Number.isFinite(at) && Date.now() - at < ONLINE_GRACE_MS;
  }

  private async requireDesktopAuth(request: Request): Promise<Response | null> {
    const expected = await this.getConfig('desktopSecret');
    if (!expected || request.headers.get('x-kota-standby-secret') !== expected) {
      return json({ ok: false, error: 'unauthorized' }, { status: 401 });
    }
    return null;
  }

  private async queuedRows(): Promise<QueueRow[]> {
    const rows = await this.ctx.storage.list<QueueRow>({ prefix: QUEUE_PREFIX });
    return [...rows.values()];
  }

  private async queueCount(): Promise<number> {
    const rows = await this.queuedRows();
    return rows.filter((row) => row.status === 'queued' && row.offline).length;
  }

  private async desktopState(): Promise<DesktopState> {
    return normalizeDesktopState(await this.getJsonConfig<DesktopState>('desktopState') ?? undefined);
  }

  private async selectedTarget(): Promise<TargetSelection | null> {
    return normalizeTarget(await this.getJsonConfig<TargetSelection>('selectedTarget'));
  }

  private async setSelectedTarget(target: TargetSelection | null): Promise<void> {
    if (target) {
      await this.setJsonConfigIfChanged('selectedTarget', target);
    } else {
      await this.ctx.storage.delete('config:selectedTarget');
    }
  }

  private async updateDesktopState(state: DesktopState | undefined, initializeTarget: boolean): Promise<DesktopState> {
    const normalized = normalizeDesktopState(state);
    const catalog = normalizeCatalog(normalized.catalog);
    const currentTarget = await this.selectedTarget();
    const reconciledTarget = reconcileTarget(catalog, currentTarget);
    await this.setJsonConfigIfChanged('desktopState', normalized);
    if (currentTarget && JSON.stringify(reconciledTarget) !== JSON.stringify(currentTarget)) {
      await this.setSelectedTarget(reconciledTarget);
    } else if (!currentTarget && initializeTarget && normalized.selected) {
      await this.setSelectedTarget(reconcileTarget(catalog, normalized.selected));
    }
    return normalized;
  }

  private async enqueueUpdate(update: TelegramUpdate, offline: boolean): Promise<string> {
    const id = updateQueueId(update);
    if (await this.ctx.storage.get(queueKey(id))) return id;
    const target = await this.selectedTarget();
    const row: QueueRow = {
      id,
      updateId: update.update_id ?? null,
      receivedAt: nowIso(),
      update,
      preview: updatePreview(update),
      target,
      offline,
      status: 'queued',
      sentAt: null,
    };
    await this.ctx.storage.put(queueKey(id), row);
    return id;
  }

  private async handleHealth(request: Request): Promise<Response> {
    return json({
      ok: true,
      relayVersion: RELAY_VERSION,
      protocolVersion: PROTOCOL_VERSION,
      paired: await this.isPaired(),
      online: await this.desktopOnline(),
      queueCount: await this.queueCount(),
      url: new URL(request.url).origin,
    });
  }

  private async handlePair(request: Request): Promise<Response> {
    if (await this.isPaired()) {
      return json({ ok: false, error: 'already paired' }, { status: 409 });
    }
    const body = await request.json<{
      deviceId?: string;
      desktopSecret?: string;
      webhookSecret?: string;
      ownerUserId?: number;
      state?: DesktopState;
    }>();
    if (!body.deviceId || !body.desktopSecret || !body.webhookSecret || !body.ownerUserId) {
      return json({ ok: false, error: 'missing pairing fields' }, { status: 400 });
    }
    const state = await this.updateDesktopState(body.state, true);
    await Promise.all([
      this.setConfig('deviceId', body.deviceId),
      this.setConfig('desktopSecret', body.desktopSecret),
      this.setConfig('webhookSecret', body.webhookSecret),
      this.setConfig('ownerUserId', String(body.ownerUserId)),
      this.setConfig('pairedAt', nowIso()),
      this.setConfig('lastHeartbeatAt', nowIso()),
    ]);
    return json({
      ok: true,
      relayVersion: RELAY_VERSION,
      protocolVersion: PROTOCOL_VERSION,
      selectedTarget: await this.selectedTarget(),
      snapshotVersion: catalogVersion(normalizeCatalog(state.catalog)),
    });
  }

  private async handleDesktopState(request: Request): Promise<Response> {
    const unauthorized = await this.requireDesktopAuth(request);
    if (unauthorized) return unauthorized;
    const body = await request.json<{ state?: DesktopState }>();
    const state = await this.updateDesktopState(body.state, false);
    return json({
      ok: true,
      queueCount: await this.queueCount(),
      selectedTarget: await this.selectedTarget(),
      snapshotVersion: catalogVersion(normalizeCatalog(state.catalog)),
    });
  }

  private async handleHeartbeat(request: Request): Promise<Response> {
    const unauthorized = await this.requireDesktopAuth(request);
    if (unauthorized) return unauthorized;
    const body = await request.json<{ state?: DesktopState }>().catch((): { state?: DesktopState } => ({}));
    const state = body.state
      ? await this.updateDesktopState(body.state, true)
      : await this.updateDesktopState(await this.desktopState(), false);
    await this.setConfig('lastHeartbeatAt', nowIso());
    return json({
      ok: true,
      relayVersion: RELAY_VERSION,
      protocolVersion: PROTOCOL_VERSION,
      queueCount: await this.queueCount(),
      selectedTarget: await this.selectedTarget(),
      snapshotVersion: catalogVersion(normalizeCatalog(state.catalog)),
    });
  }

  private async handlePull(request: Request): Promise<Response> {
    const unauthorized = await this.requireDesktopAuth(request);
    if (unauthorized) return unauthorized;
    const body = await request.json<{ limit?: number }>().catch((): { limit?: number } => ({}));
    const limit = Math.max(1, Math.min(MAX_PULL_LIMIT, body.limit ?? 20));
    const rows = (await this.queuedRows())
      .filter((row) => row.status === 'queued')
      .sort(compareQueueRows)
      .slice(0, limit);
    return json({
      ok: true,
      items: rows.map((row) => ({
        id: row.id,
        receivedAt: row.receivedAt,
        update: row.update,
        preview: row.preview,
        target: row.target,
        offline: row.offline,
        status: row.status,
        sentAt: row.sentAt,
      })),
      queueCount: await this.queueCount(),
    });
  }

  private async handleAck(request: Request): Promise<Response> {
    const unauthorized = await this.requireDesktopAuth(request);
    if (unauthorized) return unauthorized;
    const body = await request.json<{ ids?: string[]; status?: 'sent' | 'discarded' }>();
    const ids = Array.isArray(body.ids) ? body.ids.filter((id) => typeof id === 'string') : [];
    if (ids.length === 0) return json({ ok: true, queueCount: await this.queueCount() });
    await this.ctx.storage.delete(ids.map(queueKey));
    return json({ ok: true, queueCount: await this.queueCount() });
  }

  private async menuState(): Promise<{ catalog: CatalogProject[]; version: string; selected: TargetSelection | null }> {
    const state = await this.desktopState();
    const catalog = normalizeCatalog(state.catalog);
    return {
      catalog,
      version: catalogVersion(catalog),
      selected: reconcileTarget(catalog, await this.selectedTarget()),
    };
  }

  private async handleControlMessage(update: TelegramUpdate): Promise<Response | null> {
    if (!update.message || !isReplyKeyboardControl(update)) return null;
    const chatId = update.message.chat?.id;
    if (!chatId) return json({ ok: true });
    const { catalog, version, selected } = await this.menuState();
    const panel = selected ? connectedPanel(version, selected) : projectPanel(catalog, version);
    return json({
      method: 'sendMessage',
      chat_id: chatId,
      text: panel.text,
      reply_markup: panel.reply_markup,
      disable_notification: true,
    });
  }

  private async handleCallback(update: TelegramUpdate): Promise<Response | null> {
    const callback = update.callback_query;
    if (!callback) return null;
    const parsed = parseCallbackData(callback.data ?? '');
    const chatId = callback.message?.chat?.id;
    if (!parsed || !chatId) return json({ ok: true });
    const { catalog, version, selected } = await this.menuState();
    const stale = parsed.version !== version;
    const edit = (panel: { text: string; reply_markup: object }) => json({
      method: 'editMessageText',
      chat_id: chatId,
      message_id: callback.message?.message_id,
      text: panel.text,
      reply_markup: panel.reply_markup,
    });
    if (stale) {
      return edit(projectPanel(catalog, version));
    }
    if (parsed.action === 'p') {
      return edit(agentPanel(catalog, version, Number.parseInt(parsed.index, 10)));
    }
    if (parsed.action === 'back') {
      return edit(projectPanel(catalog, version));
    }
    if (parsed.action === 'switch_agent') {
      const projectIndex = selected
        ? catalog.findIndex((project) => project.projectId === selected.projectId)
        : -1;
      return projectIndex >= 0 ? edit(agentPanel(catalog, version, projectIndex)) : edit(projectPanel(catalog, version));
    }
    if (parsed.action === 'a') {
      const [projectRaw, agentRaw] = parsed.index.split('.');
      const projectIndex = Number.parseInt(projectRaw ?? '', 10);
      const agentIndex = Number.parseInt(agentRaw ?? '', 10);
      const project = Number.isFinite(projectIndex) ? catalog[projectIndex] : undefined;
      const agent = project && Number.isFinite(agentIndex) ? project.agents[agentIndex] : undefined;
      if (!project || !agent) {
        return edit(projectPanel(catalog, version));
      }
      const next: TargetSelection = {
        projectId: project.projectId,
        projectName: project.projectName,
        agentId: agent[0],
        agentName: agent[1],
        muted: false,
      };
      await this.setSelectedTarget(next);
      return json({
        method: 'sendMessage',
        chat_id: chatId,
        text: `🟢 ${next.projectName} — sending to ${next.agentName}.`,
        reply_markup: replyKeyboard(next),
        disable_notification: true,
      });
    }
    return json({
      method: 'sendMessage',
      chat_id: chatId,
      text: 'Menu updated. Choose again.',
      reply_markup: projectPanel(catalog, version).reply_markup,
      disable_notification: true,
    });
  }

  private async handleDisconnect(request: Request): Promise<Response> {
    const unauthorized = await this.requireDesktopAuth(request);
    if (unauthorized) return unauthorized;
    const configKeys = [
      'deviceId',
      'desktopSecret',
      'webhookSecret',
      'ownerUserId',
      'pairedAt',
      'desktopState',
      'selectedTarget',
      'lastHeartbeatAt',
    ].map((key) => `config:${key}`);
    await this.ctx.storage.delete(configKeys);
    await this.ctx.storage.delete((await this.queuedRows()).map((row) => queueKey(row.id)));
    return json({ ok: true });
  }

  private async handleTelegram(request: Request): Promise<Response> {
    const expectedSecret = await this.getConfig('webhookSecret');
    if (!expectedSecret || request.headers.get('x-telegram-bot-api-secret-token') !== expectedSecret) {
      return json({ ok: false, error: 'unauthorized' }, { status: 401 });
    }
    const update = await request.json<TelegramUpdate>();
    const ownerUserId = await this.getConfig('ownerUserId');
    const senderId = updateSenderId(update);
    if (!ownerUserId || !senderId || String(senderId) !== ownerUserId) {
      return json({ ok: true });
    }
    const control = await this.handleControlMessage(update);
    if (control) return control;
    const callback = await this.handleCallback(update);
    if (callback) return callback;
    const online = await this.desktopOnline();
    if (online) {
      await this.enqueueUpdate(update, false);
      return json({ ok: true });
    }
    if (update.message) {
      await this.enqueueUpdate(update, true);
      const chatId = update.message.chat?.id;
      if (chatId) {
        return json({
          method: 'sendMessage',
          chat_id: chatId,
          text: 'Kota desktop is offline. I saved this for the next launch.',
          disable_notification: true,
        });
      }
    }
    if (update.callback_query?.id) {
      return json({
        method: 'answerCallbackQuery',
        callback_query_id: update.callback_query.id,
        text: 'Kota desktop is offline.',
        show_alert: false,
      });
    }
    return json({ ok: true });
  }

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (request.method === 'GET' && url.pathname === '/') {
      return setupPage(request, await this.isPaired());
    }
    if (request.method === 'GET' && url.pathname === '/health') return this.handleHealth(request);
    if (request.method === 'POST' && url.pathname === '/pair') return this.handlePair(request);
    if (request.method === 'POST' && url.pathname === '/desktop/heartbeat') return this.handleHeartbeat(request);
    if (request.method === 'POST' && url.pathname === '/desktop/state') return this.handleDesktopState(request);
    if (request.method === 'POST' && url.pathname === '/desktop/pull') return this.handlePull(request);
    if (request.method === 'POST' && url.pathname === '/desktop/ack') return this.handleAck(request);
    if (request.method === 'POST' && url.pathname === '/desktop/disconnect') return this.handleDisconnect(request);
    if (request.method === 'POST' && url.pathname === '/telegram') return this.handleTelegram(request);
    return json({ ok: false, error: 'not found' }, { status: 404 });
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const id = env.RELAY_STATE.idFromName('kota-laughing-man-standby');
    return env.RELAY_STATE.get(id).fetch(request);
  },
};
