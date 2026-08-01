import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type ClipboardEvent as ReactClipboardEvent,
  type DragEvent as ReactDragEvent,
  type FormEvent as ReactFormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from 'react';
import type { DragDropEvent as TauriDragDropPayload } from '@tauri-apps/api/webview';
import { AGENTS } from '../mock/fixtures';
import type { Agent, AgentId } from '../types/scene';
import { ProjectAgentName, splitProjectAgentName } from './ProjectAgentName';
import { avatarClassForAgentFallback, avatarImageStyleForId } from '../lib/hero-avatars';
import { AGENT_SLOT_KEY_RANGE_LABEL, MAX_AGENT_SLOTS } from '../lib/agent-slots';
import {
  ROOM_QUOTE_MAX,
  serializeRoomQuotePrompt,
  type RoomQuoteInsertResult,
  type RoomQuoteReference,
} from '../lib/room-quote';
import { RoomQuoteMark } from '../lib/room-quote-mark';

// ──────────────────────────────────────────── Component ────
export interface InputBarProps {
  value: string;
  onChange: (next: string) => void;
  variant?: 'room' | 'embedded';
  footerSlot?: ReactNode;
  disabled?: boolean;
  placeholder?: string;
  captainId?: AgentId;
  targetAgent?: AgentId | null;
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  mentionAgentIds?: readonly AgentId[];
  broadcastMode?: boolean;
  broadcastRecipientCount?: number;
  broadcastPrivacyInfo?: {
    privateNames: readonly string[];
    publicNames: readonly string[];
  };
  privacyMode?: boolean;
  privacyControlsEnabled?: boolean;
  quoteProjectRoot?: string | null;
  /** Submit handler. Composer routing is explicit: selected target or
   *  confirmed broadcast recipients. The rich composer serializes file
   *  chips to escaped absolute paths right before submit. */
  onSend?: (
    target: AgentId | null,
    payload: string,
    options?: { broadcast: boolean; privacy: boolean; mentions?: ComposerMention[] },
  ) => boolean | void | Promise<boolean | void>;
  onPasteImage?: (file: File) => Promise<string | null>;
  onMaterializeAttachments?: (attachments: readonly ComposerAttachment[]) => Promise<ComposerAttachment[]>;
  onWhiteboardOpen?: () => void;
  onBroadcastToggle?: () => void;
  onPrivacyToggle?: () => void;
  onFocus?: () => void;
}

export interface InputBarHandle {
  focus: () => void;
  focusEnd: () => void;
  insertPaths: (paths: readonly string[]) => void;
  insertAttachment: (attachment: ComposerAttachment) => void;
  insertQuote: (quote: RoomQuoteReference) => RoomQuoteInsertResult;
  serialize: () => { payload: string; mentions: ComposerMention[] };
  clear: () => void;
}

export interface ComposerAttachment {
  path: string;
  name?: string;
  kind?: 'image' | 'file' | 'drawing' | 'prompt';
  previewUrl?: string;
  prompt?: string;
}

export interface ComposerMention {
  agentId: AgentId;
  aka: string;
}

const ATTACHMENT_SELECTOR = '[data-ib-attachment="true"]';
const MENTION_SELECTOR = '[data-ib-mention="true"]';
const PREFIX_PROMPT_SELECTOR = '[data-ib-prefix-prompt="true"]';
const BLOCK_TAGS = new Set(['DIV', 'P']);
const IMAGE_EXT_RE = /\.(apng|avif|gif|heic|jpeg|jpg|png|svg|tif|tiff|webp)$/i;
const TREE_DRAG_MIME = 'application/x-kota-file-path';

declare global {
  interface Window {
    __KOTA_FILE_TREE_DRAG_PATHS__?: string[];
  }
}

export function escapePromptPath(path: string): string {
  return path.replace(/([\\\s"'`$&;|<>()[\]{}*?!#~])/g, '\\$1');
}

function useTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

function basename(path: string): string {
  const normalized = path.replace(/\/+$/, '');
  const i = normalized.lastIndexOf('/');
  return decodeURIComponent(i >= 0 ? normalized.slice(i + 1) : normalized) || path;
}

function isImageAttachment(path: string, file?: File): boolean {
  if (file?.type?.startsWith('image/')) return true;
  return IMAGE_EXT_RE.test(path);
}

function pathFromFileUri(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  try {
    const url = new URL(trimmed);
    if (url.protocol !== 'file:') return null;
    return decodeURIComponent(url.pathname);
  } catch {
    return trimmed.startsWith('/') ? trimmed : null;
  }
}

function transferData(dataTransfer: DataTransfer, type: string): string {
  try {
    return dataTransfer.getData(type);
  } catch {
    return '';
  }
}

function clearTreeDragPaths() {
  if (typeof window === 'undefined') return;
  delete window.__KOTA_FILE_TREE_DRAG_PATHS__;
}

function dataTransferAttachments(
  dataTransfer: DataTransfer,
  options: { includePlainText?: boolean } = {},
): ComposerAttachment[] {
  const attachments: ComposerAttachment[] = [];
  const seen = new Set<string>();
  const addPath = (path: string | null | undefined, file?: File) => {
    if (!path || !path.startsWith('/') || seen.has(path)) return;
    seen.add(path);
    attachments.push({
      path,
      name: file?.name || basename(path),
      kind: isImageAttachment(path, file) ? 'image' : 'file',
    });
  };

  for (const file of Array.from(dataTransfer.files ?? [])) {
    const withPath = file as File & { path?: string; webkitRelativePath?: string };
    addPath(withPath.path || withPath.webkitRelativePath, file);
  }

  const types = options.includePlainText === false
    ? ['text/uri-list']
    : ['text/uri-list', 'text/plain'];
  for (const type of [TREE_DRAG_MIME, ...types]) {
    const data = transferData(dataTransfer, type);
    if (!data) continue;
    for (const line of data.split(/\r?\n/)) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith('#')) continue;
      addPath(pathFromFileUri(trimmed));
    }
  }

  if (typeof window !== 'undefined') {
    for (const path of window.__KOTA_FILE_TREE_DRAG_PATHS__ ?? []) {
      addPath(path);
    }
  }

  return attachments;
}

function pointCandidates(position: { x: number; y: number }): Array<{ x: number; y: number }> {
  const dpr = window.devicePixelRatio || 1;
  const raw = { x: position.x, y: position.y };
  const logical = { x: position.x / dpr, y: position.y / dpr };
  if (dpr === 1 || (raw.x === logical.x && raw.y === logical.y)) return [raw];

  const rawInViewport =
    raw.x >= 0 && raw.x <= window.innerWidth && raw.y >= 0 && raw.y <= window.innerHeight;
  const logicalInViewport =
    logical.x >= 0 && logical.x <= window.innerWidth && logical.y >= 0 && logical.y <= window.innerHeight;

  if (rawInViewport && !logicalInViewport) return [raw];
  if (!rawInViewport && logicalInViewport) return [logical];
  return [raw, logical];
}

function eventHitsEditor(
  payload: Extract<TauriDragDropPayload, { type: 'drop' }>,
  editor: HTMLElement,
): boolean {
  for (const point of pointCandidates(payload.position)) {
    const direct = document.elementFromPoint(point.x, point.y);
    if (direct && editor.contains(direct)) return true;
    const rect = editor.getBoundingClientRect();
    if (
      point.x >= rect.left &&
      point.x <= rect.right &&
      point.y >= rect.top &&
      point.y <= rect.bottom
    ) {
      return true;
    }
  }
  return false;
}

function isBlockElement(node: Node): node is HTMLElement {
  return node.nodeType === Node.ELEMENT_NODE && BLOCK_TAGS.has((node as HTMLElement).tagName);
}

function serializeNode(node: Node, options: { includeAttachments: boolean }): string {
  if (node.nodeType === Node.TEXT_NODE) {
    return node.nodeValue ?? '';
  }

  if (node.nodeType !== Node.ELEMENT_NODE) {
    return '';
  }

  const el = node as HTMLElement;
  if (el.matches(MENTION_SELECTOR)) {
    return el.dataset.aka ? `@${el.dataset.aka}` : el.textContent ?? '';
  }

  if (el.matches(ATTACHMENT_SELECTOR)) {
    if (!options.includeAttachments) return '';
    const prompt = el.dataset.prompt;
    if (prompt) return prompt;
    const path = el.dataset.path;
    return path ? escapePromptPath(path) : '';
  }

  if (el.tagName === 'BR') return '\n';

  let text = '';
  el.childNodes.forEach((child) => {
    const childText = serializeNode(child, options);
    if (isBlockElement(child) && text && !text.endsWith('\n')) text += '\n';
    text += childText;
    if (isBlockElement(child) && !text.endsWith('\n')) text += '\n';
  });
  return text;
}

function normalizeSerializedText(text: string): string {
  return text
    .replace(/\u00a0/g, ' ')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n{3,}/g, '\n\n');
}

function serializeEditor(
  editor: HTMLElement | null,
  quotes: readonly RoomQuoteReference[],
): string {
  if (!editor) return '';
  const body = normalizeSerializedText(serializeNode(editor, { includeAttachments: true }));
  return serializeRoomQuotePrompt(quotes, body);
}

function collectMentions(editor: HTMLElement | null): ComposerMention[] {
  if (!editor) return [];
  const out: ComposerMention[] = [];
  const seen = new Set<string>();
  editor.querySelectorAll(MENTION_SELECTOR).forEach((node) => {
    const el = node as HTMLElement;
    const agentId = el.dataset.agentId;
    const aka = el.dataset.aka;
    if (!agentId || !aka || seen.has(agentId)) return;
    seen.add(agentId);
    out.push({ agentId, aka });
  });
  return out;
}

function editorPlainText(editor: HTMLElement | null): string {
  if (!editor) return '';
  return normalizeSerializedText(serializeNode(editor, { includeAttachments: false }));
}

function clearEditor(editor: HTMLElement) {
  editor.replaceChildren();
}

function setEditorPlainText(editor: HTMLElement, text: string) {
  clearEditor(editor);
  const lines = text.split('\n');
  lines.forEach((line, index) => {
    if (index > 0) editor.appendChild(document.createElement('br'));
    if (line) editor.appendChild(document.createTextNode(line));
  });
}

function focusEditorAtEnd(editor: HTMLElement) {
  editor.focus({ preventScroll: true });
  const selection = window.getSelection();
  const range = document.createRange();
  range.selectNodeContents(editor);
  range.collapse(false);
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function selectionRangeForEditor(editor: HTMLElement): Range {
  const selection = window.getSelection();
  if (
    selection &&
    selection.rangeCount > 0 &&
    selection.anchorNode &&
    editor.contains(selection.anchorNode)
  ) {
    return selection.getRangeAt(0);
  }

  const range = document.createRange();
  range.selectNodeContents(editor);
  range.collapse(false);
  selection?.removeAllRanges();
  selection?.addRange(range);
  return range;
}

function textBeforeRange(editor: HTMLElement, range: Range): string {
  const before = range.cloneRange();
  before.selectNodeContents(editor);
  before.setEnd(range.startContainer, range.startOffset);
  return before.toString();
}

function activeMentionQuery(editor: HTMLElement): { query: string; startOffset: number } | null {
  const range = selectionRangeForEditor(editor);
  if (!range.collapsed) return null;
  const before = textBeforeRange(editor, range);
  const startOffset = before.lastIndexOf('@');
  if (startOffset < 0) return null;
  const query = before.slice(startOffset + 1);
  if (query.length > 0 && /[\s@]/.test(query)) return null;
  return {
    query,
    startOffset,
  };
}

function placeCaretAfter(node: Node) {
  const selection = window.getSelection();
  const range = document.createRange();
  range.setStartAfter(node);
  range.collapse(true);
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function replaceActiveMentionQuery(editor: HTMLElement, agentId: AgentId, aka: string): boolean {
  const query = activeMentionQuery(editor);
  if (!query) return false;
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0) return false;
  const range = selection.getRangeAt(0);
  const prefix = document.createRange();
  prefix.selectNodeContents(editor);
  prefix.setEnd(range.startContainer, range.startOffset);
  const text = prefix.toString();
  const start = Math.max(0, text.length - query.query.length - 1);
  const walker = document.createTreeWalker(editor, NodeFilter.SHOW_TEXT);
  let consumed = 0;
  let startNode: Text | null = null;
  let startNodeOffset = 0;
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    const next = consumed + (node.nodeValue?.length ?? 0);
    if (start <= next) {
      startNode = node;
      startNodeOffset = Math.max(0, start - consumed);
      break;
    }
    consumed = next;
  }
  if (!startNode) return false;
  const replaceRange = document.createRange();
  replaceRange.setStart(startNode, startNodeOffset);
  replaceRange.setEnd(range.startContainer, range.startOffset);
  replaceRange.deleteContents();
  const chip = createMentionChip(agentId, aka);
  const spacer = document.createTextNode(' ');
  replaceRange.insertNode(spacer);
  replaceRange.insertNode(chip);
  placeCaretAfter(spacer);
  return true;
}

function placeCaretAfterPrefixPrompts(editor: HTMLElement) {
  const prompts = Array.from(editor.querySelectorAll(PREFIX_PROMPT_SELECTOR));
  const lastPrompt = prompts.at(-1);
  if (!lastPrompt) return;
  const afterPrompt = lastPrompt.nextSibling?.nodeName === 'BR'
    ? lastPrompt.nextSibling
    : lastPrompt;
  placeCaretAfter(afterPrompt);
}

function selectionStartsBeforePrefixEnd(editor: HTMLElement): boolean {
  const prompts = Array.from(editor.querySelectorAll(PREFIX_PROMPT_SELECTOR));
  const lastPrompt = prompts.at(-1);
  if (!lastPrompt) return false;
  const selection = window.getSelection();
  if (
    !selection ||
    selection.rangeCount === 0 ||
    !selection.anchorNode ||
    !editor.contains(selection.anchorNode)
  ) {
    return false;
  }
  const current = selection.getRangeAt(0);
  const boundary = document.createRange();
  boundary.setStartAfter(lastPrompt.nextSibling?.nodeName === 'BR' ? lastPrompt.nextSibling : lastPrompt);
  boundary.collapse(true);
  return current.compareBoundaryPoints(Range.START_TO_START, boundary) < 0;
}

function moveCaretAfterPrefixIfNeeded(editor: HTMLElement): boolean {
  if (!selectionStartsBeforePrefixEnd(editor)) return false;
  placeCaretAfterPrefixPrompts(editor);
  return true;
}

function createAttachmentChip(attachment: ComposerAttachment): HTMLElement {
  const chip = document.createElement('span');
  chip.className = `ib-attachment-chip ${attachment.kind === 'image' ? 'image' : attachment.kind === 'drawing' ? 'drawing' : attachment.kind === 'prompt' ? 'prompt' : 'file'}`;
  chip.contentEditable = 'false';
  chip.dataset.ibAttachment = 'true';
  chip.dataset.path = attachment.path;
  chip.dataset.kind = attachment.kind ?? 'file';
  if (attachment.kind === 'prompt') chip.dataset.ibPrefixPrompt = 'true';
  if (attachment.previewUrl) chip.dataset.previewUrl = attachment.previewUrl;
  if (attachment.prompt) chip.dataset.prompt = attachment.prompt;
  chip.setAttribute('data-testid', 'ib-attachment-chip');
  chip.setAttribute('role', 'button');
  chip.setAttribute('aria-label', `${attachment.name || basename(attachment.path)} attachment`);

  const media = document.createElement('span');
  media.className = 'ib-attachment-media';
  if (attachment.kind === 'image' && attachment.previewUrl) {
    const image = document.createElement('img');
    image.alt = '';
    image.src = attachment.previewUrl;
    media.appendChild(image);
  } else {
    media.textContent = attachment.kind === 'image'
      ? 'IMG'
      : attachment.kind === 'drawing'
        ? 'DRAW'
        : attachment.kind === 'prompt'
          ? 'BBS'
          : 'FILE';
  }

  const name = document.createElement('span');
  name.className = 'ib-attachment-name';
  name.textContent = attachment.name || basename(attachment.path);

  const remove = document.createElement('span');
  remove.className = 'ib-attachment-remove';
  remove.dataset.attachmentRemove = 'true';
  remove.setAttribute('aria-hidden', 'true');
  remove.textContent = 'x';

  chip.append(media, name, remove);
  return chip;
}

function agentAka(name: string): string {
  const parts = splitProjectAgentName(name);
  const title = parts.title?.abbr ? `${parts.title.abbr} ` : '';
  return `${title}${parts.base || name}`.trim();
}

function createMentionChip(agentId: AgentId, aka: string): HTMLElement {
  const chip = document.createElement('span');
  chip.className = 'ib-mention-chip';
  chip.contentEditable = 'false';
  chip.dataset.ibMention = 'true';
  chip.dataset.agentId = agentId;
  chip.dataset.aka = aka;
  chip.setAttribute('data-testid', 'ib-mention-chip');
  chip.textContent = `@${aka}`;
  return chip;
}

function shortQuoteTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function normalizedProjectRoot(value?: string | null): string {
  return value?.trim().replace(/\/+$/, '') ?? '';
}

export const InputBar = forwardRef<InputBarHandle, InputBarProps>(function InputBar({
  value,
  onChange,
  variant = 'room',
  footerSlot,
  disabled = false,
  placeholder,
  captainId,
  targetAgent,
  agentMeta,
  mentionAgentIds = [],
  broadcastMode = false,
  broadcastRecipientCount = 0,
  broadcastPrivacyInfo,
  privacyMode = false,
  privacyControlsEnabled = false,
  quoteProjectRoot,
  onSend,
  onPasteImage,
  onMaterializeAttachments,
  onWhiteboardOpen,
  onBroadcastToggle,
  onPrivacyToggle,
  onFocus,
}: InputBarProps, ref) {
  const fieldRef = useRef<HTMLDivElement | null>(null);
  const previewUrlsRef = useRef<Set<string>>(new Set());
  const sendPendingRef = useRef(false);
  const quotesRef = useRef<RoomQuoteReference[]>([]);
  const [hasDraftContent, setHasDraftContent] = useState(() => value.trim().length > 0);
  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [sendPending, setSendPending] = useState(false);
  const [quotes, setQuotes] = useState<RoomQuoteReference[]>([]);
  const selectedTarget = targetAgent ?? captainId ?? null;
  const hasSendTarget = broadcastMode ? broadcastRecipientCount > 0 : !!selectedTarget;
  const embedded = variant === 'embedded';
  const canSend = !disabled && !sendPending && hasDraftContent && hasSendTarget;
  const sendTitle = 'Send (⌘↵)';
  const sendLabel = '▶';
  const effectivePrivacyMode = privacyControlsEnabled && privacyMode;
  const composerPlaceholder = placeholder
    ?? (effectivePrivacyMode ? 'Write a private prompt...' : 'Write a prompt, paste text, or drop files here...');
  const broadcastPrivateNames = privacyControlsEnabled ? broadcastPrivacyInfo?.privateNames ?? [] : [];
  const broadcastPublicNames = privacyControlsEnabled ? broadcastPrivacyInfo?.publicNames ?? [] : [];
  const broadcastAllPrivate =
    broadcastMode && broadcastPrivateNames.length > 0 && broadcastPublicNames.length === 0;
  const broadcastMixed =
    broadcastMode && broadcastPrivateNames.length > 0 && broadcastPublicNames.length > 0;
  const selectedTargetAgent = selectedTarget ? (agentMeta?.[selectedTarget] ?? AGENTS[selectedTarget]) : null;
  const targetLabel = selectedTargetAgent?.name ?? selectedTarget ?? 'None';
  const targetNameParts = splitProjectAgentName(targetLabel);
  const targetAvatarClass = selectedTargetAgent?.avatarClass ?? (selectedTarget ? avatarClassForAgentFallback(null, selectedTarget) : '');
  const targetAvatarStyle = avatarImageStyleForId(selectedTargetAgent?.avatarId);
  const broadcastTargetLabel = `${broadcastRecipientCount} Agents`;
  const targetPillInteractive = !!onBroadcastToggle;
  const mentionOptions = mentionQuery == null
    ? []
    : mentionAgentIds
      .map((id) => {
        const agent = agentMeta?.[id] ?? AGENTS[id];
        const name = agent?.name ?? id;
        return { id, agent, name, aka: agentAka(name) };
      })
      .filter((option) => (
        mentionQuery.trim().length === 0 ||
        option.aka.toLowerCase().includes(mentionQuery.toLowerCase()) ||
        option.name.toLowerCase().includes(mentionQuery.toLowerCase())
      ))
      .slice(0, MAX_AGENT_SLOTS);

  const updateQuotes = useCallback((next: readonly RoomQuoteReference[]) => {
    const copied = [...next];
    quotesRef.current = copied;
    setQuotes(copied);
  }, []);

  const syncEditorState = useCallback(() => {
    const editor = fieldRef.current;
    if (!editor) return;
    const serialized = serializeEditor(editor, quotesRef.current);
    setHasDraftContent(serialized.trim().length > 0);
    editor.dataset.empty = serialized.trim().length > 0 ? 'false' : 'true';
    onChange(editorPlainText(editor));
    const query = activeMentionQuery(editor);
    setMentionQuery(query?.query ?? null);
    setMentionIndex(0);
  }, [onChange]);

  const handleTargetPillClick = useCallback(() => {
    if (!targetPillInteractive) return;
    onBroadcastToggle?.();
  }, [onBroadcastToggle, targetPillInteractive]);

  const handleTargetPillKeyDown = useCallback((event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (!targetPillInteractive) return;
    if (event.key !== 'Enter' && event.key !== ' ') return;
    event.preventDefault();
    onBroadcastToggle?.();
  }, [onBroadcastToggle, targetPillInteractive]);

  const revokeChipPreview = useCallback((chip: Element | null) => {
    const url = (chip as HTMLElement | null)?.dataset.previewUrl;
    if (!url || !previewUrlsRef.current.has(url) || typeof URL.revokeObjectURL !== 'function') return;
    URL.revokeObjectURL(url);
    previewUrlsRef.current.delete(url);
  }, []);

  const clearDraft = useCallback(() => {
    const editor = fieldRef.current;
    if (editor) {
      editor.querySelectorAll(ATTACHMENT_SELECTOR).forEach(revokeChipPreview);
      clearEditor(editor);
      editor.dataset.empty = 'true';
    }
    updateQuotes([]);
    setHasDraftContent(false);
    onChange('');
  }, [onChange, revokeChipPreview, updateQuotes]);

  useLayoutEffect(() => {
    const editor = fieldRef.current;
    if (!editor) return;
    const plainText = editorPlainText(editor);
    if (value === plainText) {
      editor.dataset.empty = serializeEditor(editor, quotesRef.current).trim().length > 0
        ? 'false'
        : 'true';
      return;
    }
    editor.querySelectorAll(ATTACHMENT_SELECTOR).forEach(revokeChipPreview);
    setEditorPlainText(editor, value);
    const serialized = serializeEditor(editor, quotesRef.current);
    setHasDraftContent(serialized.trim().length > 0);
    editor.dataset.empty = serialized.trim().length > 0 ? 'false' : 'true';
  }, [value, revokeChipPreview]);

  useLayoutEffect(() => {
    const el = fieldRef.current;
    if (!el) return;
    el.style.height = '0px';
    el.style.height = `${Math.min(190, Math.max(112, el.scrollHeight))}px`;
  }, [value, hasDraftContent]);

  useEffect(() => () => {
    previewUrlsRef.current.forEach((url) => {
      if (typeof URL.revokeObjectURL === 'function') URL.revokeObjectURL(url);
    });
    previewUrlsRef.current.clear();
  }, []);

  const insertTextAtCursor = useCallback(
    (text: string) => {
      const editor = fieldRef.current;
      if (!editor || !text || disabled) return;
      editor.focus({ preventScroll: true });
      moveCaretAfterPrefixIfNeeded(editor);
      const range = selectionRangeForEditor(editor);
      range.deleteContents();
      const node = document.createTextNode(text);
      range.insertNode(node);
      placeCaretAfter(node);
      syncEditorState();
    },
    [disabled, syncEditorState],
  );

  const insertAttachments = useCallback(
    (attachments: readonly ComposerAttachment[]) => {
      const editor = fieldRef.current;
      if (!editor || disabled) return;
      const unique = attachments.filter((attachment, index) => (
        !!attachment.path && attachments.findIndex((item) => item.path === attachment.path) === index
      ));
      if (unique.length === 0) return;

      const promptAttachments = unique.filter((attachment) => attachment.kind === 'prompt');
      const regularAttachments = unique.filter((attachment) => attachment.kind !== 'prompt');
      if (promptAttachments.length > 0) {
        editor.querySelectorAll(PREFIX_PROMPT_SELECTOR).forEach((node) => node.remove());
        const fragment = document.createDocumentFragment();
        promptAttachments.forEach((attachment) => {
          fragment.appendChild(createAttachmentChip(attachment));
          fragment.appendChild(document.createElement('br'));
        });
        editor.insertBefore(fragment, editor.firstChild);
        focusEditorAtEnd(editor);
      }

      if (regularAttachments.length === 0) {
        syncEditorState();
        return;
      }

      editor.focus({ preventScroll: true });
      moveCaretAfterPrefixIfNeeded(editor);
      const range = selectionRangeForEditor(editor);
      range.deleteContents();

      const fragment = document.createDocumentFragment();
      const beforeText = textBeforeRange(editor, range);
      if (beforeText.length > 0 && !/\s$/.test(beforeText)) {
        fragment.appendChild(document.createTextNode(' '));
      }

      regularAttachments.forEach((attachment) => {
        fragment.appendChild(createAttachmentChip(attachment));
        fragment.appendChild(document.createTextNode(' '));
      });

      const caret = document.createTextNode('');
      fragment.appendChild(caret);
      range.insertNode(fragment);
      placeCaretAfter(caret);
      syncEditorState();
    },
    [disabled, syncEditorState],
  );

  const materializeAndInsert = useCallback(
    async (attachments: readonly ComposerAttachment[]) => {
      const next = onMaterializeAttachments
        ? await onMaterializeAttachments(attachments)
        : [...attachments];
      insertAttachments(next);
    },
    [insertAttachments, onMaterializeAttachments],
  );

  const insertPaths = useCallback(
    (paths: readonly string[]) => {
      void materializeAndInsert(paths.map((path) => ({
        path,
        name: basename(path),
        kind: isImageAttachment(path) ? 'image' : 'file',
      })));
    },
    [materializeAndInsert],
  );

  const insertAttachment = useCallback(
    (attachment: ComposerAttachment) => {
      insertAttachments([attachment]);
    },
    [insertAttachments],
  );

  const insertQuote = useCallback((quote: RoomQuoteReference): RoomQuoteInsertResult => {
    const editor = fieldRef.current;
    if (!editor || disabled) return 'blocked';
    const currentRoot = normalizedProjectRoot(quoteProjectRoot);
    const originRoot = normalizedProjectRoot(quote.projectRoot);
    if (!currentRoot || !originRoot || currentRoot !== originRoot) return 'blocked';
    const existing = quotesRef.current;
    if (existing.some((item) => item.ref === quote.ref)) return 'duplicate';
    if (existing.length >= ROOM_QUOTE_MAX) return 'limit';

    updateQuotes([...existing, quote]);
    setHasDraftContent(true);
    editor.dataset.empty = 'false';
    focusEditorAtEnd(editor);
    return 'inserted';
  }, [disabled, quoteProjectRoot, updateQuotes]);

  useEffect(() => {
    const currentRoot = normalizedProjectRoot(quoteProjectRoot);
    const existing = quotesRef.current;
    const next = existing.filter(
      (quote) => normalizedProjectRoot(quote.projectRoot) === currentRoot,
    );
    if (next.length === existing.length) return;
    updateQuotes(next);
    syncEditorState();
  }, [quoteProjectRoot, syncEditorState, updateQuotes]);

  const removeQuote = useCallback((ref: string) => {
    const existing = quotesRef.current;
    const next = existing.filter((quote) => quote.ref !== ref);
    if (next.length === existing.length) return;
    updateQuotes(next);
    syncEditorState();
    if (fieldRef.current) focusEditorAtEnd(fieldRef.current);
  }, [syncEditorState, updateQuotes]);

  useImperativeHandle(ref, () => ({
    focus: () => fieldRef.current?.focus({ preventScroll: true }),
    focusEnd: () => {
      if (fieldRef.current) focusEditorAtEnd(fieldRef.current);
    },
    insertPaths,
    insertAttachment,
    insertQuote,
    serialize: () => ({
      payload: serializeEditor(fieldRef.current, quotesRef.current).trimEnd(),
      mentions: collectMentions(fieldRef.current),
    }),
    clear: clearDraft,
  }), [clearDraft, insertAttachment, insertPaths, insertQuote]);

  useEffect(() => {
    if (disabled || !useTauriRuntime()) return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    void import('@tauri-apps/api/webview')
      .then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent((event) => {
          const editor = fieldRef.current;
          if (!editor || event.payload.type !== 'drop') return;
          if (!eventHitsEditor(event.payload, editor)) return;
          insertPaths(event.payload.paths);
        }),
      )
      .then((stopListening) => {
        if (cancelled) stopListening?.();
        else unlisten = stopListening ?? null;
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [disabled, insertPaths]);

  const handleSend = async () => {
    if (!onSend) return;
    if (sendPendingRef.current) return;
    const payload = serializeEditor(fieldRef.current, quotesRef.current).trimEnd();
    const mentions = collectMentions(fieldRef.current);
    if (payload.length === 0) return;
    if (!canSend) return;
    sendPendingRef.current = true;
    setSendPending(true);
    try {
      const options: { broadcast: boolean; privacy: boolean; mentions?: ComposerMention[] } = {
        broadcast: broadcastMode,
        privacy: effectivePrivacyMode,
      };
      if (mentions.length > 0) options.mentions = mentions;
      const result = await onSend(broadcastMode ? null : selectedTarget, payload, options);
      if (result === false) return;
      clearDraft();
    } catch (err) {
      console.error('[composer] send failed', err);
    } finally {
      sendPendingRef.current = false;
      setSendPending(false);
    }
  };

  const previewUrlForFile = (file: File): string | undefined => {
    if (!file.type.startsWith('image/') || typeof URL.createObjectURL !== 'function') return undefined;
    const url = URL.createObjectURL(file);
    previewUrlsRef.current.add(url);
    return url;
  };

  const handlePaste = (e: ReactClipboardEvent<HTMLDivElement>) => {
    if (disabled) return;
    if (fieldRef.current) moveCaretAfterPrefixIfNeeded(fieldRef.current);
    const attachments = dataTransferAttachments(e.clipboardData, { includePlainText: false });
    if (attachments.length > 0) {
      e.preventDefault();
      void materializeAndInsert(attachments);
      return;
    }

    const imageFile = Array.from(e.clipboardData.files ?? []).find((file) =>
      file.type.startsWith('image/'),
    );
    if (imageFile && onPasteImage) {
      e.preventDefault();
      void onPasteImage(imageFile).then((path) => {
        if (!path) return;
        insertAttachments([{
          path,
          name: imageFile.name || basename(path),
          kind: 'image',
          previewUrl: previewUrlForFile(imageFile),
        }]);
      });
      return;
    }

    const text = e.clipboardData.getData('text/plain');
    if (!text) return;
    e.preventDefault();
    insertTextAtCursor(text);
  };

  const handleDrop = (e: ReactDragEvent<HTMLDivElement>) => {
    if (disabled) return;
    e.preventDefault();
    e.stopPropagation();
    const attachments = dataTransferAttachments(e.dataTransfer, { includePlainText: true });
    void materializeAndInsert(attachments);
    clearTreeDragPaths();
  };

  const handleDragOver = (e: ReactDragEvent<HTMLDivElement>) => {
    if (disabled) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
  };

  const handleKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (disabled) return;
    if (mentionQuery != null && mentionOptions.length > 0) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        setMentionIndex((prev) => {
          const delta = e.key === 'ArrowDown' ? 1 : -1;
          return (prev + delta + mentionOptions.length) % mentionOptions.length;
        });
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        const option = mentionOptions[Math.min(mentionIndex, mentionOptions.length - 1)];
        if (option && fieldRef.current && replaceActiveMentionQuery(fieldRef.current, option.id, option.aka)) {
          setMentionQuery(null);
          syncEditorState();
        }
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        setMentionQuery(null);
        return;
      }
    }
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void handleSend();
      return;
    }
    const editingKey = e.key.length === 1 || e.key === 'Enter' || e.key === 'Backspace' || e.key === 'Delete';
    if (editingKey && fieldRef.current && selectionStartsBeforePrefixEnd(fieldRef.current)) {
      e.preventDefault();
      placeCaretAfterPrefixPrompts(fieldRef.current);
    }
  };

  const handleBeforeInput = (e: ReactFormEvent<HTMLDivElement>) => {
    if (disabled) return;
    if (!fieldRef.current) return;
    if (!selectionStartsBeforePrefixEnd(fieldRef.current)) return;
    e.preventDefault();
    placeCaretAfterPrefixPrompts(fieldRef.current);
  };

  const handleClick = (e: ReactMouseEvent<HTMLDivElement>) => {
    if (disabled) return;
    const target = e.target as HTMLElement;
    const remove = target.closest('[data-attachment-remove="true"]');
    if (!remove) return;
    e.preventDefault();
    const chip = remove.closest(ATTACHMENT_SELECTOR);
    revokeChipPreview(chip);
    chip?.remove();
    syncEditorState();
    fieldRef.current?.focus({ preventScroll: true });
  };

  return (
    <div
      className={[
        'input-bar-wrap',
        embedded ? 'embedded' : '',
        disabled ? 'disabled' : '',
        broadcastMode ? 'broadcast' : '',
        broadcastAllPrivate ? 'broadcast-private' : '',
        broadcastMixed ? 'broadcast-mixed' : '',
        effectivePrivacyMode ? 'private' : '',
      ].filter(Boolean).join(' ')}
      data-privacy={effectivePrivacyMode ? 'true' : 'false'}
      data-broadcast={broadcastMode ? 'true' : 'false'}
      data-variant={variant}
      onDragOver={disabled ? undefined : handleDragOver}
      onDrop={disabled ? undefined : handleDrop}
    >
      <div className="input-bar">
        {!embedded && (effectivePrivacyMode || broadcastMode) && (
          <div className="ib-mode-indicator" data-testid="ib-mode-indicator">
            {broadcastMode ? (
              broadcastMixed ? (
                <>
                  <span className="ib-mode-broadcast">Broadcast</span>
                  <span aria-hidden> · </span>
                  <span className="ib-mode-private">{broadcastPrivateNames.join(', ')} private</span>
                  <span aria-hidden> · </span>
                  <span className="ib-mode-public">{broadcastPublicNames.join(', ')} public</span>
                </>
              ) : (
                <>
                  <span className="ib-mode-broadcast">Broadcast · {broadcastRecipientCount} selected</span>
                  {broadcastAllPrivate && <span className="ib-mode-private"> · all private</span>}
                </>
              )
            ) : (
              effectivePrivacyMode && <span>Private prompt · Violet paused</span>
            )}
          </div>
        )}
        {quotes.length > 0 && (
          <div
            className="ib-quote-shelf"
            aria-label="Quoted messages"
            data-testid="ib-quote-shelf"
          >
            {quotes.map((quote) => {
              const to = quote.to.map((party) => party.name).join(', ') || 'Room';
              return (
                <div
                  key={quote.ref}
                  className="ib-quote-chip"
                  data-testid="ib-quote-chip"
                  aria-label={`Quote from ${quote.from.name}`}
                  onMouseDown={(event) => event.preventDefault()}
                >
                  <span className="ib-quote-mark">
                    <RoomQuoteMark />
                  </span>
                  <span className="ib-quote-copy">
                    <span className="ib-quote-meta">
                      {quote.from.name} → {to} · {shortQuoteTime(quote.at)}
                    </span>
                    <span className="ib-quote-excerpt">{quote.excerpt}</span>
                  </span>
                  <button
                    type="button"
                    className="ib-quote-remove"
                    aria-label={`Remove quote from ${quote.from.name}`}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => removeQuote(quote.ref)}
                    disabled={disabled}
                  >
                    <span aria-hidden="true">×</span>
                  </button>
                </div>
              );
            })}
          </div>
        )}
        <div
          ref={fieldRef}
          className="ib-field ib-rich-field"
          role="textbox"
          aria-multiline="true"
          aria-label="Prompt composer"
          contentEditable={!disabled}
          suppressContentEditableWarning
          spellCheck={false}
          data-empty={hasDraftContent ? 'false' : 'true'}
          data-placeholder={composerPlaceholder}
          onFocus={onFocus}
          onInput={disabled ? undefined : syncEditorState}
          onBeforeInput={disabled ? undefined : handleBeforeInput}
          onPaste={disabled ? undefined : handlePaste}
          onClick={disabled ? undefined : handleClick}
          onDragOver={disabled ? undefined : handleDragOver}
          onDrop={disabled ? undefined : handleDrop}
          onKeyDown={disabled ? undefined : handleKeyDown}
          data-testid="input-field"
          data-passthrough="false"
        />
        {mentionQuery != null && mentionOptions.length > 0 && (
          <div className="ib-mention-popover" data-testid="ib-mention-popover">
            {mentionOptions.map((option, index) => {
              const avatarClass = option.agent?.avatarClass ?? avatarClassForAgentFallback(null, option.id);
              return (
                <button
                  key={option.id}
                  type="button"
                  className={index === mentionIndex ? 'active' : ''}
                  onMouseDown={(event) => {
                    event.preventDefault();
                    if (fieldRef.current && replaceActiveMentionQuery(fieldRef.current, option.id, option.aka)) {
                      setMentionQuery(null);
                      syncEditorState();
                    }
                  }}
                >
                  <span
                    className={[
                      'ib-mention-avatar',
                      'tavern-avatar-art',
                      avatarClass,
                    ].filter(Boolean).join(' ')}
                    style={avatarImageStyleForId(option.agent?.avatarId)}
                    aria-hidden
                  >
                    <span />
                    <i />
                    <b />
                  </span>
                  <span className="ib-mention-name">
                    <ProjectAgentName name={option.name} compact />
                  </span>
                </button>
              );
            })}
          </div>
        )}
        <div className="ib-toolbar">
          <div className="ib-toolbar-left">
            <button
              type="button"
              className="ib-tool-btn"
              title="Open whiteboard"
              aria-label="Open whiteboard"
              onClick={onWhiteboardOpen}
              disabled={disabled || !onWhiteboardOpen}
              data-testid="ib-whiteboard-tool"
            >
              <WhiteboardToolIcon />
            </button>
            {!embedded && privacyControlsEnabled && (
              <button
                type="button"
                className={['ib-tool-btn', effectivePrivacyMode ? 'active private' : ''].filter(Boolean).join(' ')}
                onClick={onPrivacyToggle}
                disabled={!onPrivacyToggle || !selectedTarget}
                title="Toggle current target privacy"
                aria-label="Toggle current target privacy"
                data-testid="ib-privacy-tool"
              >
                <PrivacyToolIcon />
              </button>
            )}
            {!embedded && quotes.length >= ROOM_QUOTE_MAX && (
              <span className="ib-quote-cap" data-testid="ib-quote-cap">4 quotes max</span>
            )}
          </div>
          <div className="ib-toolbar-spacer" />
          {!embedded && (
          <div className="ib-target-stack">
            <div
              className={[
                'chip',
                'ib-target-pill',
                effectivePrivacyMode ? 'private' : '',
                broadcastMode ? 'broadcast' : '',
                targetPillInteractive ? 'interactive' : '',
              ].filter(Boolean).join(' ')}
              title={broadcastMode ? `${broadcastTargetLabel} · ${AGENT_SLOT_KEY_RANGE_LABEL} toggles, Enter confirms, Esc cancels` : `${targetLabel} · PageUp/PageDown cycles table target`}
              data-full-name={broadcastMode ? broadcastTargetLabel : targetLabel}
              data-testid="ib-target-pill"
              role={targetPillInteractive ? 'button' : undefined}
              tabIndex={targetPillInteractive ? 0 : undefined}
              aria-label={targetPillInteractive ? 'Open target selection' : undefined}
              onClick={handleTargetPillClick}
              onKeyDown={handleTargetPillKeyDown}
            >
              {broadcastMode ? (
                <span className="chip-name">
                  <span className="chip-name-base">{broadcastTargetLabel}</span>
                </span>
              ) : (
                <>
                  <span className={`chip-avatar tavern-avatar-art ${targetAvatarClass}`} style={targetAvatarStyle} aria-hidden>
                    <span />
                    <i />
                    <b />
                  </span>
                  <span className="chip-name">
                    {targetNameParts.title && (
                      <span className={`project-agent-name-title chip-title title-${targetNameParts.title.id}`}>
                        {targetNameParts.title.abbr}
                      </span>
                    )}
                    <span className="chip-name-base">{targetNameParts.base}</span>
                    {effectivePrivacyMode && <TargetLockMini />}
                  </span>
                </>
              )}
            </div>
          </div>
          )}
          {!embedded && (
          <button
            className="ib-send"
            type="button"
            title={sendTitle}
            aria-label={sendTitle}
            onClick={() => { void handleSend(); }}
            data-testid="ib-send"
            disabled={!canSend}
          >
            {sendLabel}
          </button>
          )}
        </div>
        {footerSlot && <div className="ib-footer-slot">{footerSlot}</div>}
      </div>
    </div>
  );
});

function PrivacyToolIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M5 7V5.7C5 3.9 6.2 2.8 8 2.8s3 1.1 3 2.9V7" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
      <rect x="4" y="6.7" width="8" height="6" rx="1.4" stroke="currentColor" strokeWidth="1.25" />
    </svg>
  );
}

function WhiteboardToolIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="3" y="3.8" width="8.2" height="8" rx="1.4" stroke="currentColor" strokeWidth="1.2" />
      <path d="m9.2 11.7 3.7-3.7.9.9-3.7 3.7-1.3.4.4-1.3Z" fill="currentColor" />
    </svg>
  );
}

function TargetLockMini() {
  return (
    <svg className="chip-private-lock" viewBox="0 0 12 12" fill="none" aria-hidden>
      <path d="M3.8 5.2V4.1c0-1.4.9-2.3 2.2-2.3s2.2.9 2.2 2.3v1.1" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
      <rect x="2.8" y="5" width="6.4" height="5" rx="1.1" stroke="currentColor" strokeWidth="1" />
    </svg>
  );
}
