import { describe, expect, it } from 'vitest';
import { plainOutput, readCode, readReason } from '../src/lib/gh-output';
// What gh 2.83.2 printed without a terminal, against a fake GitHub on this machine (null: still waiting).
import measured from './fixtures/gh-sign-in-output.json';

const prefix = 'failed to authenticate via web browser: ';
const code = { code: 'WDJB-MJHT', line: '! First copy your one-time code: WDJB-MJHT' };

describe('what the GitHub sign-in reads out of gh', () => {
  it('finds the code in every measured output that has one, colour codes or not', () => {
    const found = Object.entries(measured).map(([name, { output }]) => [name, readCode(output)]);
    const without = ['no-connection', 'server-error', 'rate-limited', 'no-such-host'];
    expect(found).toHaveLength(15);
    for (const [name, value] of found) expect(value, String(name)).toEqual(without.includes(name as string) ? null : code);
    expect(plainOutput(measured.color.output)).toBe(plainOutput(measured.waiting.output));
  });

  it.each([
    ['declined', `${prefix}The authorization request was denied. (access_denied)`],
    ['expired', `${prefix}The device_code has expired. (expired_token)`],
    ['local-expiry', `${prefix}context deadline exceeded`],
    ['slow-down', `${prefix}slow_down`],
    ['drop-once', `${prefix}Post "http://github.localhost/login/oauth/access_token": EOF`],
    ['drop-while-waiting', `${prefix}Post "http://github.localhost/login/oauth/access_token": EOF`],
    ['unavailable-while-waiting', `${prefix}HTTP 503`],
    ['server-error', `${prefix}HTTP 503`],
    ['rate-limited', `${prefix}Too many requests (slow_down)`],
    // The address stands in the reason itself: nothing is left after it, so everything is shown.
    ['no-connection', `${prefix}Post "http://github.localhost/login/device/code": EOF`],
    ['no-such-host', 'error connecting to github.invalid\ncheck your internet connection or https://githubstatus.com'],
    ['cannot-store', '✓ Authentication complete.\n- gh config set -h github.localhost git_protocol https\n✓ Configured git protocol\nopen /fixture/config/hosts.yml: permission denied'],
  ] as const)('gives gh’s own reason without the dead code: %s', (name, reason) => {
    expect(measured[name].exit).toBe(1);
    expect(readReason(measured[name].output)).toBe(reason);
  });

  it('goes by shape, not by wording', () => {
    expect(readCode('Your code is ABCD-1234, enter it on GitHub\n')).toEqual({ code: 'ABCD-1234', line: 'Your code is ABCD-1234, enter it on GitHub' });
    expect(readCode('(ABCD-1234)\nagain: ABCD-1234.\n')).toEqual({ code: 'ABCD-1234', line: '(ABCD-1234)' });
  });

  it('does not guess: no code, two codes, or something that only looks like one', () => {
    for (const text of ['', 'Enter WDJBMJHT at https://github.com/login/device\n', 'ABCD-1234 EFGH-5678\n', 'ABCD-1234 since 2025-1110\n',
      'request 0A1B2C3D-0000-1111-2222-0A1B2C3D4E5F\n', 'code wdjb-mjht\n']) expect(readCode(text), text).toBeNull();
    expect(readReason('')).toBe('');
  });

  it('parses on macOS 13.0, whose WebKit has no lookbehind', async () => {
    const { readFile } = await import('node:fs/promises');
    expect(await readFile('src/lib/gh-output.ts', 'utf8')).not.toMatch(/\(\?<[!=]/);
  });
});
