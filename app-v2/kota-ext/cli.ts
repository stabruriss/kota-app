import { spawn } from 'node:child_process';
import type { CommandInvocation, ToolName } from './tool-command.ts';

export type WrapperError = 'invalid_arguments' | 'spawn_failed' | 'cancelled' | 'timeout' | 'output_limit';
export interface CommandResult {
  exitCode: number | null;
  stdout: string;
  stderr: string;
  error?: WrapperError;
}
export type FailureCopy = Record<WrapperError, string>;
// Approved TOOLS-FINAL.md, SHA-256 7e422261e0029a9ff15a613c58505ad1a97e3a1dde2c2844b50246bbaa40b7b5.
export const FAILURE_COPY: FailureCopy = {
  invalid_arguments: 'Invalid tool arguments.',
  spawn_failed: 'Could not start the Kota command.',
  cancelled: 'The Kota command was cancelled. Its side effects may already have occurred.',
  timeout: 'The Kota command timed out. Its side effects may already have occurred.',
  output_limit: 'The Kota command exceeded the output limit. Its output is incomplete; its side effects may already have occurred.',
};
export const OUTPUT_LIMIT_BYTES = 1024 * 1024;
export const COMMAND_TIMEOUT_MS = 120_000;
// The Bartender CLI itself waits up to 30 minutes; do not preempt it at the ordinary tool deadline.
export const BARTENDER_TIMEOUT_MS = 1_815_000;
export const timeoutForTool = (name: ToolName) => name === 'kota_bartender' ? BARTENDER_TIMEOUT_MS : COMMAND_TIMEOUT_MS;

export function wrapperFailure(error: WrapperError, copy: FailureCopy, result: CommandResult = { exitCode: null, stdout: '', stderr: '' }): CommandResult {
  return { ...result, stderr: result.stderr ? `${result.stderr}\n${copy[error]}` : copy[error], error };
}

/** Spawn fixed argv with raw stdin. The caller supplies approved copy, never arbitrary exception prose. */
export async function executeCommand(invocation: CommandInvocation, {
  cwd, signal, copy, timeoutMs = COMMAND_TIMEOUT_MS, outputLimit = OUTPUT_LIMIT_BYTES,
}: { cwd: string; signal?: AbortSignal; copy: FailureCopy; timeoutMs?: number; outputLimit?: number }): Promise<CommandResult> {
  if (signal?.aborted) return wrapperFailure('cancelled', copy);
  return new Promise(resolve => {
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    const lengths = { stdout: 0, stderr: 0 };
    let error: WrapperError | undefined;
    let killTimer: ReturnType<typeof setTimeout> | undefined;
    const child = spawn(invocation.command, invocation.args, {
      cwd, stdio: ['pipe', 'pipe', 'pipe'], shell: false, detached: process.platform !== 'win32',
    });

    function killProcess(hard = false) {
      const sig = hard ? 'SIGKILL' : 'SIGTERM';
      try {
        if (process.platform !== 'win32' && child.pid) process.kill(-child.pid, sig);
        else child.kill(sig);
      } catch { /* A concurrent exit needs no second failure. */ }
    }
    function stop(reason: WrapperError) {
      if (error) return;
      error = reason;
      child.stdin.destroy();
      killProcess();
      killTimer = setTimeout(() => {
        killProcess(true);
        child.stdout.destroy();
        child.stderr.destroy();
      }, 500);
      killTimer.unref();
    }
    function capture(stream: 'stdout' | 'stderr', chunk: Buffer) {
      const remaining = Math.max(0, outputLimit - lengths[stream]);
      if (remaining > 0) (stream === 'stdout' ? stdout : stderr).push(chunk.subarray(0, remaining));
      lengths[stream] += chunk.length;
      if (lengths[stream] > outputLimit) stop('output_limit');
    }
    const timeout = setTimeout(() => stop('timeout'), timeoutMs);
    const cancel = () => stop('cancelled');
    signal?.addEventListener('abort', cancel, { once: true });
    child.stdout.on('data', chunk => capture('stdout', chunk));
    child.stderr.on('data', chunk => capture('stderr', chunk));
    child.once('error', () => { error ??= 'spawn_failed'; });
    // EPIPE can mean the CLI rejected argv before reading stdin. Preserve its actual result.
    child.stdin.on('error', () => {});
    child.once('close', exitCode => {
      clearTimeout(timeout);
      clearTimeout(killTimer);
      signal?.removeEventListener('abort', cancel);
      const result: CommandResult = { exitCode, stdout: Buffer.concat(stdout).toString('utf8'), stderr: Buffer.concat(stderr).toString('utf8') };
      resolve(error ? wrapperFailure(error, copy, result) : result);
    });
    child.stdin.end(invocation.stdin, 'utf8');
  });
}
