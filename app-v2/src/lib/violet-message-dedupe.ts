export type PreparedDedupeText = {
  normalized: string;
  attachmentInsensitive: string;
};

export function prepareDedupeText(text: string): PreparedDedupeText {
  return {
    normalized: normalizeForDedupe(text),
    attachmentInsensitive: normalizeAttachmentInsensitive(text),
  };
}

export function preparedDedupeTextsMatch(
  nativeText: PreparedDedupeText,
  localText: PreparedDedupeText,
): boolean {
  if (!nativeText.normalized) return false;
  if (nativeText.normalized === localText.normalized) return true;
  // Attachment sends never match verbatim: the composer payload carries
  // the attachment path inline while provider logs rewrite it to a marker.
  // Empty attachment-free values intentionally confirm attachment-only sends.
  return nativeText.attachmentInsensitive === localText.attachmentInsensitive;
}

export function timestampsWithinComposerConfirmationWindow(left: number, right: number): boolean {
  if (!Number.isFinite(left) || !Number.isFinite(right)) return true;
  return Math.abs(left - right) < 15 * 60 * 1000;
}

/* Erase attachment decorations from both sides of the local/native compare:
   provider markers ("[Image #1]"), provider source trailers
   ("[Image: source: /path/to.png]"), and inline attachment paths the
   composer serializes for chips. */
export function normalizeAttachmentInsensitive(text: string): string {
  let normalized = text;
  if (normalized.includes('#')) {
    normalized = normalized.replace(/\[[^\]\n#]{1,40}#\d+\]/g, ' ');
  }
  if (normalized.includes(': source:')) {
    normalized = normalized.replace(/\[[^\]\n:]{1,20}: source: [^\]\n]+\]/g, ' ');
  }
  return normalizeForDedupe(stripAttachmentPathTokens(normalized));
}

export function normalizeForDedupe(text: string): string {
  return normalizeSharedAttachmentPathsForDedupe(trimTerminalControlPadding(text))
    .replace(/\s+/g, ' ')
    .trim();
}

function normalizeSharedAttachmentPathsForDedupe(text: string): string {
  if (!text.includes(SHARED_ATTACHMENT_PATH)) return text;
  // Token scanning is deliberately linear. A previous unanchored `\S*` path
  // regex backtracked catastrophically on long URLs without attachments.
  return text.replace(/\S+/gu, canonicalizeAttachmentPathToken);
}

const SHARED_ATTACHMENT_PATH = 'project-memory/attachments/';
const ATTACHMENT_PATH_BOUNDARIES = new Set(['(', '[', '{', '"', "'", '`']);

function stripAttachmentPathTokens(text: string): string {
  if (!text.includes(SHARED_ATTACHMENT_PATH)) return text;
  return text.replace(/\S+/gu, (token) => (
    token.includes(SHARED_ATTACHMENT_PATH) ? ' ' : token
  ));
}

function canonicalizeAttachmentPathToken(token: string): string {
  const needleIndex = token.indexOf(SHARED_ATTACHMENT_PATH);
  if (needleIndex <= 0) return token;
  const prefix = token.slice(0, needleIndex);
  let boundaryIndex = -1;
  for (let index = prefix.length - 1; index >= 0; index -= 1) {
    if (ATTACHMENT_PATH_BOUNDARIES.has(prefix[index]!)) {
      boundaryIndex = index;
      break;
    }
  }
  const pathPrefix = prefix.slice(boundaryIndex + 1);
  if (!pathPrefix.startsWith('/') && !pathPrefix.startsWith('~/')) return token;
  return `${prefix.slice(0, boundaryIndex + 1)}${token.slice(needleIndex)}`;
}

function trimTerminalControlPadding(text: string): string {
  return text.replace(
    /^[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]+|[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]+$/gu,
    '',
  );
}
