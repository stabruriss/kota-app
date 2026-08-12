import { forwardRef, useState } from 'react';
import {
  InputBar,
  type InputBarHandle,
  type InputBarProps,
} from './InputBar';

export type RoomComposerProps = Omit<InputBarProps, 'value' | 'onChange'>;

/** Keeps the room draft below App so ordinary typing only updates this leaf. */
export const RoomComposer = forwardRef<InputBarHandle, RoomComposerProps>(function RoomComposer(
  props,
  ref,
) {
  const [draft, setDraft] = useState('');

  return (
    <InputBar
      {...props}
      ref={ref}
      value={draft}
      onChange={setDraft}
    />
  );
});
