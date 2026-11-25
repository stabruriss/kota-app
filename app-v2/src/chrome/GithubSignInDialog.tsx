import { useEffect, useRef, useState, type ReactNode } from 'react';
import { openExternalUrl } from '../pty-client';
import {
  closeGithubSignIn, continueGithubSignIn, installGithubCli, retryGithubSignIn, useGithubAccounts, type GithubSignIn,
} from '../lib/github-account-flow';
import { plainOutput, readCode, readReason } from '../lib/gh-output';
import { LocalAuthLayer } from './KotaLocalSignInDialog';
import githubMark from '../assets/tavern/icons/providers/github.svg';
import '../styles/kota-local-link.css';
import '../styles/github-sign-in.css';

const DEVICE_ADDRESS = 'github.com/login/device';
/** github_sign_in::LIMIT. */
const MINUTES = 10;

type Tone = 'success' | 'failed' | 'neutral';
const RESULT_ICON: Record<Tone, string> = { success: 'M8 12.4 10.6 15 16.4 9', failed: 'M12 7.5v5.5M12 16.2v.3', neutral: 'M8.5 12h7' };
interface Result { tone: Tone; title: string; body?: string; said?: string }

function resultOf(signIn: GithubSignIn): Result | null {
  switch (signIn.phase) {
    case 'cli_missing': return { tone: 'neutral', title: 'GitHub CLI is not installed', body: 'Kota keeps GitHub accounts in GitHub CLI. Install it, then check again.' };
    case 'timed_out': return { tone: 'failed', title: 'Sign-in did not finish', body: `${MINUTES} minutes passed. Try again for a new code.` };
    // Whatever else stopped it, GitHub CLI says so in its own words.
    case 'failed': return { tone: 'failed', title: 'Sign-in did not finish', said: readReason(signIn.output) || 'GitHub CLI stopped without saying why.' };
    case 'done': {
      const { login, added, active, restoreFailed, view } = signIn.result;
      const notice = restoreFailed && `The current gh account is now ${active}.`;
      const text = (...parts: (string | false)[]) => parts.filter(Boolean).join(' ') || undefined;
      switch (signIn.outcome) {
        case 'added': return { tone: 'success', title: `${login} added`, body: text(view.defaultLogin === login && 'It is the default for new projects.', notice) };
        case 'signed_in': return { tone: 'success', title: `${login} is signed in`, body: text(notice) };
        case 'already': return { tone: 'neutral', title: `${login} is already in Kota`, body: text(notice) };
        case 'different': return { tone: 'neutral', title: `You signed in as ${login}`, body: text(added && `${login} was added.`, `${signIn.expect} still needs to sign in.`, notice) };
      }
    }
  }
  return null;
}

function Countdown({ deadline }: { deadline: number }) {
  const left = () => Math.max(0, Math.ceil((deadline - Date.now()) / 1000));
  const [seconds, setSeconds] = useState(left);
  useEffect(() => {
    setSeconds(left());
    const timer = window.setInterval(() => setSeconds(left()), 1000);
    return () => window.clearInterval(timer);
  }, [deadline]);
  return <div className={`local-auth-device-expiry${seconds <= 60 ? ' ending' : ''}`} role="timer">
    Expires in {Math.floor(seconds / 60)}:{String(seconds % 60).padStart(2, '0')}
  </div>;
}

function Step({ number, current, title, children }: { number: 1 | 2; current: boolean; title: string; children: ReactNode }) {
  return <li className="github-step" aria-current={current ? 'step' : undefined}>
    <div className="github-step-title"><span>Step {number}:</span> {title}</div>
    {current && children}
  </li>;
}

function SignInSteps({ expect, output, deadline }: { expect: string | null; output: string | null; deadline: number | null }) {
  // gh starts with an empty line: nothing is there to show until it has said something.
  const said = plainOutput(output ?? ''), found = readCode(said);
  const [copied, setCopied] = useState(false);
  const steps = useRef<HTMLOListElement>(null), primary = useRef<HTMLButtonElement>(null);
  // While the code is on its way there is nothing to press: the dialog itself keeps the focus.
  useEffect(() => {
    (primary.current ?? steps.current!.closest<HTMLElement>('[role=dialog]')!).focus({ preventScroll: true });
  }, [output === null, !said]);
  const copyAndOpen = () => {
    // The label only says Copied once the clipboard has it; the code can be selected either way.
    if (found) void navigator.clipboard.writeText(found.code).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1600); });
    void openExternalUrl(`https://${DEVICE_ADDRESS}`);
  };
  return <ol ref={steps} className="github-steps">
    <Step number={1} current={output === null} title={expect ? `In your browser, sign in to GitHub as ${expect}` : 'In your browser, sign in to the GitHub account you want to add'}>
      <div className="github-step-actions">
        <button type="button" className="github-open-link" onClick={() => void openExternalUrl('https://github.com/login')}>
          <img src={githubMark} alt="" /><span>Open GitHub</span>
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6 3.5H3.5v9h9V10M9 3.5h3.5V7M12.2 3.8 7.2 8.8" /></svg>
        </button>
        <button ref={primary} type="button" className="local-auth-button primary" onClick={() => void continueGithubSignIn()}>Signed in, next</button>
      </div>
    </Step>
    <Step number={2} current={output !== null} title="Paste the code">
      {!said ? <div className="local-auth-progress" role="status"><span className="local-auth-spinner" aria-hidden="true" /><span>Getting a code from GitHub…</span></div> : <>
        <div className="local-auth-step local-auth-device">
          {found ? <>
            <div className="github-gh-line">{found.line.split(found.code).flatMap((part, index) => index ? [<span key={index}>{found.code}</span>, part] : [part])}</div>
            <div className="local-auth-device-code">{found.code}</div>
          </> : <pre className="github-gh-output">{said}</pre>}
          <div className="local-auth-step-actions">
            <button ref={primary} type="button" className="local-auth-button primary" onClick={copyAndOpen}>{!found ? 'Open GitHub' : copied ? 'Copied' : 'Copy code and open GitHub'}</button>
          </div>
          <div className="github-device-address">{DEVICE_ADDRESS}</div>
          <Countdown deadline={deadline!} />
        </div>
        <div className="local-auth-progress" role="status"><span className="local-auth-spinner" aria-hidden="true" /><span>Waiting for you to confirm in the browser…</span></div>
      </>}
    </Step>
  </ol>;
}

/** Mounted once in App: every entry point opens this dialog. */
export function GithubSignInDialog() {
  const { signIn } = useGithubAccounts(false);
  return signIn && <OpenDialog signIn={signIn} />;
}

function OpenDialog({ signIn }: { signIn: GithubSignIn }) {
  const card = useRef<HTMLDivElement>(null);
  // WebKit's own Tab skips buttons and lands in the fields of whatever is behind this dialog.
  useEffect(() => {
    const keepInside = (event: KeyboardEvent) => {
      if (event.key !== 'Tab') return;
      event.preventDefault(); event.stopPropagation();
      const buttons = Array.from(card.current!.querySelectorAll('button'));
      const step = event.shiftKey ? -1 : 1, at = buttons.indexOf(document.activeElement as HTMLButtonElement);
      buttons[at < 0 ? (event.shiftKey ? buttons.length - 1 : 0) : (at + step + buttons.length) % buttons.length].focus();
    };
    window.addEventListener('keydown', keepInside, true);
    return () => window.removeEventListener('keydown', keepInside, true);
  }, []);
  const result = resultOf(signIn);
  return <LocalAuthLayer onEscape={closeGithubSignIn}>
    <div ref={card} className="kota-confirm-card local-auth-dialog github-sign-in-dialog" role="dialog" aria-modal="true" aria-label="Sign in to GitHub" tabIndex={-1}>
      <div className="local-auth-dialog-head">
        <span className="local-auth-dialog-logo" aria-hidden="true"><img src={githubMark} alt="" /></span>
        <div><h2>Sign in · GitHub</h2><div className="local-auth-dialog-sub">{signIn.expect ?? 'Add an account'}</div></div>
      </div>
      <div className="local-auth-body">
        {signIn.phase === 'account' ? <SignInSteps expect={signIn.expect} output={null} deadline={null} />
          : signIn.phase === 'running' ? <SignInSteps expect={signIn.expect} output={signIn.output} deadline={signIn.deadline} />
            : result && <div className={`local-auth-result ${result.tone}`} role={result.tone === 'failed' ? 'alert' : 'status'}>
              <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d={RESULT_ICON[result.tone]} /></svg>
              <div><b>{result.title}</b>{result.body && <p>{result.body}</p>}{result.said && <pre className="github-gh-output">{result.said}</pre>}</div>
            </div>}
      </div>
      <div className="kota-confirm-actions">
        {!result ? <button type="button" onClick={closeGithubSignIn}>Cancel</button>
          : result.tone === 'success' ? <button type="button" className="confirm" onClick={closeGithubSignIn}>Done</button>
            : <>
              <button type="button" onClick={closeGithubSignIn}>Close</button>
              <button type="button" className={signIn.phase === 'cli_missing' ? undefined : 'confirm'} onClick={retryGithubSignIn}>{signIn.phase === 'cli_missing' ? 'Check again' : 'Try again'}</button>
              {signIn.phase === 'cli_missing' && <button type="button" className="confirm" onClick={() => void installGithubCli()}>Install GitHub CLI</button>}
            </>}
      </div>
    </div>
  </LocalAuthLayer>;
}
