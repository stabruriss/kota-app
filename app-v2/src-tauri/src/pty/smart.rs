use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Context, Result};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use super::ansi::{AnsiLineDecoder, GridSnapshot};
use super::path_env::{augmented_path, resolve_on_augmented_path};

const SMART_OUTPUT_EVENT: &str = "pty://smart/output";
const SMART_EXIT_EVENT: &str = "pty://smart/exit";
const SMART_STATUS_EVENT: &str = "pty://smart/status";
/// Fires when the controlling terminal's foreground process group
/// transitions BACK to the shell after having been some child (claude
/// / codex / ssh / make / whatever). This is the signal the frontend
/// uses to clear `tuiRunning` after the user `/exit`s the agent CLI
/// without killing the parent shell — `pty://smart/exit` only fires
/// on shell-process death and misses subprocess exits. Computed via
/// `MasterPty::process_group_leader()` (a `tcgetpgrp` syscall under
/// the hood) on the existing emitter thread tick — sub-µs cost.
const SMART_TUI_EXIT_EVENT: &str = "pty://smart/tui-exit";
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 32;
const MAGI_TRANSLATE_PROMPT_FILE: &str = "magi-nl-translate.md";
const MAGI_TRANSLATE_PROMPT_TEMPLATE: &str = include_str!("../../prompts/magi-nl-translate.md");

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartOutputEvent {
    pub pty_id: String,
    /// Full visible-grid snapshot. Replaces the legacy line-based payload
    /// (per the M6.A grid-renderer refactor). See pty/ansi.rs::GridSnapshot.
    pub snapshot: GridSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartStatusEvent {
    pub pty_id: String,
    pub running: bool,
    pub cwd: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartExitPayload {
    pub code: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartExitEvent {
    pub pty_id: String,
    pub code: Option<i32>,
}

/// Payload for `SMART_TUI_EXIT_EVENT`. The frontend uses just `pty_id`
/// to resolve which tab; the event itself only fires when the shell's
/// own pgid has reclaimed the controlling tty (i.e., the in-flight
/// subprocess exited).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartTuiExitEvent {
    pub pty_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartPtySummary {
    pub pty_id: String,
    pub cwd: String,
    pub running: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateResult {
    pub kind: TranslateKind,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<MagiProvider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MagiProvider {
    Claude,
    Codex,
}

impl Default for MagiProvider {
    fn default() -> Self {
        MagiProvider::Claude
    }
}

impl MagiProvider {
    fn as_str(self) -> &'static str {
        match self {
            MagiProvider::Claude => "claude",
            MagiProvider::Codex => "codex",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranslateKind {
    Command,
    Escape,
}

struct SmartProcess {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    decoder: Arc<Mutex<AnsiLineDecoder>>,
}

struct SmartState {
    pty_id: String,
    shell: String,
    home: PathBuf,
    cwd: Mutex<PathBuf>,
    size: Mutex<PtySize>,
    generation: AtomicU64,
    process: Mutex<Option<SmartProcess>>,
}

#[derive(Clone)]
pub struct SmartTerminalPty {
    inner: Arc<SmartState>,
}

impl SmartTerminalPty {
    pub fn new(pty_id: String, cwd: Option<String>, cli: Option<String>) -> Result<Self> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let shell = resolve_shell(cli.as_deref());
        let cwd = resolve_initial_cwd(&home, cwd.as_deref());

        Ok(Self {
            inner: Arc::new(SmartState {
                pty_id,
                shell,
                cwd: Mutex::new(cwd),
                home,
                size: Mutex::new(PtySize {
                    cols: DEFAULT_COLS,
                    rows: DEFAULT_ROWS,
                    pixel_width: 0,
                    pixel_height: 0,
                }),
                generation: AtomicU64::new(0),
                process: Mutex::new(None),
            }),
        })
    }

    pub fn pty_id(&self) -> String {
        self.inner.pty_id.clone()
    }

    pub fn summary(&self) -> SmartPtySummary {
        let cwd = self.inner.cwd.lock().expect("smart cwd poisoned");
        SmartPtySummary {
            pty_id: self.inner.pty_id.clone(),
            cwd: display_path(&self.inner.home, &cwd),
            running: self.is_running(),
        }
    }

    pub fn is_running(&self) -> bool {
        self.inner
            .process
            .lock()
            .expect("smart process poisoned")
            .is_some()
    }

    pub fn init(&self, app: &AppHandle) -> Result<()> {
        if self.is_running() {
            self.emit_status(app, true);
            return Ok(());
        }

        self.spawn(app)
    }

    pub fn write(&self, app: &AppHandle, input: String) -> Result<()> {
        if !self.is_running() {
            self.spawn(app)?;
        }

        let trimmed = input.trim();
        if matches!(trimmed, "clear" | "reset") {
            return self.clear(app);
        }

        // Snapshot cwd into a local so the read guard is dropped BEFORE
        // resolve_cd_target's if-let body re-locks for the write. Without
        // this split, an `if let` scrutinee holds the MutexGuard for the
        // whole arm, and the second `lock()` deadlocks the IPC handler
        // (manifests as the entire app freezing on `cd <existing-dir>`).
        let cwd_snapshot = self.inner.cwd.lock().expect("smart cwd poisoned").clone();
        if let Some(next_cwd) = resolve_cd_target(&self.inner.home, &cwd_snapshot, trimmed) {
            *self.inner.cwd.lock().expect("smart cwd poisoned") = next_cwd;
            self.emit_status(app, true);
        }

        let mut process_guard = self.inner.process.lock().expect("smart process poisoned");
        let process = process_guard
            .as_mut()
            .ok_or_else(|| anyhow!("smart terminal process missing"))?;
        process
            .writer
            .write_all(input.as_bytes())
            .context("write smart terminal input")?;
        process
            .writer
            .flush()
            .context("flush smart terminal input")?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let size = PtySize {
            cols: cols.max(1),
            rows: rows.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        if let Ok(mut current_size) = self.inner.size.lock() {
            *current_size = size;
        }

        let mut process_guard = match self.inner.process.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                crate::kota_debug_log("[smart] process mutex poisoned during resize; recovering");
                self.inner.process.clear_poison();
                poisoned.into_inner()
            }
        };
        let Some(process) = process_guard.as_mut() else {
            return Ok(());
        };
        let _ = process.master.resize(size);
        let mut decoder = match process.decoder.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                crate::kota_debug_log("[smart] decoder mutex poisoned during resize; recovering");
                process.decoder.clear_poison();
                poisoned.into_inner()
            }
        };
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            decoder.resize(size.cols, size.rows);
        }))
        .is_err()
        {
            crate::kota_debug_log(&format!(
                "[smart] decoder resize panicked at {}x{}; resetting decoder",
                size.cols, size.rows,
            ));
            *decoder = AnsiLineDecoder::new(size.cols, size.rows);
        }
        Ok(())
    }

    pub fn scroll(&self, app: &AppHandle, lines: i32) -> Result<()> {
        let Some(snapshot) = ({
            let mut process_guard = match self.inner.process.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    crate::kota_debug_log(
                        "[smart] process mutex poisoned during scroll; recovering",
                    );
                    self.inner.process.clear_poison();
                    poisoned.into_inner()
                }
            };
            let Some(process) = process_guard.as_mut() else {
                return Ok(());
            };
            let mut decoder = match process.decoder.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    crate::kota_debug_log(
                        "[smart] decoder mutex poisoned during scroll; recovering",
                    );
                    process.decoder.clear_poison();
                    poisoned.into_inner()
                }
            };
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                decoder.scroll_display(lines);
            }))
            .is_err()
            {
                crate::kota_debug_log("[smart] decoder scroll panicked; resetting decoder");
                *decoder = AnsiLineDecoder::new(
                    self.inner
                        .size
                        .lock()
                        .map(|s| s.cols)
                        .unwrap_or(DEFAULT_COLS),
                    self.inner
                        .size
                        .lock()
                        .map(|s| s.rows)
                        .unwrap_or(DEFAULT_ROWS),
                );
                None
            } else {
                Some(decoder.snapshot())
            }
        }) else {
            return Ok(());
        };

        self.emit_output_snapshot(app, snapshot);
        Ok(())
    }

    pub fn interrupt(&self) -> Result<()> {
        let mut process_guard = self.inner.process.lock().expect("smart process poisoned");
        let process = process_guard
            .as_mut()
            .ok_or_else(|| anyhow!("smart terminal process missing"))?;
        process.writer.write_all(&[0x03])?;
        process.writer.flush()?;
        Ok(())
    }

    pub fn clear(&self, app: &AppHandle) -> Result<()> {
        let snapshot = if let Some(process) = self
            .inner
            .process
            .lock()
            .expect("smart process poisoned")
            .as_mut()
        {
            let mut dec = process.decoder.lock().expect("smart decoder poisoned");
            dec.reset();
            dec.snapshot()
        } else {
            // No live process — emit a blank snapshot at default size.
            let size = *self.inner.size.lock().expect("smart size poisoned");
            AnsiLineDecoder::new(size.cols, size.rows).snapshot()
        };

        self.emit_output_snapshot(app, snapshot);
        Ok(())
    }

    pub fn restart(&self, app: &AppHandle) -> Result<()> {
        self.stop_current();
        *self.inner.cwd.lock().expect("smart cwd poisoned") = self.inner.home.clone();
        self.clear(app)?;
        self.spawn(app)
    }

    pub fn close(&self, app: &AppHandle) -> Result<()> {
        self.stop_current();
        self.emit_exit(app, SmartExitPayload { code: None });
        self.emit_status(app, false);
        Ok(())
    }

    pub fn translate(&self, ask: String, provider: MagiProvider) -> Result<TranslateResult> {
        run_translate_cli(&ask, provider)
    }

    fn spawn(&self, app: &AppHandle) -> Result<()> {
        self.stop_current();

        let size = *self.inner.size.lock().expect("smart size poisoned");
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(size)
            .context("open smart terminal PTY")?;

        let cwd = self.inner.cwd.lock().expect("smart cwd poisoned").clone();
        let mut cmd = CommandBuilder::new(&self.inner.shell);
        cmd.cwd(&cwd);
        // Inherit parent env (HOME / PATH / USER / SHELL etc) so the
        // shell's dotfiles load and child binaries (node etc) resolve.
        // Without this, portable_pty starts the shell with an EMPTY
        // env — bash/zsh tolerate it but the user's setup is missing.
        for (k, v) in std::env::vars() {
            cmd.env(k, v);
        }
        cmd.env("PATH", augmented_path(Some(&self.inner.home)));
        cmd.env_remove("NO_COLOR");
        cmd.env("FORCE_COLOR", "1");
        cmd.env("CLICOLOR_FORCE", "1");
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .context("spawn smart terminal shell")?;
        drop(pair.slave);

        // Capture the shell's pid so the emitter thread can compare
        // it against `master.process_group_leader()` each tick. When
        // they match, the shell is the foreground process group of
        // its controlling tty — i.e. no subprocess (claude, codex,
        // ssh, …) is currently in the foreground. Used to fire the
        // tui-exit event when the user `/exit`s the agent CLI.
        let shell_pid = child.process_id();

        let mut reader = pair.master.try_clone_reader().context("clone PTY reader")?;
        let writer = pair.master.take_writer().context("take PTY writer")?;
        let killer = child.clone_killer();
        let decoder = Arc::new(Mutex::new(AnsiLineDecoder::new(size.cols, size.rows)));
        // Reader thread pushes bytes + sets `dirty`; emitter thread
        // ticks at ~30 fps and snapshots only when dirty. Without this
        // throttle, a noisy zsh hook (autosuggestions, syntax highlight,
        // ls-on-cd) can fire 50+ snapshots in a few hundred ms — each
        // 50 KB JSON over Tauri IPC freezes the UI.
        let dirty = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let state = Arc::clone(&self.inner);
        let decode_handle = Arc::clone(&decoder);
        let dirty_handle = Arc::clone(&dirty);

        thread::Builder::new()
            .name(format!("kota-smart-pty-reader-{}", state.pty_id))
            .spawn(move || {
                let mut buffer = [0u8; 8192];
                loop {
                    if state.generation.load(Ordering::SeqCst) != generation {
                        break;
                    }

                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            // Feed bytes into alacritty Term; emitter
                            // thread snapshots and emits at 30 fps.
                            {
                                let mut dec = match decode_handle.lock() {
                                    Ok(guard) => guard,
                                    Err(poisoned) => {
                                        crate::kota_debug_log(
                                            "[smart] decoder mutex poisoned in reader; recovering",
                                        );
                                        decode_handle.clear_poison();
                                        poisoned.into_inner()
                                    }
                                };
                                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    let _ = dec.push_bytes(&buffer[..n]);
                                }))
                                .is_err()
                                {
                                    let size = state.size.lock().map(|s| *s).unwrap_or(PtySize {
                                        cols: DEFAULT_COLS,
                                        rows: DEFAULT_ROWS,
                                        pixel_width: 0,
                                        pixel_height: 0,
                                    });
                                    crate::kota_debug_log(
                                        "[smart] decoder push panicked; resetting decoder",
                                    );
                                    *dec = AnsiLineDecoder::new(size.cols, size.rows);
                                }
                            }
                            dirty_handle.store(true, Ordering::Release);
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })
            .context("spawn smart terminal reader thread")?;

        // Emitter thread: 30 fps trailing-edge throttle.
        let state = Arc::clone(&self.inner);
        let app_handle = app.clone();
        let decode_handle = Arc::clone(&decoder);
        let dirty_handle = Arc::clone(&dirty);
        thread::Builder::new()
            .name(format!("kota-smart-pty-emitter-{}", state.pty_id))
            .spawn(move || {
                let frame = std::time::Duration::from_millis(33);
                // Foreground-pgrp tracking. `last_fg_is_shell` is None
                // until we've sampled at least once. `saw_non_shell`
                // arms the tui-exit event so we only emit when a
                // non-shell foreground (claude / codex / ssh) has
                // actually been observed and then yielded the tty back
                // — the boot-time None→Some(true) transition (shell at
                // its first prompt) is suppressed.
                let mut last_fg_is_shell: Option<bool> = None;
                let mut saw_non_shell = false;
                loop {
                    if state.generation.load(Ordering::SeqCst) != generation {
                        break;
                    }
                    std::thread::sleep(frame);
                    if dirty_handle.swap(false, Ordering::AcqRel) {
                        let snapshot = {
                            let mut dec = match decode_handle.lock() {
                                Ok(guard) => guard,
                                Err(poisoned) => {
                                    crate::kota_debug_log(
                                        "[smart] decoder mutex poisoned in emitter; recovering",
                                    );
                                    decode_handle.clear_poison();
                                    poisoned.into_inner()
                                }
                            };
                            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                dec.snapshot()
                            })) {
                                Ok(snapshot) => snapshot,
                                Err(_) => {
                                    let size = state.size.lock().map(|s| *s).unwrap_or(PtySize {
                                        cols: DEFAULT_COLS,
                                        rows: DEFAULT_ROWS,
                                        pixel_width: 0,
                                        pixel_height: 0,
                                    });
                                    crate::kota_debug_log(
                                        "[smart] decoder snapshot panicked; resetting decoder",
                                    );
                                    *dec = AnsiLineDecoder::new(size.cols, size.rows);
                                    continue;
                                }
                            }
                        };
                        let _ = app_handle.emit(
                            SMART_OUTPUT_EVENT,
                            SmartOutputEvent {
                                pty_id: state.pty_id.clone(),
                                snapshot,
                            },
                        );
                    }

                    // tcgetpgrp via portable_pty's helper. Brief lock
                    // on `process` — reader/writer threads don't hold
                    // it across syscalls, so contention is negligible.
                    let fg_pid_opt = state
                        .process
                        .lock()
                        .expect("smart process poisoned")
                        .as_ref()
                        .and_then(|p| p.master.process_group_leader());
                    let fg_is_shell_opt: Option<bool> = match (fg_pid_opt, shell_pid) {
                        (Some(fg), Some(sh)) => Some(fg as u32 == sh),
                        _ => None,
                    };
                    if let Some(fg_is_shell) = fg_is_shell_opt {
                        if !fg_is_shell {
                            saw_non_shell = true;
                        }
                        if last_fg_is_shell != Some(fg_is_shell) {
                            last_fg_is_shell = Some(fg_is_shell);
                            if fg_is_shell && saw_non_shell {
                                saw_non_shell = false;
                                let _ = app_handle.emit(
                                    SMART_TUI_EXIT_EVENT,
                                    SmartTuiExitEvent {
                                        pty_id: state.pty_id.clone(),
                                    },
                                );
                            }
                        }
                    }
                }
            })
            .context("spawn smart terminal emitter thread")?;

        let state = Arc::clone(&self.inner);
        let app_handle = app.clone();
        thread::Builder::new()
            .name(format!("kota-smart-pty-wait-{}", state.pty_id))
            .spawn(move || {
                let exit_code = child.wait().ok().and_then(exit_status_code);
                if state.generation.load(Ordering::SeqCst) != generation {
                    return;
                }

                state.process.lock().expect("smart process poisoned").take();
                let _ = app_handle.emit(
                    SMART_EXIT_EVENT,
                    SmartExitEvent {
                        pty_id: state.pty_id.clone(),
                        code: exit_code,
                    },
                );
                let _ = app_handle.emit(
                    SMART_STATUS_EVENT,
                    SmartStatusEvent {
                        pty_id: state.pty_id.clone(),
                        running: false,
                        cwd: display_path(
                            &state.home,
                            &state.cwd.lock().expect("smart cwd poisoned"),
                        ),
                    },
                );
            })
            .context("spawn smart terminal wait thread")?;

        self.inner
            .process
            .lock()
            .expect("smart process poisoned")
            .replace(SmartProcess {
                master: pair.master,
                writer,
                killer,
                decoder,
            });

        self.emit_status(app, true);
        Ok(())
    }

    fn stop_current(&self) {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(mut process) = self
            .inner
            .process
            .lock()
            .expect("smart process poisoned")
            .take()
        {
            let _ = process.killer.kill();
        }
    }

    fn emit_status(&self, app: &AppHandle, running: bool) {
        let cwd = self.inner.cwd.lock().expect("smart cwd poisoned");
        let _ = app.emit(
            SMART_STATUS_EVENT,
            SmartStatusEvent {
                pty_id: self.inner.pty_id.clone(),
                running,
                cwd: display_path(&self.inner.home, &cwd),
            },
        );
    }

    fn emit_output_snapshot(&self, app: &AppHandle, snapshot: GridSnapshot) {
        let _ = app.emit(
            SMART_OUTPUT_EVENT,
            SmartOutputEvent {
                pty_id: self.inner.pty_id.clone(),
                snapshot,
            },
        );
    }

    fn emit_exit(&self, app: &AppHandle, payload: SmartExitPayload) {
        let _ = app.emit(
            SMART_EXIT_EVENT,
            SmartExitEvent {
                pty_id: self.inner.pty_id.clone(),
                code: payload.code,
            },
        );
    }
}

fn resolve_shell(cli: Option<&str>) -> String {
    match cli.map(str::trim).filter(|value| !value.is_empty()) {
        Some("bash") => "/bin/bash".to_string(),
        Some("zsh") => "/bin/zsh".to_string(),
        Some(value) => value.to_string(),
        None => std::env::var("SHELL")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/bin/zsh".to_string()),
    }
}

fn resolve_initial_cwd(home: &Path, raw: Option<&str>) -> PathBuf {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return home.to_path_buf();
    };

    let candidate = expand_home(home, raw).unwrap_or_else(|| {
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            home.join(path)
        }
    });

    candidate
        .canonicalize()
        .ok()
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| home.to_path_buf())
}

fn display_path(home: &Path, cwd: &Path) -> String {
    if cwd == home {
        return "~".to_string();
    }

    if let Ok(relative) = cwd.strip_prefix(home) {
        return format!("~/{}", relative.display());
    }

    cwd.display().to_string()
}

fn resolve_cd_target(home: &Path, cwd: &Path, input: &str) -> Option<PathBuf> {
    let parts = shell_words::split(input).ok()?;
    if parts.first()?.as_str() != "cd" {
        return None;
    }

    if parts.len() > 2 {
        return None;
    }

    let raw_target = parts.get(1).map(String::as_str).unwrap_or("~");
    if raw_target == "-" {
        return None;
    }

    let candidate = expand_home(home, raw_target).unwrap_or_else(|| {
        let path = PathBuf::from(raw_target);
        if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        }
    });

    let resolved = candidate.canonicalize().ok()?;
    resolved.is_dir().then_some(resolved)
}

fn expand_home(home: &Path, raw: &str) -> Option<PathBuf> {
    if raw == "~" {
        return Some(home.to_path_buf());
    }

    raw.strip_prefix("~/").map(|suffix| home.join(suffix))
}

fn exit_status_code(status: portable_pty::ExitStatus) -> Option<i32> {
    if status.signal().is_some() {
        Some(1)
    } else {
        Some(status.exit_code() as i32)
    }
}

fn run_translate_cli(ask: &str, preferred_provider: MagiProvider) -> Result<TranslateResult> {
    let home = dirs::home_dir();
    let env_path = augmented_path(home.as_deref());

    for candidate in cli_candidates(preferred_provider) {
        let bin = resolve_on_augmented_path(candidate.bin, home.as_deref());
        let prompt = build_translate_prompt(ask, candidate.provider);
        let output = Command::new(&bin)
            .args(&candidate.args)
            .arg(prompt.as_str())
            .env("PATH", &env_path)
            .output();

        let output = match output {
            Ok(output) if output.status.success() => output,
            _ => continue,
        };

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            continue;
        }

        if let Some(mut result) = parse_translate_response(&stdout) {
            result.provider = Some(candidate.provider);
            if result.kind == TranslateKind::Escape {
                result.value = candidate.provider.as_str().to_string();
            }
            return Ok(result);
        }
    }

    Err(anyhow!("no Claude or Codex translator succeeded"))
}

#[derive(Debug)]
struct CliCandidate {
    provider: MagiProvider,
    bin: &'static str,
    args: Vec<&'static str>,
}

fn cli_candidates(preferred_provider: MagiProvider) -> Vec<CliCandidate> {
    let claude = CliCandidate {
        provider: MagiProvider::Claude,
        bin: "claude",
        args: vec![
            "-p",
            "--output-format",
            "text",
            "--no-session-persistence",
            "--permission-mode",
            "bypassPermissions",
        ],
    };
    let codex = CliCandidate {
        provider: MagiProvider::Codex,
        bin: "codex",
        args: vec![
            "exec",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "--ignore-rules",
            "--color",
            "never",
        ],
    };

    match preferred_provider {
        MagiProvider::Claude => vec![claude, codex],
        MagiProvider::Codex => vec![codex, claude],
    }
}

fn build_translate_prompt(ask: &str, provider: MagiProvider) -> String {
    read_magi_translate_prompt_template()
        .replace("{{handoff_provider}}", provider.as_str())
        .replace("{{ask}}", ask)
}

fn read_magi_translate_prompt_template() -> String {
    crate::read_system_prompt_template_content(
        MAGI_TRANSLATE_PROMPT_FILE,
        MAGI_TRANSLATE_PROMPT_TEMPLATE,
    )
}

fn parse_translate_response(raw: &str) -> Option<TranslateResult> {
    let json = raw
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap_or(raw)
        .trim();

    serde_json::from_str(json).ok().or_else(|| {
        if json.is_empty() {
            None
        } else {
            Some(TranslateResult {
                kind: TranslateKind::Command,
                value: json.to_string(),
                provider: None,
                hint: None,
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_simple_cd_targets() {
        let temp = std::env::temp_dir().join(format!(
            "kota-smart-pty-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        let home = temp.join("home");
        let work = home.join("work");
        std::fs::create_dir_all(&work).expect("create temp dirs");
        let cwd = home.join("work");
        let next = resolve_cd_target(&home, &cwd, "cd ..").expect("cd target");
        assert_eq!(next, home.canonicalize().expect("canonical home"));
        let _ = std::fs::remove_dir_all(temp);
    }

    #[test]
    fn ignores_complex_cd_sequences() {
        let home = PathBuf::from("/Users/example");
        let cwd = home.join("work");
        assert!(resolve_cd_target(&home, &cwd, "cd foo && ls").is_none());
    }

    #[test]
    fn parses_json_translate_output() {
        let parsed = parse_translate_response("{\"kind\":\"command\",\"value\":\"pwd\"}")
            .expect("translate result");
        assert_eq!(parsed.kind, TranslateKind::Command);
        assert_eq!(parsed.value, "pwd");
        assert_eq!(parsed.provider, None);
    }

    #[test]
    fn orders_magi_candidates_by_preferred_provider() {
        let claude_first = cli_candidates(MagiProvider::Claude);
        assert_eq!(claude_first[0].provider, MagiProvider::Claude);
        assert_eq!(claude_first[1].provider, MagiProvider::Codex);

        let codex_first = cli_candidates(MagiProvider::Codex);
        assert_eq!(codex_first[0].provider, MagiProvider::Codex);
        assert_eq!(codex_first[1].provider, MagiProvider::Claude);
    }

    #[test]
    fn renders_provider_specific_escape_prompt() {
        let prompt = build_translate_prompt("refactor this", MagiProvider::Codex);
        assert!(prompt.contains("\"value\":\"codex\""));
        assert!(prompt.contains("User request: refactor this"));
    }

    #[test]
    fn displays_home_as_tilde() {
        let home = PathBuf::from("/Users/example");
        assert_eq!(display_path(&home, &home), "~");
        assert_eq!(display_path(&home, &home.join("work")), "~/work");
    }

    #[test]
    fn resolves_initial_cwd_from_home_relative_paths() {
        let temp = std::env::temp_dir().join(format!(
            "kota-smart-pty-cwd-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        let home = temp.join("home");
        let work = home.join("work");
        std::fs::create_dir_all(&work).expect("create temp dirs");

        let resolved = resolve_initial_cwd(&home, Some("~/work"));
        assert_eq!(resolved, work.canonicalize().expect("canonical work"));

        let _ = std::fs::remove_dir_all(temp);
    }
}
