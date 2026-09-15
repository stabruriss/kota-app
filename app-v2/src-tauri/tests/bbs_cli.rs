use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use uuid::Uuid;

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("bbs-cli-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn run(&self, args: &[&str], body: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kota-bbs"))
            .env_clear()
            .env("HOME", &self.0)
            .env("KOTA_BBS_ROOT", self.0.join("board"))
            .current_dir(&self.0)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn help_is_stdout_identity_free_and_never_initializes_storage() {
    let sandbox = Sandbox::new();
    for args in [
        vec![],
        vec!["help"],
        vec!["--help"],
        vec!["-h"],
        vec!["new", "help"],
        vec!["reply", "--help"],
        vec!["show", "-h"],
        vec!["agents", "--help"],
    ] {
        let output = sandbox.run(&args, "");
        assert!(output.status.success(), "{:?}", output);
        assert!(output.stderr.is_empty());
        assert!(String::from_utf8_lossy(&output.stdout).contains("--attach <file>"));
        assert!(!sandbox.0.join("board").exists());
    }
    for args in [
        vec!["new", "--broadcast", "extra"],
        vec!["reply", "thread-one", "extra"],
        vec!["show", "thread-one", "extra"],
    ] {
        assert!(!sandbox.run(&args, "").status.success());
        assert!(!sandbox.0.join("board").exists());
    }
}

#[test]
fn agents_and_explicit_mentions_are_real_cli_io_without_identity_or_a_control_lease() {
    let sandbox = Sandbox::new();
    let account = sandbox.0.join("Kota");
    let project = account.join("Workspaces/p");
    let agent = project.join(".agent-workspaces/a");
    fs::create_dir_all(&agent).unwrap();
    fs::write(
        project.join("workspace.json"),
        br#"{"projectId":"p","repoFullName":"org/project","archived":false,"agents":[]}"#,
    )
    .unwrap();
    fs::write(
        agent.join("agent.yaml"),
        "display-name: Target\nstatus: active\nsession-id: secret-session\n",
    )
    .unwrap();
    let listed = sandbox.run(&["agents"], "");
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert!(listed.stderr.is_empty());
    let roster: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(
        roster["devices"][0]["projects"][0]["agents"][0]["targetRef"],
        "local/p/a"
    );
    assert!(!String::from_utf8(listed.stdout)
        .unwrap()
        .contains("secret-session"));
    assert!(!account.join("bbs-sync").exists());
    assert!(!sandbox.0.join("board").exists());
    fs::write(sandbox.0.join("mention.png"), b"attachment bytes").unwrap();
    let sent = sandbox.run(&["new", "--broadcast", "--at", "local/p/a", "--attach", "./mention.png"], "From stdin.");
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let thread = String::from_utf8(sent.stdout).unwrap().trim().to_string();
    let posts = sandbox.0.join("board/threads").join(&thread).join("posts");
    let file = fs::read_dir(&posts)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let raw = fs::read_to_string(file).unwrap();
    assert!(raw.contains("mentions:"));
    assert!(raw.contains("deviceId: local"));
    assert!(raw.contains("@Target\n\nFrom stdin."));
    assert!(!account.join("bbs-sync/identity.json").exists());
    assert!(!account.join("bbs-sync/control.json").exists());
    let pending = account.join("bbs-sync/notify/pending");
    let entries = fs::read_dir(&pending).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(entries.len(), 1);
    let notice: serde_json::Value = serde_json::from_slice(&fs::read(entries[0].path()).unwrap()).unwrap();
    assert_eq!(notice["destination"]["threadId"], thread);
    assert_eq!(notice["destination"]["target"], serde_json::json!({"deviceId":"local","projectId":"p","agentId":"a"}));
    assert_eq!(notice["ready"], true);
    assert_eq!(notice["waiting"], serde_json::json!([]));
    assert!(!project.join("project-memory").exists(), "the CLI only publishes; the App owns room delivery");
    let count = fs::read_dir(&posts).unwrap().count();
    for args in [
        vec!["agents", "--json"],
        vec!["reply", &thread, "--at"],
        vec!["reply", &thread, "--at", "local/p/a", "--at", "local/p/a"],
    ] {
        assert!(!sandbox.run(&args, "Not published").status.success());
    }
    assert_eq!(fs::read_dir(posts).unwrap().count(), count);
}

#[test]
fn cli_publishes_metadata_only_attachments_and_reports_complete_errors() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.0.join("screen shot.png"), b"image bytes").unwrap();
    fs::write(sandbox.0.join("report.pdf"), b"document bytes").unwrap();
    let output = sandbox.run(
        &[
            "new",
            "--broadcast",
            "--attach",
            "./screen shot.png",
            "--attach",
            "./report.pdf",
        ],
        "Prose without injected paths.",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let thread = String::from_utf8(output.stdout).unwrap().trim().to_string();
    let output = sandbox.run(&["reply", &thread, "--attach", "./report.pdf"], "");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let post = String::from_utf8(output.stdout).unwrap().trim().to_string();
    let shown = sandbox.run(&["show", &thread], "");
    assert!(shown.status.success());
    let text = String::from_utf8(shown.stdout).unwrap();
    assert!(text.contains(&format!("post: {post}")));
    assert!(text.contains("screen shot.png"));
    assert!(text.contains("sha256:"));
    assert!(text.contains("/attachments/"));
    assert_eq!(
        fs::read(sandbox.0.join("report.pdf")).unwrap(),
        b"document bytes"
    );
    let failure = sandbox.run(
        &["reply", &thread, "--attach", "./gone.pdf"],
        "not published",
    );
    assert!(!failure.status.success());
    let error = String::from_utf8_lossy(&failure.stderr);
    assert!(error.contains("gone.pdf"));
    assert!(error.contains("No such file") || error.contains("not found"));
    assert!(
        !String::from_utf8(sandbox.run(&["show", &thread], "").stdout)
            .unwrap()
            .contains("not published")
    );
}
