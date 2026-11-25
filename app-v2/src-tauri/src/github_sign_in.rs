//! Signing in to GitHub inside Kota: `gh auth login` without a terminal.
//!
//! gh asks nothing then. It prints the one-time code and the address, polls GitHub and exits
//! by itself. Kota passes on what gh printed, unread; the dialog finds the code in it. Who
//! signed in is gh's active account afterwards, never its words.
//!
//! Nothing cleans up when Kota quits: gh loses its pipes, so it dies on its next line, which
//! comes before it stores an account.

use std::io::{ErrorKind, Read};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::github_accounts::{gh_command, SignInResult, GH_CANDIDATES, HOST};

/// From the moment gh prints (老无; GitHub's own codes last 15 minutes). gh is stopped at the
/// end of it, so the countdown never says a code is over that still works.
pub const LIMIT: Duration = Duration::from_secs(10 * 60);
const TICK: Duration = Duration::from_millis(100);
/// gh adds `repo`, `read:org` and `gist`. `workflow` is what it adds in a terminal when the
/// person lets it set up git, and what a push that changes `.github/workflows` needs.
const SCOPES: &str = "repo,read:user,user:email,workflow";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Event {
    /// Everything gh has printed so far.
    Output { id: String, output: String, deadline_ms: u64 },
    // Every sign-in ends with exactly one of these.
    SignedIn { id: String, result: SignInResult },
    /// gh stopped by itself; its output says why.
    Failed { id: String, output: String },
    TimedOut { id: String },
    Cancelled { id: String },
}

#[derive(Default)]
pub struct Manager(Mutex<Current>);

#[derive(Default)]
struct Current {
    id: String,
    stop: Arc<AtomicBool>,
}

impl Manager {
    /// One sign-in at a time: starting another, or cancelling, stops the one before. The
    /// dialog chooses the id, so it can listen before the first event.
    pub fn start(
        &self,
        id: String,
        child: Child,
        limit: Duration,
        emit: impl Fn(Event) + Send + 'static,
        finish: impl FnOnce() -> Result<SignInResult> + Send + 'static,
    ) {
        let mut current = self.0.lock().unwrap();
        current.stop.store(true, Ordering::SeqCst);
        *current = Current { id: id.clone(), stop: Arc::default() };
        let stop = current.stop.clone();
        thread::spawn(move || watch(id, child, limit, &stop, emit, finish));
    }

    pub fn cancel(&self, id: &str) {
        let current = self.0.lock().unwrap();
        if current.id == id {
            current.stop.store(true, Ordering::SeqCst);
        }
    }
}

pub fn spawn_gh() -> Result<Child> {
    for candidate in GH_CANDIDATES {
        let mut command = gh_command(candidate);
        command.args(["auth", "login", "--hostname", HOST, "--git-protocol", "https", "--web", "--scopes", SCOPES]);
        match spawn(&mut command) {
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            child => return child.context("start gh"),
        }
    }
    bail!("github_cli_missing")
}

fn spawn(command: &mut Command) -> std::io::Result<Child> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
}

fn watch(id: String, mut child: Child, limit: Duration, stop: &AtomicBool, emit: impl Fn(Event), finish: impl FnOnce() -> Result<SignInResult>) {
    let (sender, chunks) = mpsc::channel();
    read_into(child.stdout.take(), sender.clone());
    read_into(child.stderr.take(), sender);
    let mut output = Vec::new();
    // Wall-clock time, like GitHub's own limit: a sleeping Mac does not stop it.
    let mut deadline: Option<SystemTime> = None;
    let mut timed_out = false;
    let text = |output: &[u8]| String::from_utf8_lossy(output).into_owned();
    loop {
        match chunks.recv_timeout(TICK) {
            Ok(chunk) => {
                output.extend::<Vec<u8>>(chunk);
                let deadline = deadline.get_or_insert_with(|| SystemTime::now() + limit);
                let deadline_ms = deadline.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                if !stop.load(Ordering::SeqCst) {
                    emit(Event::Output { id: id.clone(), output: text(&output), deadline_ms });
                }
            }
            // Gone with nothing left to read, even if something it started still holds a pipe.
            Err(mpsc::RecvTimeoutError::Timeout) if matches!(child.try_wait(), Ok(Some(_))) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Both pipes closed: gh is gone or about to be.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        timed_out = deadline.is_some_and(|deadline| SystemTime::now() >= deadline);
        if timed_out || stop.load(Ordering::SeqCst) {
            let _ = child.kill();
            break;
        }
    }
    // gh may have finished just before it was stopped. Then the account is in gh, so it is
    // recorded and gh is switched back like any other sign-in.
    let signed_in = child.wait().is_ok_and(|status| status.success());
    emit(match (signed_in, timed_out) {
        (true, _) => match finish() {
            Ok(result) => Event::SignedIn { id, result },
            Err(error) => Event::Failed { id, output: format!("{}\n{error:#}", text(&output)) },
        },
        (false, true) => Event::TimedOut { id },
        _ if stop.load(Ordering::SeqCst) => Event::Cancelled { id },
        _ => Event::Failed { id, output: text(&output) },
    });
}

fn read_into(pipe: Option<impl Read + Send + 'static>, sender: mpsc::Sender<Vec<u8>>) {
    let Some(mut pipe) = pipe else { return };
    thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(read @ 1..) = pipe.read(&mut buffer) {
            if sender.send(buffer[..read].to_vec()).is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
#[path = "github_sign_in_tests.rs"]
mod tests;
