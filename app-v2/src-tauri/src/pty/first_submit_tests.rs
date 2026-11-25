use super::*;
use std::sync::atomic::AtomicUsize;

const FAST: Timing = Timing {
    ready: Duration::from_secs(5),
    quiet: Duration::from_millis(300),
    watch: Duration::from_secs(6),
};
/// One read in raw mode: a paste, or the Enter after it.
const SWALLOW: &str = "dd bs=4096 count=1 >/dev/null 2>&1;";

struct Fake {
    pty: AgentTerminalPty,
    dir: PathBuf,
    /// Everything on its screen, the terminal's own echo included.
    screen: Arc<Mutex<Vec<u8>>>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.pty.stop_current();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn request(dir: &Path) -> AgentSpawnRequest {
    AgentSpawnRequest {
        agent_id: "agent-cedar-first".into(),
        cli: AgentCli::Codex,
        cwd: dir.display().to_string(),
        project_root: dir.display().to_string(),
        worktree_root: None,
        shared_dir: Some(dir.join("project-memory").display().to_string()),
        rules_dir: None,
        adapter_path: None,
        args: Vec::new(),
        session_id: None,
        project_id: Some("cedar-project".into()),
        project_remote: None,
        project_base_ref: None,
        takeover: false,
        fresh_session: false,
    }
}

/// What `spawn` does to the state, with a script in place of the CLI. `$1` is the fake's folder.
fn launch(pty: &AgentTerminalPty, dir: &Path, script: &str) -> Arc<Mutex<Vec<u8>>> {
    let size = *pty.inner.size.lock().unwrap();
    let pair = native_pty_system().openpty(size).unwrap();
    let mut command = CommandBuilder::new("/bin/bash");
    command.args(["-c", script, "fake"]);
    command.arg(dir);
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let process = AgentProcess {
        writer: Arc::new(Mutex::new(pair.master.take_writer().unwrap())),
        killer: child.clone_killer(),
        decoder: Arc::new(Mutex::new(AnsiLineDecoder::new(size.cols, size.rows))),
        child_pid: child.process_id(),
        master: pair.master,
    };
    let generation = pty.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
    pty.inner.inputs.release_start();
    *pty.inner.process.lock().unwrap() = Some(process);
    let screen = Arc::new(Mutex::new(Vec::new()));
    let (state, shown) = (Arc::clone(&pty.inner), Arc::clone(&screen));
    thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(read @ 1..) = reader.read(&mut buffer) {
            shown.lock().unwrap().extend_from_slice(&buffer[..read]);
            *state.output_signal.epoch.lock().unwrap() += 1;
            state.output_signal.changed.notify_all();
        }
    });
    let state = Arc::clone(&pty.inner);
    thread::spawn(move || {
        let _ = child.wait();
        if state.generation.load(Ordering::SeqCst) == generation {
            state.process.lock().unwrap().take();
        }
    });
    screen
}

fn fake(script: &str) -> Fake {
    let dir = std::env::temp_dir().join(format!(
        "kota-first-submit-{}",
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&dir).unwrap();
    let pty = AgentTerminalPty::new(request(&dir)).unwrap();
    let screen = launch(&pty, &dir, script);
    Fake { pty, dir, screen }
}

fn message() -> PreparedSubmit {
    PreparedSubmit {
        input: "\u{1b}[200~hello from the bus\u{1b}[201~".into(),
        metadata_cwd: PathBuf::new(),
        handoff: None,
        soul: None,
        native_before: None,
        clear_after_interrupt: false,
    }
}

impl Fake {
    /// The outcome, and how often the message was reported delivered.
    fn deliver(&self, message: &PreparedSubmit, timing: &Timing) -> (Result<()>, usize) {
        let launch = self.pty.awaits_first_submit().expect("a first message");
        let delivered = AtomicUsize::new(0);
        let result = self
            .pty
            .wait_until_ready(launch, timing)
            .map_err(anyhow::Error::from)
            .and_then(|()| {
                self.pty.deliver_first(launch, message, timing, || {
                    delivered.fetch_add(1, Ordering::SeqCst);
                })
            });
        (result, delivered.into_inner())
    }

    fn shown(&self) -> String {
        String::from_utf8_lossy(&self.screen.lock().unwrap()).into_owned()
    }

    fn file(&self, name: &str) -> String {
        fs::read_to_string(self.dir.join(name)).unwrap_or_default()
    }
}

fn why(result: Result<()>) -> NotTaken {
    *result
        .unwrap_err()
        .downcast_ref::<NotTaken>()
        .expect("a first message that was not taken")
}

#[test]
fn every_branch_after_an_enter_without_echo() {
    let seen = Seen {
        launch: Launch::Running,
        session_changed: false,
        output: false,
        quiet: false,
        terminal_echoes: false,
        repasted: false,
        over: false,
    };
    let exited = Seen {
        launch: Launch::Exited,
        session_changed: true,
        output: true,
        quiet: true,
        over: true,
        ..seen
    };
    let cases = [
        (seen, Verdict::Watch),
        (exited, Verdict::NotTaken(NotTaken::Exited)),
        (
            Seen {
                launch: Launch::Replaced,
                ..exited
            },
            Verdict::NotTaken(NotTaken::Replaced),
        ),
        (
            Seen {
                session_changed: true,
                ..seen
            },
            Verdict::TakenBySession,
        ),
        (
            Seen {
                session_changed: true,
                repasted: true,
                over: true,
                ..seen
            },
            Verdict::TakenBySession,
        ),
        // Nothing printed yet is not a settled screen.
        (
            Seen {
                quiet: true,
                ..seen
            },
            Verdict::Watch,
        ),
        (
            Seen {
                output: true,
                ..seen
            },
            Verdict::Watch,
        ),
        (
            Seen {
                output: true,
                quiet: true,
                ..seen
            },
            Verdict::Repaste,
        ),
        (
            Seen {
                output: true,
                quiet: true,
                over: true,
                ..seen
            },
            Verdict::Repaste,
        ),
        (
            Seen {
                output: true,
                quiet: true,
                repasted: true,
                ..seen
            },
            Verdict::Watch,
        ),
        // The terminal answers for itself: never paste into it, never count its output.
        (
            Seen {
                output: true,
                quiet: true,
                terminal_echoes: true,
                ..seen
            },
            Verdict::Watch,
        ),
        (
            Seen {
                output: true,
                over: true,
                ..seen
            },
            Verdict::TakenWorking,
        ),
        (
            Seen {
                output: true,
                terminal_echoes: true,
                over: true,
                ..seen
            },
            Verdict::NotTaken(NotTaken::NoEcho),
        ),
        (
            Seen { over: true, ..seen },
            Verdict::NotTaken(NotTaken::NoEcho),
        ),
        (
            Seen {
                output: true,
                repasted: true,
                over: true,
                ..seen
            },
            Verdict::NotTaken(NotTaken::NoEcho),
        ),
    ];
    for (seen, verdict) in cases {
        assert_eq!(judge(&seen), verdict, "{seen:?}");
    }
}

#[test]
fn an_echoed_paste_is_entered_and_delivered_once() {
    let fake = fake(INPUT_BOX);
    let (result, delivered) = fake.deliver(&message(), &FAST);
    result.unwrap();
    assert_eq!(delivered, 1);
    assert_eq!(fake.file("paste"), message().input);
    assert_eq!(fake.file("enter"), "\r");
    assert_eq!(
        fake.pty.awaits_first_submit(),
        None,
        "the next message takes the ordinary path"
    );
}

/// Enter leaves the prompt, the terminal echoes again, and the updater is silent for longer than `quiet`.
fn updater() -> String {
    format!("stty raw -echo; printf 'Update available'; {SWALLOW} {SWALLOW} stty sane; echo Updating; sleep 2")
}

const INPUT_BOX: &str =
    "stty raw -echo; printf ready; dd bs=4096 count=1 2>/dev/null | tee \"$1/paste\"; \
    dd bs=4096 count=1 2>/dev/null > \"$1/enter\"; printf working; sleep 5";

#[test]
fn an_update_that_swallows_the_paste_and_exits_is_not_a_delivery() {
    let fake = fake(&updater());
    let (result, delivered) = fake.deliver(&message(), &FAST);
    assert_eq!(why(result), NotTaken::Exited);
    assert_eq!(delivered, 0);
    let shown = fake.shown();
    assert!(shown.contains("Updating"), "{shown:?}");
    assert!(
        !shown.contains("hello from the bus"),
        "a second paste would be echoed here: {shown:?}"
    );
}

#[test]
fn the_message_goes_to_the_process_launched_after_the_update() {
    let registry = AgentRegistry::default();
    let updating = fake(&updater());
    let delivered = AtomicUsize::new(0);
    let launches = AtomicUsize::new(0);
    registry
        .submit_with(
            "agent-cedar-first",
            true,
            |_| {
                if !updating.pty.is_running() {
                    launches.fetch_add(1, Ordering::SeqCst);
                    *updating.pty.inner.output_signal.epoch.lock().unwrap() = 0;
                    launch(&updating.pty, &updating.dir, INPUT_BOX);
                }
                Ok(updating.pty.clone())
            },
            |_| panic!("ordinary path"),
            |pty, launch| {
                pty.wait_until_ready(launch, &FAST)?;
                pty.deliver_first(launch, &message(), &FAST, || {
                    delivered.fetch_add(1, Ordering::SeqCst);
                })
            },
        )
        .unwrap();
    assert_eq!((launches.into_inner(), delivered.into_inner()), (1, 1));
    assert_eq!(updating.file("paste"), message().input);
    assert_eq!(updating.file("enter"), "\r");
}

#[test]
fn a_prompt_that_gives_way_to_the_input_box_gets_the_message_again() {
    let fake = fake(&format!("stty raw -echo; printf prompt; {SWALLOW} {SWALLOW} printf box; \
        dd bs=4096 count=1 2>/dev/null | tee \"$1/paste\"; dd bs=4096 count=1 2>/dev/null > \"$1/enter\"; printf working; sleep 5"));
    let (result, delivered) = fake.deliver(&message(), &FAST);
    result.unwrap();
    assert_eq!(delivered, 1);
    assert_eq!(fake.file("paste"), message().input);
    assert_eq!(fake.file("enter"), "\r");
}

#[test]
fn a_second_paste_without_echo_gets_no_enter() {
    let fake = fake(&format!(
        "stty raw -echo; printf prompt; {SWALLOW} {SWALLOW} printf other; cat > \"$1/rest\""
    ));
    let (result, delivered) = fake.deliver(
        &message(),
        &Timing {
            watch: Duration::from_secs(3),
            ..FAST
        },
    );
    assert_eq!(why(result), NotTaken::NoEcho);
    assert_eq!(delivered, 0);
    assert_eq!(
        fake.file("rest"),
        message().input,
        "pasted once more, and no Enter"
    );
}

#[test]
fn silence_until_the_end_is_no_echo() {
    let fake = fake("stty raw -echo; printf prompt; cat > /dev/null");
    let (result, delivered) = fake.deliver(
        &message(),
        &Timing {
            watch: Duration::from_secs(1),
            ..FAST
        },
    );
    assert_eq!(why(result), NotTaken::NoEcho);
    assert_eq!(delivered, 0);
}

#[test]
fn a_terminal_that_never_stops_working_took_it_unless_it_echoes_by_itself() {
    let busy = "while :; do printf .; sleep 0.1; done";
    let timing = Timing {
        watch: Duration::from_secs(1),
        ..FAST
    };
    let working = fake(&format!(
        "stty raw -echo; printf prompt; {SWALLOW} {SWALLOW} {busy}"
    ));
    let (result, delivered) = working.deliver(&message(), &timing);
    result.unwrap();
    assert_eq!(delivered, 1);
    let updating = fake(&format!(
        "stty raw -echo; printf prompt; {SWALLOW} {SWALLOW} stty sane; {busy}"
    ));
    let (result, delivered) = updating.deliver(&message(), &timing);
    assert_eq!(why(result), NotTaken::NoEcho);
    assert_eq!(delivered, 0);
}

#[test]
fn a_process_that_leaves_echo_on_is_delivered_to_as_ever() {
    let fake = fake("printf ready; sleep 20");
    let (result, delivered) = fake.deliver(&message(), &FAST);
    result.unwrap();
    assert_eq!(delivered, 1);
}

#[test]
fn a_changed_session_file_is_a_delivery() {
    let fake = fake(&format!("stty raw -echo; printf prompt; {SWALLOW} {SWALLOW} echo entry >> \"$1/session.jsonl\"; cat > /dev/null"));
    let session = fake.dir.join("session.jsonl");
    fs::write(&session, "{}\n").unwrap();
    let message = PreparedSubmit {
        native_before: NativeSessionMarker::read(session),
        ..message()
    };
    let (result, delivered) = fake.deliver(&message, &FAST);
    result.unwrap();
    assert_eq!(delivered, 1);
}

#[test]
fn a_first_message_waits_for_the_process_to_print_and_settle() {
    let silent = fake("stty raw -echo; sleep 1; printf ready; cat > /dev/null");
    let launch = silent.pty.awaits_first_submit().unwrap();
    let started = Instant::now();
    silent.pty.wait_until_ready(launch, &FAST).unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(1200),
        "{:?}",
        started.elapsed()
    );
    // It has printed: nothing to wait for.
    let started = Instant::now();
    silent.pty.wait_until_ready(launch, &FAST).unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "{:?}",
        started.elapsed()
    );

    let mute = fake("stty raw -echo; sleep 20");
    let launch = mute.pty.awaits_first_submit().unwrap();
    let started = Instant::now();
    mute.pty
        .wait_until_ready(
            launch,
            &Timing {
                ready: Duration::from_millis(600),
                ..FAST
            },
        )
        .unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(600),
        "a limit, not a failure"
    );

    let gone = fake("exit 0");
    let launch = gone.pty.awaits_first_submit().unwrap();
    assert_eq!(
        gone.pty.wait_until_ready(launch, &FAST),
        Err(NotTaken::Exited)
    );
}

#[test]
fn watching_leaves_the_terminal_to_a_spawn_and_holds_back_other_deliveries() {
    let fake = fake(&format!(
        "stty raw -echo; printf prompt; {SWALLOW} {SWALLOW} cat > /dev/null"
    ));
    let registry = AgentRegistry::default();
    let id = fake.pty.inner.agent_id.clone();
    registry
        .ptys
        .lock()
        .unwrap()
        .insert(id.clone(), fake.pty.clone());
    let replacement = registry.replacement(&id);
    let order = Mutex::new(Vec::new());
    let first = |pty: &AgentTerminalPty, launch: u64| {
        pty.wait_until_ready(launch, &FAST)?;
        let timing = Timing {
            watch: Duration::from_secs(20),
            ..FAST
        };
        let result = pty.deliver_first(launch, &message(), &timing, || panic!("not delivered"));
        order.lock().unwrap().push("first");
        result
    };
    thread::scope(|scope| {
        let watching = scope.spawn(|| {
            registry.submit_with(
                &id,
                false,
                |_| registry.lookup(&id),
                |_| panic!("ordinary path"),
                first,
            )
        });
        while fake.pty.awaits_first_submit().is_some() {
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(1200)); // pasted, entered, and now watching
        let other = scope.spawn(|| {
            registry.submit_with(
                &id,
                false,
                |_| registry.lookup(&id),
                |_| panic!("the new process has had no message"),
                |pty, launch| {
                    order.lock().unwrap().push("second");
                    pty.inner.first_submit.store(launch, Ordering::SeqCst);
                    Ok(())
                },
            )
        });
        assert!(
            replacement.delivery.try_lock().is_err(),
            "the next delivery waits"
        );

        // The seat is clicked: a takeover replaces the process without waiting for the watch.
        let started = Instant::now();
        let epoch = replacement.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let guard = replacement
            .serial
            .try_lock()
            .expect("a spawn must not wait for the watch");
        let takeover = AgentSpawnRequest {
            takeover: true,
            ..request(&fake.dir)
        };
        registry
            .spawn_serial_with(
                takeover,
                &replacement,
                epoch,
                None,
                |_| panic!("reused"),
                |pty| {
                    launch(
                        pty,
                        &fake.dir,
                        "stty raw -echo; printf new; cat > \"$1/new\"",
                    );
                    Ok(())
                },
                |_| {},
            )
            .unwrap();
        drop(guard);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );

        assert_eq!(why(watching.join().unwrap()), NotTaken::Replaced);
        other.join().unwrap().unwrap();
    });
    assert_eq!(*order.lock().unwrap(), ["first", "second"]);
    assert_eq!(
        fake.file("new"),
        "",
        "nothing is written into the new process"
    );
    registry.lookup(&id).unwrap().stop_current();
}

#[test]
fn a_composer_message_is_tried_again_once_after_an_exit() {
    let registry = AgentRegistry::default();
    let fake = fake("stty raw -echo; printf ready; cat > /dev/null");
    let tries = |relaunch: bool, outcomes: &[Option<NotTaken>]| {
        let count = AtomicUsize::new(0);
        fake.pty
            .inner
            .first_submit
            .store(u64::MAX, Ordering::SeqCst);
        let result = registry.submit_with(
            "agent-cedar-first",
            relaunch,
            |_| Ok(fake.pty.clone()),
            |_| panic!("ordinary path"),
            |_, _| match outcomes[count.fetch_add(1, Ordering::SeqCst)] {
                Some(why) => Err(why.into()),
                None => Ok(()),
            },
        );
        (
            count.into_inner(),
            result
                .err()
                .map(|error| *error.downcast_ref::<NotTaken>().unwrap()),
        )
    };
    use NotTaken::{Exited, NoEcho, Replaced};
    assert_eq!(tries(true, &[None]), (1, None));
    assert_eq!(tries(true, &[Some(Exited), None]), (2, None));
    assert_eq!(
        tries(true, &[Some(Exited), Some(Exited), None]),
        (2, Some(Exited))
    );
    assert_eq!(
        tries(true, &[Some(Exited), Some(NoEcho)]),
        (2, Some(NoEcho))
    );
    assert_eq!(tries(true, &[Some(NoEcho), None]), (1, Some(NoEcho)));
    assert_eq!(tries(true, &[Some(Replaced), None]), (1, Some(Replaced)));
    // The bus owns its own second round.
    assert_eq!(tries(false, &[Some(Exited), None]), (1, Some(Exited)));
}

#[test]
fn only_the_first_message_of_a_process_is_watched() {
    let registry = AgentRegistry::default();
    let fake = fake("stty raw -echo; printf ready; cat > /dev/null");
    let paths = Mutex::new(Vec::new());
    let submit = || {
        registry.submit_with(
            "agent-cedar-first",
            false,
            |_| Ok(fake.pty.clone()),
            |_| {
                paths.lock().unwrap().push("ordinary");
                Ok(())
            },
            |pty, launch| {
                paths.lock().unwrap().push("first");
                pty.inner.first_submit.store(launch, Ordering::SeqCst);
                Ok(())
            },
        )
    };
    submit().unwrap();
    submit().unwrap();
    launch(
        &fake.pty,
        &fake.dir,
        "stty raw -echo; printf ready; cat > /dev/null",
    );
    submit().unwrap();
    submit().unwrap();
    assert_eq!(
        *paths.lock().unwrap(),
        ["first", "ordinary", "first", "ordinary"]
    );
}
