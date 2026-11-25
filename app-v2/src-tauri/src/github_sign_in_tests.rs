use super::*;
use crate::github_accounts::AccountsView;
use std::time::Instant;

/// A shell script in place of gh; no GitHub and no network.
fn script(source: &str) -> Child {
    spawn(Command::new("/bin/sh").args(["-c", source])).unwrap()
}

const ID: &str = "first";

fn result() -> SignInResult {
    SignInResult {
        login: "fable-indigo".into(),
        added: true,
        active: Some("fable-copper".into()),
        restore_failed: false,
        view: AccountsView { default_login: Some("fable-copper".into()), accounts: vec![] },
    }
}

struct Run {
    manager: Manager,
    pid: u32,
    events: mpsc::Receiver<Event>,
}

impl Run {
    fn start(source: &str, limit: Duration) -> Self {
        let (manager, (sender, events)) = (Manager::default(), mpsc::channel());
        let child = script(source);
        let pid = child.id();
        manager.start(ID.into(), child, limit, move |event| drop(sender.send(event)), || Ok(result()));
        Self { manager, pid, events }
    }
    fn next(&self) -> Event {
        self.events.recv_timeout(Duration::from_secs(10)).expect("an event")
    }
    /// gh's output arrives in pieces: everything up to and including the piece that has `text`.
    fn output_with(&self, text: &str) -> (String, Duration) {
        loop {
            match self.next() {
                Event::Output { id, output, deadline_ms } if output.contains(text) => {
                    assert_eq!(id, ID);
                    return (output, Duration::from_millis(deadline_ms));
                }
                Event::Output { .. } => {}
                end => panic!("{end:?}"),
            }
        }
    }
    /// Skips the output on the way to what ended the sign-in. The sender is dropped with the
    /// watching thread, so this also proves that nothing follows.
    fn end(&self) -> Event {
        let mut last = None;
        while let Ok(event) = self.events.recv_timeout(Duration::from_secs(10)) {
            last = Some(event);
        }
        // Stopped or not, gh has been waited for: a zombie would still be listed.
        let listed = Command::new("ps").args(["-o", "pid=", "-p", &self.pid.to_string()]).output().unwrap();
        assert!(listed.stdout.is_empty(), "gh is still there");
        last.expect("an end")
    }
}

const CODE: &str = "printf '\\n! First copy your one-time code: WDJB-MJHT\\nOpen this URL to continue in your web browser: https://github.com/login/device\\n' >&2";

#[test]
fn what_gh_prints_is_passed_on_unread_and_its_exit_ends_the_sign_in() {
    let run = Run::start(&format!("{CODE}; sleep 0.3; echo '✓ Logged in as fable-indigo' >&2"), LIMIT);
    let (output, deadline) = run.output_with("login/device\n");
    assert!(output.contains("one-time code: WDJB-MJHT"), "{output}");
    let left = deadline.saturating_sub(SystemTime::now().duration_since(UNIX_EPOCH).unwrap());
    assert!((Duration::from_secs(590)..=LIMIT).contains(&left), "{left:?}");
    assert_eq!(run.end(), Event::SignedIn { id: ID.into(), result: result() });
}

#[test]
fn a_gh_that_stops_by_itself_fails_with_everything_it_printed() {
    let run = Run::start(&format!("{CODE}; sleep 0.2; echo 'failed to authenticate via web browser: HTTP 503' >&2; exit 1"), LIMIT);
    let Event::Failed { output, .. } = run.end() else { panic!("failed") };
    assert!(output.contains("WDJB-MJHT") && output.ends_with("HTTP 503\n"), "{output}");
    let silent = Run::start("exit 1", LIMIT);
    assert_eq!(silent.end(), Event::Failed { id: ID.into(), output: String::new() });
}

#[test]
fn the_limit_counts_from_the_first_output_and_stops_gh() {
    // Nothing printed yet: still fetching the code, so the limit has not begun.
    let fetching = Run::start("sleep 0.6; echo code >&2; exec sleep 30", Duration::from_millis(400));
    assert!(matches!(fetching.next(), Event::Output { .. }));
    let started = Instant::now();
    assert_eq!(fetching.end(), Event::TimedOut { id: ID.into() });
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn cancelling_stops_gh_and_ends_the_sign_in() {
    let run = Run::start(&format!("{CODE}; exec sleep 30"), LIMIT);
    run.next();
    run.manager.cancel("another"); // changes nothing
    while let Ok(event) = run.events.recv_timeout(Duration::from_millis(300)) {
        assert!(matches!(event, Event::Output { .. }), "{event:?}");
    }
    run.manager.cancel(ID);
    assert_eq!(run.end(), Event::Cancelled { id: ID.into() });
}

#[test]
fn starting_again_stops_the_sign_in_before() {
    let first = Run::start(&format!("{CODE}; exec sleep 30"), LIMIT);
    first.next();
    let (sender, events) = mpsc::channel();
    first.manager.start("second".into(), script("exit 0"), LIMIT, move |event| drop(sender.send(event)), || Ok(result()));
    assert_eq!(first.end(), Event::Cancelled { id: ID.into() });
    assert_eq!(events.recv_timeout(Duration::from_secs(10)).unwrap(), Event::SignedIn { id: "second".into(), result: result() });
}

#[test]
fn a_gh_that_finished_before_it_was_stopped_still_counts() {
    // The exit status is already there when the stop arrives.
    let mut child = script("exit 0");
    child.wait().unwrap();
    let (sender, events) = mpsc::channel();
    watch(ID.into(), child, LIMIT, &AtomicBool::new(true), move |event| drop(sender.send(event)), || Ok(result()));
    assert_eq!(events.recv().unwrap(), Event::SignedIn { id: ID.into(), result: result() });
}

#[test]
fn a_process_gh_left_behind_does_not_hold_up_the_end() {
    let run = Run::start("sleep 20 & echo code >&2", LIMIT);
    let started = Instant::now();
    assert_eq!(run.end(), Event::SignedIn { id: ID.into(), result: result() });
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn an_account_kota_cannot_record_is_a_failure_that_keeps_both_reasons() {
    let (sender, events) = mpsc::channel();
    watch(ID.into(), script("echo done >&2"), LIMIT, &AtomicBool::new(false), move |event| drop(sender.send(event)), || bail!("write GitHub accounts"));
    let Some(Event::Failed { output, .. }) = events.iter().last() else { panic!("failed") };
    assert_eq!(output, "done\n\nwrite GitHub accounts");
}

#[test]
fn events_reach_the_dialog_in_its_own_shape() {
    assert_eq!(
        serde_json::to_value(Event::Output { id: ID.into(), output: "x".into(), deadline_ms: 5 }).unwrap(),
        serde_json::json!({"kind": "output", "id": ID, "output": "x", "deadlineMs": 5})
    );
    let signed_in = serde_json::to_value(Event::SignedIn { id: ID.into(), result: result() }).unwrap();
    assert_eq!((&signed_in["kind"], &signed_in["result"]["login"], &signed_in["result"]["restoreFailed"]), (&"signed_in".into(), &"fable-indigo".into(), &false.into()));
    assert_eq!(serde_json::to_value(Event::TimedOut { id: ID.into() }).unwrap(), serde_json::json!({"kind": "timed_out", "id": ID}));
}
