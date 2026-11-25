import { act } from '@testing-library/react';
import { vi } from 'vitest';
import type * as ipc from '../src/pty-client';

type Ended<T> = T extends { id: string } ? Omit<T, 'id'> : never;
export const GH_OUTPUT = '\n! First copy your one-time code: WDJB-MJHT\nOpen this URL to continue in your web browser: https://github.com/login/device\n';

/** Stands in for the backend's sign-in. Use with a mocked pty-client. */
export function fakeGithubSignIn(mocked: Pick<typeof ipc, 'onGithubSignIn' | 'githubSignInStart' | 'githubSignInCancel'>) {
  const listeners = new Set<(event: ipc.GithubSignInEvent) => void>();
  const ids: string[] = [];
  vi.mocked(mocked.onGithubSignIn).mockImplementation(async callback => { listeners.add(callback); return () => { listeners.delete(callback); }; });
  vi.mocked(mocked.githubSignInStart).mockImplementation(async id => { ids.push(id); });
  vi.mocked(mocked.githubSignInCancel).mockResolvedValue(undefined);
  const send = (id: string, event: Ended<ipc.GithubSignInEvent>) =>
    act(async () => { [...listeners].forEach(listener => listener({ ...event, id } as ipc.GithubSignInEvent)); });
  return {
    ids, listeners, send,
    emit: (event: Ended<ipc.GithubSignInEvent>) => send(ids[ids.length - 1], event),
    print: (output = GH_OUTPUT, deadlineMs = Date.now() + 600_000) => send(ids[ids.length - 1], { kind: 'output', output, deadlineMs }),
    signIn: (result: Partial<ipc.GithubSignInResult> & Pick<ipc.GithubSignInResult, 'login' | 'view'>) =>
      send(ids[ids.length - 1], { kind: 'signed_in', result: { added: true, active: 'original', restoreFailed: false, ...result } }),
  };
}
