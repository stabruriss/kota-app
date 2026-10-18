use super::protocol::MAX_RECORD;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{atomic::{AtomicUsize, Ordering}, mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const MAX_BUFFERED: usize = 64 * 1024 * 1024;
const MAX_STDERR: usize = 64 * 1024;

pub(super) enum PipeEvent {
    Record(Value, Credit),
    Written(String),
    Eof,
}

pub(super) struct Credit { bytes: usize, total: Arc<AtomicUsize> }
impl Drop for Credit {
    fn drop(&mut self) { self.total.fetch_sub(self.bytes, Ordering::AcqRel); }
}

struct WriteRequest { id: String, bytes: Vec<u8> }

pub(super) struct Process {
    child: Child,
    stopped: bool,
    writer: Option<mpsc::SyncSender<WriteRequest>>,
    events: mpsc::Receiver<PipeEvent>,
    fault: Arc<Mutex<Option<String>>>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
}

fn set_fault(fault: &Mutex<Option<String>>, message: String) {
    let mut current = fault.lock().unwrap();
    if current.is_none() { *current = Some(message); }
}

fn emit(tx: &mpsc::SyncSender<PipeEvent>, fault: &Mutex<Option<String>>, event: PipeEvent) -> bool {
    if tx.try_send(event).is_err() {
        set_fault(fault, "Local RPC event queue exceeded its bound".into());
        return false;
    }
    true
}

/// The owner never blocks on child stdin/stdout or on a UI callback. Exhausting
/// either bound fails the transport explicitly; records are never truncated.
impl Process {
    pub fn spawn(mut command: Command) -> Result<Self> {
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().context("spawn Local Pi RPC")?;
        let stdout = child.stdout.take().context("Local stdout missing")?;
        let mut stdin = child.stdin.take().context("Local stdin missing")?;
        let mut stderr = child.stderr.take().context("Local stderr missing")?;
        // Short records can arrive in a network/tool burst. Keep their count
        // below a generous bound while Credit enforces the 64 MiB byte budget.
        let (tx, events) = mpsc::sync_channel(65_536);
        let (writer, writes) = mpsc::sync_channel::<WriteRequest>(8);
        let fault = Arc::new(Mutex::new(None));
        let total = Arc::new(AtomicUsize::new(0));
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        // Install the child guard before any fallible thread creation. Failure
        // to start a pipe worker must not orphan the spawned process.
        let process = Self { child, stopped: false, writer: Some(writer), events, fault: fault.clone(), stderr: stderr_tail.clone() };
        let fault_out = fault.clone(); let tx_out = tx.clone();
        thread::Builder::new().name("local-rpc-stdout".into()).spawn(move || {
            let result = (|| -> Result<()> {
                let mut reader = BufReader::new(stdout);
                loop {
                    let mut bytes = Vec::new();
                    let size = (&mut reader).take((MAX_RECORD + 1) as u64).read_until(b'\n', &mut bytes)?;
                    if size == 0 { emit(&tx_out, &fault_out, PipeEvent::Eof); return Ok(()); }
                    if size > MAX_RECORD { bail!("Local RPC stdout record exceeds 64 MiB"); }
                    if bytes.last() != Some(&b'\n') { bail!("Local RPC stdout ended inside a record"); }
                    let old = total.fetch_add(size, Ordering::AcqRel);
                    let credit = Credit { bytes: size, total: total.clone() };
                    if old + size > MAX_BUFFERED { bail!("Local RPC buffered output exceeds 64 MiB"); }
                    let value: Value = serde_json::from_slice(&bytes).context("Local RPC stdout is not JSON")?;
                    if !value.is_object() { bail!("Local RPC record is not an object"); }
                    if !emit(&tx_out, &fault_out, PipeEvent::Record(value, credit)) { return Ok(()); }
                }
            })();
            if let Err(error) = result { set_fault(&fault_out, error.to_string()); }
        })?;
        let fault_in = fault.clone();
        thread::Builder::new().name("local-rpc-stdin".into()).spawn(move || {
            for request in writes {
                if let Err(error) = stdin.write_all(&request.bytes).and_then(|_| stdin.flush()) {
                    set_fault(&fault_in, format!("Local RPC stdin failed: {error}"));
                    break;
                }
                if !emit(&tx, &fault_in, PipeEvent::Written(request.id)) { break; }
            }
        })?;
        let tail = stderr_tail.clone();
        thread::Builder::new().name("local-rpc-stderr".into()).spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(size) = stderr.read(&mut buffer) {
                if size == 0 { break; }
                let mut tail = tail.lock().unwrap();
                for byte in &buffer[..size] {
                    if tail.len() == MAX_STDERR { tail.pop_front(); }
                    tail.push_back(*byte);
                }
            }
        })?;
        Ok(process)
    }

    pub fn id(&self) -> u32 { self.child.id() }

    pub fn send(&self, request: Value) -> Result<()> {
        let id = request.get("id").and_then(Value::as_str).context("Local command ID missing")?.to_owned();
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_RECORD { bail!("Local command exceeds transport bound"); }
        self.writer.as_ref().context("Local stdin closed")?.try_send(WriteRequest { id, bytes })
            .map_err(|_| anyhow!("Local stdin queue is full or closed"))
    }

    pub fn poll(&self) -> Option<PipeEvent> { self.events.try_recv().ok() }
    pub fn fault(&self) -> Option<String> { self.fault.lock().unwrap().clone() }
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> { Ok(self.child.try_wait()?) }

    pub fn stop(&mut self) -> Result<()> {
        if self.stopped { return Ok(()); }
        self.writer.take();
        let mut exited = self.child.try_wait()?.is_some();
        // The Pi parent may already have exited while its read/tool children
        // still own pipes. Reap the owned group once, not just the parent.
        #[cfg(unix)]
        {
            // Proper-lockfile's exit handler cannot run after SIGKILL. Let Pi
            // close its credential lock normally before the bounded fallback.
            // Signal the owned group so tool children also begin shutting down.
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGTERM); }
            let deadline = Instant::now() + Duration::from_secs(2);
            while !exited && Instant::now() < deadline {
                exited = self.child.try_wait()?.is_some();
                if !exited { thread::sleep(Duration::from_millis(5)); }
            }
            let mut result = unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            // Darwin returns EPERM for a group containing only our unreaped
            // zombie. Reap an exit racing the first check, then retry the group
            // once so live tool descendants are still cleaned up.
            if result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
                && self.child.try_wait()?.is_some() {
                exited = true;
                result = unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            }
            if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        if !exited {
            #[cfg(not(unix))]
            self.child.kill()?;
            self.child.wait()?;
        }
        self.stopped = true;
        Ok(())
    }

    /// Explicit diagnostics only: never copied to the ordinary debug log.
    pub fn stderr_tail(&self) -> String {
        let bytes: Vec<_> = self.stderr.lock().unwrap().iter().copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Drop for Process {
    fn drop(&mut self) { let _ = self.stop(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{Duration, Instant};

    #[test]
    fn real_jsonl_child_echoes_unicode_without_terminal_framing_and_exits() {
        let mut cmd = Command::new("/usr/bin/python3");
        cmd.args(["-u", "-c", "import sys,json\nfor line in sys.stdin:\n v=json.loads(line); print(json.dumps({'type':'response','id':v['id'],'data':v}),flush=True)"]);
        let mut process = Process::spawn(cmd).unwrap();
        let request = json!({"id":"local-2-1", "type":"prompt", "message":"/kota-inject hello\n\"quoted\" 測試"});
        process.send(request.clone()).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(PipeEvent::Record(value, _credit)) = process.poll() {
                assert_eq!(value["data"], request); break;
            }
            assert!(process.fault().is_none());
            assert!(Instant::now() < end);
            thread::yield_now();
        }
        process.stop().unwrap();
        assert!(process.try_wait().unwrap().is_some());
    }

    #[test]
    fn invalid_partial_and_oversized_stdout_fail_without_becoming_records() {
        for (script, error) in [
            ("print('not json',flush=True)".to_string(), "not JSON"),
            ("import sys; sys.stdout.write('{\"unfinished\":'); sys.stdout.flush()".to_string(), "inside a record"),
            (format!("import sys; sys.stdout.write('x'*{}); sys.stdout.flush()", MAX_RECORD + 1), "exceeds 64 MiB"),
        ] {
            let mut cmd = Command::new("/usr/bin/python3"); cmd.args(["-u", "-c", &script]);
            let mut process = Process::spawn(cmd).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                assert!(!matches!(process.poll(), Some(PipeEvent::Record(..))));
                if let Some(fault) = process.fault() { assert!(fault.contains(error), "{fault}"); break; }
                assert!(Instant::now() < deadline); thread::yield_now();
            }
            process.stop().unwrap();
        }
    }
}
