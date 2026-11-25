//! The first message of a process. Its first screen may be a prompt of its own (an update
//! notice) that ignores a paste and takes Enter for its default choice. A paste the process
//! answers with output went in; without any, what follows Enter tells whether it was taken.

use super::*;

/// The process may take this long to print anything before its first message.
const READY_WAIT: Duration = Duration::from_secs(10);
/// Nothing printed for this long: whatever was starting or updating has settled.
const QUIET: Duration = Duration::from_millis(1500);
/// How long after Enter a message that met no echo may still prove taken.
const WATCH: Duration = Duration::from_secs(30);

/// Why a first message did not go in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotTaken {
    /// The process ended by itself. A new one can have the message.
    Exited,
    /// The process runs on and showed no sign of it.
    NoEcho,
    /// The process was relaunched, switched to another shell or closed meanwhile.
    Replaced,
}

impl NotTaken {
    pub(crate) fn exited(error: &anyhow::Error) -> bool {
        error.downcast_ref::<Self>() == Some(&Self::Exited)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Exited => "exited",
            Self::NoEcho => "no-echo",
            Self::Replaced => "replaced",
        }
    }
}

impl std::fmt::Display for NotTaken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "first message not taken: {}", self.as_str())
    }
}

impl std::error::Error for NotTaken {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Launch {
    Running,
    Exited,
    Replaced,
}

impl Launch {
    fn running(self) -> Result<(), NotTaken> {
        match self {
            Self::Running => Ok(()),
            Self::Exited => Err(NotTaken::Exited),
            Self::Replaced => Err(NotTaken::Replaced),
        }
    }
}

/// What is known since the Enter that met no echo.
#[derive(Clone, Copy, Debug)]
struct Seen {
    launch: Launch,
    session_changed: bool,
    output: bool,
    quiet: bool,
    /// The terminal echoes input by itself: no screen of the process holds the keyboard, so
    /// output says nothing about the message (an updater runs like this).
    terminal_echoes: bool,
    repasted: bool,
    over: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Watch,
    Repaste,
    /// The session file has it.
    TakenBySession,
    /// The terminal never stopped working.
    TakenWorking,
    NotTaken(NotTaken),
}

fn judge(seen: &Seen) -> Verdict {
    if let Err(why) = seen.launch.running() {
        return Verdict::NotTaken(why);
    }
    if seen.session_changed {
        return Verdict::TakenBySession;
    }
    let on_screen = seen.output && !seen.terminal_echoes && !seen.repasted;
    match (on_screen, seen.quiet, seen.over) {
        (true, true, _) => Verdict::Repaste,
        (_, _, false) => Verdict::Watch,
        (true, false, true) => Verdict::TakenWorking,
        (false, _, true) => Verdict::NotTaken(NotTaken::NoEcho),
    }
}

struct Timing {
    ready: Duration,
    quiet: Duration,
    watch: Duration,
}

const TIMING: Timing = Timing {
    ready: READY_WAIT,
    quiet: QUIET,
    watch: WATCH,
};

/// The output epoch and when it last moved.
struct Output {
    epoch: u64,
    moved: Instant,
}

impl Output {
    fn at(epoch: u64) -> Self {
        Self {
            epoch,
            moved: Instant::now(),
        }
    }

    fn see(&mut self, epoch: u64) {
        if epoch > self.epoch {
            *self = Self::at(epoch);
        }
    }
}

impl AgentTerminalPty {
    /// The running process, if it has not had its first message.
    pub(super) fn awaits_first_submit(&self) -> Option<u64> {
        let launch = self.inner.generation.load(Ordering::SeqCst);
        (self.inner.first_submit.load(Ordering::SeqCst) != launch).then_some(launch)
    }

    /// Waits outside `Replacement::serial`, so every write checks that `launch` still runs.
    pub(super) fn submit_first(
        &self,
        app: &AppHandle,
        launch: u64,
        input: String,
        source: crate::shell_switch::PromptSource,
        soul: Option<crate::soul_push::input::Input>,
    ) -> Result<()> {
        let _submit_guard = self
            .inner
            .submit_lock
            .lock()
            .expect("agent submit lock poisoned");
        let started = Instant::now();
        let result = self
            .wait_until_ready(launch, &TIMING)
            .map_err(anyhow::Error::from)
            .and_then(|()| {
                let mut prepared = self.prepare_submit(app, input, source, soul)?;
                let soul = prepared.soul.take();
                self.deliver_first(launch, &prepared, &TIMING, || confirm_soul(app, soul))?;
                prepared.submitted();
                Ok(())
            });
        if let Some(why) = result
            .as_ref()
            .err()
            .and_then(|error| error.downcast_ref::<NotTaken>())
        {
            crate::kota_debug_log(&format!(
                "[agent:{}] submit prompt not taken after {}ms (first_message={})",
                self.inner.agent_id,
                started.elapsed().as_millis(),
                why.as_str(),
            ));
        }
        result
    }

    /// A process that has printed nothing yet is not listening yet.
    fn wait_until_ready(&self, launch: u64, timing: &Timing) -> Result<(), NotTaken> {
        let deadline = Instant::now() + timing.ready;
        let mut output = Output::at(self.output_epoch());
        if output.epoch > 0 {
            return self.launch_state(launch).running();
        }
        loop {
            self.launch_state(launch).running()?;
            output.see(self.output_epoch());
            let settled = output.epoch > 0 && output.moved.elapsed() >= timing.quiet;
            if settled || Instant::now() >= deadline {
                return Ok(());
            }
            thread::sleep(AGENT_PROMPT_NATIVE_CONFIRM_POLL);
        }
    }

    fn deliver_first(
        &self,
        launch: u64,
        prepared: &PreparedSubmit,
        timing: &Timing,
        delivered: impl FnOnce(),
    ) -> Result<()> {
        let write = |text: &str| self.write_to_launch(launch, text);
        let started = Instant::now();
        self.inner.first_submit.store(launch, Ordering::SeqCst);
        let echoed = self.paste_echoed(&prepared.input, &write)?;
        if echoed.is_some() {
            return self.finish_submit(
                started,
                echoed,
                prepared,
                &write,
                delivered,
                Some("echoed"),
            );
        }
        // Enter once, as ever: a prompt takes its default. What follows tells.
        let baseline = self.output_epoch();
        write(prompt_submit_sequence(self.inner.cli))?;
        let entered = Instant::now();
        let mut output = Output::at(baseline);
        let mut repasted = false;
        loop {
            output.see(self.output_epoch());
            let verdict = judge(&Seen {
                launch: self.launch_state(launch),
                session_changed: prepared
                    .native_before
                    .as_ref()
                    .is_some_and(NativeSessionMarker::changed),
                output: output.epoch > baseline,
                quiet: output.moved.elapsed() >= timing.quiet,
                terminal_echoes: self.terminal_echoes(),
                repasted,
                over: entered.elapsed() >= timing.watch,
            });
            let (how, confirm) = match verdict {
                Verdict::Watch => {
                    thread::sleep(AGENT_PROMPT_NATIVE_CONFIRM_POLL);
                    continue;
                }
                Verdict::Repaste => {
                    repasted = true;
                    let started = Instant::now();
                    let echoed = self.paste_echoed(&prepared.input, &write)?;
                    if echoed.is_some() {
                        return self.finish_submit(
                            started,
                            echoed,
                            prepared,
                            &write,
                            delivered,
                            Some("repasted"),
                        );
                    }
                    continue;
                }
                Verdict::TakenBySession => ("session", SubmitConfirmation::NativeLog),
                Verdict::TakenWorking => ("working", SubmitConfirmation::Output),
                Verdict::NotTaken(why) => return Err(why.into()),
            };
            delivered();
            let elapsed = started.elapsed().as_millis();
            crate::kota_debug_log(&format!(
                "[agent:{}] submit prompt enter sent after {elapsed}ms (first-output-timeout, clear_after_interrupt={}, first_ack_ms=-, quiet_ms=-, enter_attempts=1, submit_confirm={}, submit_confirm_ms={elapsed}, first_message={how})",
                self.inner.agent_id, prepared.clear_after_interrupt, confirm.as_str(),
            ));
            return Ok(());
        }
    }

    /// The epoch of the output that answered the paste.
    fn paste_echoed(
        &self,
        input: &str,
        write: &impl Fn(&str) -> Result<()>,
    ) -> Result<Option<u64>> {
        let baseline = self.output_epoch();
        write(input)?;
        Ok(self.wait_for_output_after(baseline, AGENT_PROMPT_FIRST_OUTPUT_WAIT))
    }

    fn write_to_launch(&self, launch: u64, input: &str) -> Result<()> {
        self.write_if(input, |process| {
            Ok(self.launch_of(launch, process).running()?)
        })
        // A terminal that cannot be written has lost its process.
        .map_err(|error| {
            if error.is::<NotTaken>() {
                error
            } else {
                NotTaken::Exited.into()
            }
        })
    }

    fn launch_state(&self, launch: u64) -> Launch {
        let process = self.inner.process.lock().expect("agent process poisoned");
        self.launch_of(launch, process.as_ref())
    }

    fn launch_of(&self, launch: u64, process: Option<&AgentProcess>) -> Launch {
        if self.inner.generation.load(Ordering::SeqCst) != launch || self.inner.inputs.is_closed() {
            Launch::Replaced
        } else if process
            .is_none_or(|process| process.child_pid.is_some_and(|pid| !process_is_alive(pid)))
        {
            Launch::Exited
        } else {
            Launch::Running
        }
    }

    /// Unknown counts as yes: no second paste, and no claim that it was taken.
    #[cfg(unix)]
    fn terminal_echoes(&self) -> bool {
        let process = self.inner.process.lock().expect("agent process poisoned");
        process
            .as_ref()
            .and_then(|process| process.master.get_termios())
            .is_none_or(|modes| modes.local_flags.bits() & libc::ECHO != 0)
    }

    #[cfg(not(unix))]
    fn terminal_echoes(&self) -> bool {
        true
    }
}

#[cfg(all(test, unix))]
#[path = "first_submit_tests.rs"]
mod tests;
