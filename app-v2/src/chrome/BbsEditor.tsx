import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef } from 'react';
import { bbsValidateAttachments, type BbsAttachmentSource } from '../pty-client';
import { InputBar, type ComposerAttachment, type InputBarHandle, type InputBarProps } from './InputBar';

export const BBS_ATTACHMENT_MAX_BYTES = 1024 ** 3;
export const BBS_ATTACHMENT_MAX_COUNT = 9;

export interface BbsEditorState { hasContent: boolean; preparing: boolean }
export interface BbsEditorHandle {
  snapshot(): { body: string; attachments: BbsAttachmentSource[]; preparing: boolean };
  clear(): void;
}

interface Props extends Pick<InputBarProps, 'value' | 'onChange' | 'disabled' | 'placeholder' | 'agentMeta' | 'mentionAgentIds' | 'onPasteImage'> {
  onStateChange(state: BbsEditorState): void;
  onError(message: string): void;
}

/** BBS-only attachment lifecycle. The room's callbacks and serializer are not reused/changed. */
export const BbsEditor = forwardRef<BbsEditorHandle, Props>(function BbsEditor({
  onChange, onStateChange, onError, onPasteImage, ...props
}, ref) {
  const input = useRef<InputBarHandle | null>(null);
  const preparing = useRef(false);
  const mounted = useRef(true);
  const callbacks = useRef({ onChange, onStateChange, onError });
  callbacks.current = { onChange, onStateChange, onError };

  const snapshot = useCallback(() => {
    const draft = input.current?.serializeBbs();
    return {
      body: draft?.body ?? '',
      attachments: (draft?.attachments ?? []).map(({ path, name }) => ({ path, name })),
      preparing: preparing.current,
    };
  }, []);
  const publishState = useCallback(() => {
    if (!mounted.current) return;
    const draft = snapshot();
    callbacks.current.onStateChange({
      hasContent: !!draft.body.trim() || draft.attachments.length > 0,
      preparing: draft.preparing,
    });
  }, [snapshot]);
  useEffect(() => {
    mounted.current = true;
    publishState();
    return () => { mounted.current = false; };
  }, [publishState]);

  useImperativeHandle(ref, () => ({ snapshot, clear: () => input.current?.clear() }), [snapshot]);

  const begin = () => {
    if (preparing.current) {
      callbacks.current.onError('Please wait for the current attachment to finish preparing.');
      return false;
    }
    preparing.current = true;
    publishState();
    return true;
  };
  const finish = () => { preparing.current = false; publishState(); };
  const reportError = (error: unknown) => {
    if (mounted.current) callbacks.current.onError(`${String(error)} Retry or remove the attachment.`);
  };

  const materialize = async (incoming: readonly ComposerAttachment[]): Promise<ComposerAttachment[]> => {
    if (!begin()) return [];
    try {
      const existing = snapshot().attachments;
      if (existing.length + incoming.length > BBS_ATTACHMENT_MAX_COUNT) throw new Error('9 attachments max.');
      // Only stat/open in this IPC. The actual copy happens once, at publication.
      const validated = await bbsValidateAttachments([...existing, ...incoming]);
      if (!mounted.current) return [];
      return incoming.map((attachment, index) => ({ ...attachment, name: validated[existing.length + index]!.name }));
    } catch (error) {
      reportError(error);
      return [];
    } finally { finish(); }
  };

  const pasteImage = async (file: File): Promise<string | null> => {
    if (!begin()) return null;
    try {
      if (file.size > BBS_ATTACHMENT_MAX_BYTES) throw new Error(`${file.name || 'Image'} exceeds 1 GiB.`);
      const existing = snapshot().attachments;
      if (existing.length >= BBS_ATTACHMENT_MAX_COUNT) throw new Error('9 attachments max.');
      const validated = await bbsValidateAttachments(existing);
      if (validated.reduce((sum, item) => sum + item.sizeBytes, file.size) > BBS_ATTACHMENT_MAX_BYTES) {
        throw new Error('Attachments exceed 1 GiB in total.');
      }
      if (!mounted.current) return null;
      // Clipboard pixels have no source path. Retain the existing temporary image saver.
      const path = await onPasteImage?.(file);
      if (!path) throw new Error(`Could not save ${file.name || 'clipboard image'}.`);
      return mounted.current ? path : null;
    } catch (error) {
      reportError(error);
      return null;
    } finally { finish(); }
  };

  return <InputBar
    {...props}
    ref={input}
    variant="embedded"
    onChange={(body) => { callbacks.current.onChange(body); publishState(); }}
    onMaterializeAttachments={materialize}
    onPasteImage={pasteImage}
  />;
});
