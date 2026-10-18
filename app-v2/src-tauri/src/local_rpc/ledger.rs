use super::protocol::{DeliveryState as State, Injection, MAX_RECORD};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
#[cfg(unix)]
use std::os::unix::{fs::{OpenOptionsExt, PermissionsExt}, io::AsRawFd};

#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub session: String,
    pub injection: Injection,
    pub state: State,
    pub sequence: u64,
    pub at: String,
    pub bytes: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case", deny_unknown_fields)]
enum Record {
    Input { session: String, sequence: u64, injection: Injection, at: String },
    Fact { session: String, kota_id: String, state: State, at: String },
    /// A new process proved absence by reading the entire reloaded session.
    Replay { session: String, kota_id: String, at: String },
    Retry { session: String, kota_id: String, at: String, #[serde(default)] sequence: Option<u64> },
    Reorder { session: String, ids: Vec<String> },
}

pub(super) struct Ledger {
    file: File,
    entries: BTreeMap<(String, String), Entry>,
    next_sequence: u64,
    writable: bool,
}

pub(super) fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() { bail!("Local directory cannot be a symlink"); }
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

impl Ledger {
    pub fn open(path: &Path) -> Result<Self> {
        private_dir(path.parent().context("Local ledger has no parent")?)?;
        let mut options = OpenOptions::new();
        options.read(true).append(true).create(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let file = options.open(path).context("open Local ledger")?;
        #[cfg(unix)]
        {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            // A second application must not append facts under a different owner.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                    bail!("Local ledger already has an owner (errno {}): {error}", libc::EWOULDBLOCK);
                }
                bail!("Local ledger lock failed (errno {:?}): {error}", error.raw_os_error());
            }
        }
        let mut ledger = Self { file, entries: BTreeMap::new(), next_sequence: 0, writable: true };
        let mut reader = BufReader::new(ledger.file.try_clone()?);
        let mut good_length = 0;
        loop {
            let mut line = Vec::new();
            let length = (&mut reader).take((MAX_RECORD + 1) as u64).read_until(b'\n', &mut line)?;
            if length == 0 { break; }
            if length > MAX_RECORD { bail!("Local ledger record exceeds limit"); }
            if line.last() != Some(&b'\n') {
                // Only the incomplete final append is discarded. All complete
                // records are validated; corruption is not silently skipped.
                ledger.file.set_len(good_length)?;
                ledger.file.sync_data()?;
                break;
            }
            let record: Record = serde_json::from_slice(&line).context("invalid Local ledger record")?;
            ledger.apply(record)?;
            good_length += length as u64;
        }
        Ok(ledger)
    }

    fn append(&mut self, record: Record) -> Result<()> {
        if !self.writable { bail!("Local ledger must be reopened after a write failure"); }
        let mut bytes = serde_json::to_vec(&record)?;
        bytes.push(b'\n');
        if let Err(error) = self.file.write_all(&bytes).and_then(|_| self.file.sync_data()) {
            self.writable = false;
            return Err(error.into());
        }
        self.apply(record)
    }

    fn apply(&mut self, record: Record) -> Result<()> {
        match record {
            Record::Reorder { session, ids } => {
                let slots = self.reorder_slots(&session, &ids)?;
                for (id, sequence) in ids.into_iter().zip(slots) {
                    self.entries.get_mut(&(session.clone(), id)).unwrap().sequence = sequence;
                }
            },
            Record::Input { session, sequence, injection, at } => {
                injection.validate(&injection.details.target_agent_id)?;
                let key = (session.clone(), injection.kota_id.clone());
                if self.entries.contains_key(&key) || sequence < self.next_sequence { bail!("duplicate Local ledger input"); }
                self.next_sequence = sequence.checked_add(1).context("Local sequence exhausted")?;
                let bytes = serde_json::to_vec(&injection)?.len();
                self.entries.insert(key, Entry { session, injection, state: State::Held, sequence, at, bytes });
            },
            Record::Fact { session, kota_id, state, at } => {
                let entry = self.entries.get_mut(&(session, kota_id)).context("Local fact has no input")?;
                entry.state = reduce(entry.state, state);
                entry.at = at;
            },
            Record::Replay { session, kota_id, at } => {
                let entry = self.entries.get_mut(&(session, kota_id)).context("Local replay has no input")?;
                if entry.state.terminal() || entry.state == State::ClearRequested { bail!("cannot replay terminal/clearing Local input"); }
                entry.state = State::Held;
                entry.at = at;
            },
            Record::Retry { session, kota_id, at, sequence } => {
                let entry = self.entries.get_mut(&(session, kota_id)).context("Local retry has no input")?;
                if !matches!(entry.state, State::Failed | State::Cleared | State::Abandoned) {
                    bail!("Local explicit retry has no retryable failure");
                }
                entry.state = State::Held;
                entry.at = at;
                if let Some(sequence) = sequence {
                    if sequence < self.next_sequence { bail!("invalid Local retry sequence"); }
                    self.next_sequence = sequence.checked_add(1).context("Local sequence exhausted")?;
                    entry.sequence = sequence;
                }
            },
        }
        Ok(())
    }

    fn reorder_slots(&self, session: &str, ids: &[String]) -> Result<Vec<u64>> {
        if !(2..=64).contains(&ids.len()) || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
            bail!("invalid Local reorder IDs");
        }
        let mut slots = ids.iter().map(|id| self.get(session, id).map(|entry| entry.sequence)
            .context("Local reorder has no input")).collect::<Result<Vec<_>>>()?;
        slots.sort_unstable();
        Ok(slots)
    }

    /// An ordering fact only: states, timestamps, payloads and next_sequence
    /// stay intact. A late consumed fact can precede this ACK and still wins.
    pub fn reorder(&mut self, session: &str, ids: Vec<String>) -> Result<()> {
        self.reorder_slots(session, &ids)?;
        self.append(Record::Reorder { session: session.into(), ids })
    }

    /// Idempotent submission requires the original, byte-equivalent input.
    pub fn hold(&mut self, session: &str, injection: Injection) -> Result<bool> {
        if let Some(entry) = self.get(session, &injection.kota_id) {
            if entry.injection != injection { bail!("Local delivery ID reused with different input"); }
            return Ok(false);
        }
        self.append(Record::Input { session: session.into(), sequence: self.next_sequence, injection, at: chrono::Utc::now().to_rfc3339() })?;
        Ok(true)
    }

    pub fn get(&self, session: &str, id: &str) -> Option<&Entry> {
        self.entries.get(&(session.into(), id.into()))
    }

    pub fn retry(&mut self, session: &str, id: &str) -> Result<Option<Entry>> {
        let entry = self.get(session, id).context("unknown Local delivery")?;
        if !matches!(entry.state, State::Failed | State::Cleared | State::Abandoned) { return Ok(None); }
        self.append(Record::Retry { session: session.into(), kota_id: id.into(), at: chrono::Utc::now().to_rfc3339(), sequence: Some(self.next_sequence) })?;
        Ok(self.get(session, id).cloned())
    }

    pub fn record(&mut self, session: &str, id: &str, state: State) -> Result<Option<Entry>> {
        let Some(entry) = self.get(session, id) else { return Ok(None); };
        if reduce(entry.state, state) == entry.state { return Ok(None); }
        self.append(Record::Fact { session: session.into(), kota_id: id.into(), state, at: chrono::Utc::now().to_rfc3339() })?;
        Ok(self.get(session, id).cloned())
    }

    pub fn pending(&self, session: &str) -> Vec<Entry> {
        let mut result: Vec<_> = self.entries.values().filter(|e|
            e.session == session && !e.state.terminal()).cloned().collect();
        result.sort_by_key(|e| e.sequence);
        result
    }

    pub fn next_held_ref(&self, session: &str) -> Option<&Entry> {
        self.entries.values().filter(|e| e.session == session && e.state == State::Held).min_by_key(|e| e.sequence)
    }

    pub(super) fn queued_entries(&self, session: &str, limit: usize) -> (usize, Vec<&Entry>) {
        let mut first = BTreeMap::new();
        let mut count = 0;
        for entry in self.entries.values().filter(|e| e.session == session && e.state.queued()) {
            count += 1;
            first.insert(entry.sequence, entry);
            if first.len() > limit { first.pop_last(); }
        }
        (count, first.into_values().collect())
    }

    pub fn queued(&self, session: &str) -> usize {
        self.entries.values().filter(|e| e.session == session && e.state.queued()).count()
    }

    /// Called exactly once after a NEW process reloads the same session.
    pub fn reconcile(&mut self, session: &str, present: &BTreeSet<String>) -> Result<Vec<Entry>> {
        let mut changed = Vec::new();
        let entries: Vec<_> = self.entries.values().filter(|e| e.session == session && e.state != State::Durable).cloned().collect();
        for entry in entries {
            let id = &entry.injection.kota_id;
            if present.contains(id) {
                if let Some(entry) = self.record(session, id, State::Durable)? { changed.push(entry); }
            } else if !entry.state.terminal() {
                if let Some(entry) = self.record(session, id, State::Abandoned)? { changed.push(entry); }
            }
        }
        Ok(changed)
    }

    pub fn abandon_other_sessions(&mut self, current: &str) -> Result<Vec<Entry>> {
        let others: Vec<_> = self.entries.values().filter(|e| e.session != current && !e.state.terminal()).cloned().collect();
        let mut changed = Vec::new();
        for entry in others {
            if let Some(entry) = self.record(&entry.session, &entry.injection.kota_id, State::Abandoned)? {
                changed.push(entry);
            }
        }
        Ok(changed)
    }

    pub fn freeze_clear(&mut self, session: &str, writing: Option<&str>) -> Result<Vec<Entry>> {
        let queued: Vec<_> = self.pending(session).into_iter().filter(|e| e.state.queued()).collect();
        let mut changed = Vec::new();
        for entry in queued {
            let state = if entry.state == State::Held && writing != Some(entry.injection.kota_id.as_str()) {
                State::Cleared
            } else { continue };
            if let Some(entry) = self.record(session, &entry.injection.kota_id, state)? { changed.push(entry); }
        }
        Ok(changed)
    }
}

impl Drop for Ledger {
    fn drop(&mut self) {
        // CLOEXEC only closes a forked child's copy at exec. Explicit unlock
        // releases this open-file-description lock before File is closed,
        // including owner exit and failed reads during Ledger::open.
        #[cfg(unix)]
        if unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) } != 0 {
            let error = std::io::Error::last_os_error();
            crate::kota_debug_log(&format!("local-rpc: ledger unlock failed (errno {:?}): {error}", error.raw_os_error()));
        }
    }
}

fn reduce(current: State, incoming: State) -> State {
    if current == State::Durable { return current; }
    if matches!(incoming, State::Consumed | State::Durable) { return incoming; }
    if current.terminal() { return current; }
    if matches!(incoming, State::Abandoned | State::Durable) { return incoming; }
    // Consumed is not durable, but no command response or Clear can undo it.
    if current == State::Consumed { return current; }
    if incoming == State::Consumed { return incoming; }
    if incoming.terminal() { return incoming; }
    if current == State::ClearRequested { return current; }
    if incoming == State::ClearRequested { return incoming; }
    if incoming == State::Held { return current; } // Only Input/explicit Retry may enter Held.
    if current == State::Accepted && incoming == State::Sent { return current; }
    incoming
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::protocol::fixture;
    fn temp() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("local-ledger-{}", uuid::Uuid::new_v4())).join("ledger.jsonl")
    }
    #[cfg(unix)]
    #[test]
    fn a_live_ledger_excludes_same_process_and_other_process_owners() {
        let path = temp();
        let ledger = Ledger::open(&path).unwrap();
        let error = Ledger::open(&path).err().unwrap().to_string();
        assert!(error.contains("already has an owner"), "{error}");
        assert!(error.contains(&format!("errno {}", libc::EWOULDBLOCK)), "{error}");
        let output = std::process::Command::new("python3")
            .arg("-c").arg(r#"
import errno,fcntl,sys
with open(sys.argv[1], 'ab') as file:
 try: fcntl.flock(file, fcntl.LOCK_EX | fcntl.LOCK_NB)
 except OSError as error:
  if error.errno in (errno.EWOULDBLOCK, errno.EAGAIN): sys.exit(0)
  raise
sys.exit(1)
"#).arg(&path).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        drop(ledger);
        drop(Ledger::open(&path).unwrap());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn parallel_spawn_cannot_extend_a_closed_ledgers_lock() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::sync::{Arc, Barrier};
        let gate = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8).map(|_| {
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || {
                let path = temp();
                let mut failures = Vec::new();
                gate.wait();
                for _ in 0..64 {
                    let ledger = Ledger::open(&path).unwrap();
                    assert_ne!(unsafe { libc::fcntl(ledger.file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
                    // process_group matches the production spawn path and
                    // leaves a fork/exec window while other workers close.
                    let mut child = Command::new("/bin/cat").stdin(Stdio::piped())
                        .stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn().unwrap();
                    drop(ledger);
                    match Ledger::open(&path) {
                        Ok(next) => drop(next),
                        Err(error) => failures.push(error.to_string()),
                    }
                    child.stdin.take();
                    assert!(child.wait().unwrap().success());
                }
                std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
                failures
            })
        }).collect();
        let failures: Vec<_> = workers.into_iter().flat_map(|worker| worker.join().unwrap()).collect();
        assert!(failures.is_empty(), "{} lock failures in 512 close/reopen cycles: {failures:?}", failures.len());
    }
    #[test]
    fn disk_recovery_covers_late_ack_and_consumed_but_not_saved() {
        let path = temp();
        let a = fixture("input-a"); let b = fixture("input-b");
        {
            let mut ledger = Ledger::open(&path).unwrap();
            ledger.hold("session-a", a.clone()).unwrap();
            ledger.hold("session-a", b.clone()).unwrap();
            ledger.record("session-a", &a.kota_id, State::Consumed).unwrap();
            ledger.record("session-a", &a.kota_id, State::Accepted).unwrap();
            assert_eq!(ledger.get("session-a", &a.kota_id).unwrap().state, State::Consumed);
            ledger.record("session-a", &b.kota_id, State::Accepted).unwrap();
        }
        let mut ledger = Ledger::open(&path).unwrap();
        ledger.reconcile("session-a", &BTreeSet::from([b.kota_id.clone()])).unwrap();
        assert_eq!(ledger.get("session-a", &b.kota_id).unwrap().state, State::Durable);
        assert_eq!(ledger.get("session-a", &a.kota_id).unwrap().state, State::Abandoned);
        assert!(ledger.pending("session-a").is_empty());
        assert!(!ledger.hold("session-a", b).unwrap());
        drop(ledger); std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn stop_freezes_unwritten_items_and_late_consumption_wins() {
        let path = temp(); let mut ledger = Ledger::open(&path).unwrap();
        let sent = fixture("sent"); let held = fixture("held"); let later = fixture("later");
        for input in [&sent, &held] { ledger.hold("a", input.clone()).unwrap(); }
        ledger.record("a", &sent.kota_id, State::Sent).unwrap();
        let frozen = ledger.freeze_clear("a", None).unwrap();
        ledger.hold("a", later.clone()).unwrap();
        assert_eq!(ledger.get("a", &held.kota_id).unwrap().state, State::Cleared);
        assert_eq!(ledger.get("a", &sent.kota_id).unwrap().state, State::Sent);
        ledger.record("a", &sent.kota_id, State::Consumed).unwrap();
        for entry in frozen { ledger.record("a", &entry.injection.kota_id, State::Cleared).unwrap(); }
        assert_eq!(ledger.get("a", &sent.kota_id).unwrap().state, State::Consumed);
        assert_eq!(ledger.queued("a"), 1);
        ledger.abandon_other_sessions("b").unwrap();
        assert_eq!(ledger.get("a", &later.kota_id).unwrap().state, State::Abandoned);
        drop(ledger); std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn interrupted_append_does_not_hide_complete_records_or_replay_clear() {
        let path = temp(); let a = fixture("a");
        {
            let mut ledger = Ledger::open(&path).unwrap();
            ledger.hold("s", a.clone()).unwrap();
            ledger.record("s", &a.kota_id, State::Sent).unwrap();
            ledger.freeze_clear("s", None).unwrap();
        }
        OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{\"record\":").unwrap();
        let mut ledger = Ledger::open(&path).unwrap();
        ledger.reconcile("s", &BTreeSet::new()).unwrap();
        assert_eq!(ledger.get("s", &a.kota_id).unwrap().state, State::Abandoned);
        drop(ledger); std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn late_confirmation_overrides_soft_clear_and_retry_moves_to_tail() {
        let path=temp();let mut ledger=Ledger::open(&path).unwrap();
        let a=fixture("edited");let b=fixture("other");
        ledger.hold("s",a.clone()).unwrap();ledger.hold("s",b.clone()).unwrap();
        ledger.record("s",&a.kota_id,State::Cleared).unwrap();
        ledger.retry("s",&a.kota_id).unwrap();
        assert!(ledger.get("s",&a.kota_id).unwrap().sequence>ledger.get("s",&b.kota_id).unwrap().sequence);
        ledger.record("s",&a.kota_id,State::Cleared).unwrap();
        assert_eq!(ledger.record("s",&a.kota_id,State::Consumed).unwrap().unwrap().state,State::Consumed);
        ledger.reconcile("s",&BTreeSet::from([a.kota_id.clone()])).unwrap();
        assert_eq!(ledger.get("s",&a.kota_id).unwrap().state,State::Durable);
        assert_eq!(ledger.get("s",&b.kota_id).unwrap().state,State::Abandoned);
        drop(ledger);let reopened=Ledger::open(&path).unwrap();
        assert_eq!(reopened.get("s",&a.kota_id).unwrap().state,State::Durable);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("\"record\":\"replay\""));
        drop(reopened);std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn reorder_changes_slots_only_and_add_retry_edit_return_stay_at_the_tail() {
        let path = temp(); let mut ledger = Ledger::open(&path).unwrap();
        let ids: Vec<_> = ["A", "B", "C"].map(|id| { let input = fixture(id); let id = input.kota_id.clone(); ledger.hold("s", input).unwrap(); id }).into();
        let before = ledger.pending("s");
        assert!(ledger.reorder("s", vec![ids[0].clone(), ids[0].clone()]).is_err());
        assert!(ledger.reorder("s", vec![ids[0].clone(), "missing".into()]).is_err());
        let order = vec![ids[2].clone(), ids[0].clone(), ids[1].clone()];
        ledger.reorder("s", order.clone()).unwrap();
        assert_eq!(ledger.pending("s").iter().map(|e| &e.injection.kota_id).collect::<Vec<_>>(), order.iter().collect::<Vec<_>>());
        for old in before { let new = ledger.get("s", &old.injection.kota_id).unwrap(); assert_eq!(new.injection, old.injection); assert_eq!(new.at, old.at); assert_eq!(new.state, old.state); }
        let d = fixture("D"); ledger.hold("s", d.clone()).unwrap();
        assert_eq!(ledger.get("s", &d.kota_id).unwrap().sequence, 3);
        for state in [State::Cleared, State::Failed] {
            ledger.record("s", &ids[2], state).unwrap(); ledger.retry("s", &ids[2]).unwrap();
            assert_eq!(ledger.pending("s").last().unwrap().injection.kota_id, ids[2]);
        }
        drop(ledger); let mut ledger = Ledger::open(&path).unwrap();
        ledger.reconcile("s", &BTreeSet::new()).unwrap();
        assert!(ledger.pending("s").is_empty());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().filter(|line| line.contains("\"record\":\"input\"")).count(), 4);
        assert_eq!(text.lines().filter(|line| line.contains("\"record\":\"reorder\"")).count(), 1);
        assert!(!text.contains("\"record\":\"replay\""));
        drop(ledger); std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn reorder_write_failure_does_not_change_memory_and_reopen_never_replays() {
        let path=temp();let mut ledger=Ledger::open(&path).unwrap();
        let a=fixture("A");let b=fixture("B");for input in [&a,&b] {ledger.hold("s",input.clone()).unwrap();}
        // A real failed write on this file, without a production failpoint.
        let locked=std::mem::replace(&mut ledger.file,OpenOptions::new().read(true).open(&path).unwrap());
        assert!(ledger.reorder("s",vec![b.kota_id.clone(),a.kota_id.clone()]).is_err());
        assert_eq!(ledger.pending("s")[0].injection.kota_id,a.kota_id);
        assert!(!ledger.writable);ledger.file=locked;drop(ledger);
        let mut ledger=Ledger::open(&path).unwrap();ledger.reconcile("s",&BTreeSet::new()).unwrap();assert!(ledger.pending("s").is_empty());
        drop(ledger);std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

}
