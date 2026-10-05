// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
use crate::db::now;
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

/// How many console lines are replayed to a client that has just connected.
const HISTORY_LINES: usize = 2_000;
const BROADCAST_CAPACITY: usize = 1_024;
/// How long a graceful stop is given before the process is killed.
const GRACEFUL_STOP_SECS: u64 = 30;
/// Resource samples are taken this often and kept for roughly fifteen minutes.
const SAMPLE_INTERVAL_SECS: u64 = 5;
const SAMPLE_HISTORY: usize = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsoleLine {
    pub stream: &'static str,
    pub line: String,
    pub at: i64,
}

/// Everything the supervisor needs in order to launch one server.
#[derive(Debug, Clone)]
pub struct ServerSpec {
    pub id: String,
    pub binary_path: PathBuf,
    pub working_dir: PathBuf,
    pub args: Vec<String>,
    pub stop_command: String,
    /// 0 means every core on the machine.
    pub cpu_cores: usize,
}

/// Emitted when a server exits without the panel asking it to.
///
/// The supervisor cannot reach `AppState` without a dependency cycle, so it
/// reports lifecycle changes on a channel and the consumer in `main` turns them
/// into audit records.
#[derive(Debug, Clone)]
pub struct Lifecycle {
    pub server_id: String,
    pub exit_code: Option<i32>,
    /// False when the panel asked for the shutdown.
    pub crashed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeInfo {
    pub status: Status,
    pub pid: Option<u32>,
    pub started_at: Option<i64>,
    pub exit_code: Option<i32>,
    /// True when this server was already running when the panel started, so
    /// the panel holds no pipes to it.
    pub reattached: bool,
}

struct InstanceState {
    status: Status,
    reattached: bool,
    stdin: Option<ChildStdin>,
    kill: Option<mpsc::Sender<()>>,
    pid: Option<u32>,
    started_at: Option<i64>,
    exit_code: Option<i32>,
}

pub struct Instance {
    /// Where unexpected exits are announced.
    crashes: Option<mpsc::Sender<Lifecycle>>,
    state: Mutex<InstanceState>,
    history: Mutex<VecDeque<ConsoleLine>>,
    samples: Mutex<VecDeque<crate::metrics::Sample>>,
    tx: broadcast::Sender<ConsoleLine>,
}

impl Instance {
    fn new(crashes: Option<mpsc::Sender<Lifecycle>>) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            crashes,
            state: Mutex::new(InstanceState {
                status: Status::Stopped,
                reattached: false,
                stdin: None,
                kill: None,
                pid: None,
                started_at: None,
                exit_code: None,
            }),
            history: Mutex::new(VecDeque::with_capacity(HISTORY_LINES)),
            samples: Mutex::new(VecDeque::with_capacity(SAMPLE_HISTORY)),
            tx,
        }
    }

    pub async fn runtime(&self) -> RuntimeInfo {
        let s = self.state.lock().await;
        RuntimeInfo {
            status: s.status,
            pid: s.pid,
            started_at: s.started_at,
            exit_code: s.exit_code,
            reattached: s.reattached,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConsoleLine> {
        self.tx.subscribe()
    }

    pub async fn history(&self) -> Vec<ConsoleLine> {
        self.history.lock().await.iter().cloned().collect()
    }

    /// Recorded CPU and memory samples, oldest first.
    pub async fn samples(&self) -> Vec<crate::metrics::Sample> {
        self.samples.lock().await.iter().copied().collect()
    }

    async fn record_sample(&self, sample: crate::metrics::Sample) {
        let mut samples = self.samples.lock().await;
        if samples.len() == SAMPLE_HISTORY {
            samples.pop_front();
        }
        samples.push_back(sample);
    }

    async fn push(&self, stream: &'static str, line: String) {
        let entry = ConsoleLine { stream, line, at: now() };
        {
            let mut history = self.history.lock().await;
            if history.len() == HISTORY_LINES {
                history.pop_front();
            }
            history.push_back(entry.clone());
        }
        // A send error only means nobody is watching the console right now.
        let _ = self.tx.send(entry);
    }

    /// Records CPU and memory every few seconds so the panel can draw history
    /// rather than a single instantaneous number.
    fn sample_process(self: &Arc<Self>, pid: u32) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(SAMPLE_INTERVAL_SECS)).await;

                let still_running = {
                    let state = this.state.lock().await;
                    state.status == Status::Running && state.pid == Some(pid)
                };
                if !still_running {
                    return;
                }

                let Ok((memory_bytes, cpu_percent)) =
                    tokio::task::spawn_blocking(move || crate::metrics::sample_process(pid)).await
                else {
                    return;
                };

                this.record_sample(crate::metrics::Sample {
                    at: now(),
                    cpu_percent,
                    memory_bytes,
                })
                .await;
            }
        });
    }

    async fn set_status(&self, status: Status) {
        self.state.lock().await.status = status;
    }

    pub async fn start(self: &Arc<Self>, spec: &ServerSpec) -> AppResult<()> {
        if !spec.binary_path.is_file() {
            return Err(AppError::BadRequest(format!(
                "server binary not found: {}",
                spec.binary_path.display()
            )));
        }
        if !spec.working_dir.is_dir() {
            return Err(AppError::BadRequest(format!(
                "working directory not found: {}",
                spec.working_dir.display()
            )));
        }

        // Check and claim under a single lock: two concurrent start requests
        // must not both get as far as spawning a process.
        {
            let mut state = self.state.lock().await;
            if matches!(state.status, Status::Running | Status::Starting | Status::Stopping) {
                return Err(AppError::Conflict("server is already running".into()));
            }
            state.status = Status::Starting;
        }

        let mut command = Command::new(&spec.binary_path);
        command
            .args(&spec.args)
            .current_dir(&spec.working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Deliberately NOT kill_on_drop: a Minecraft server must outlive the
            // panel. Updating or restarting the panel should never take a world
            // down with it, so the child is left running and picked up again on
            // the next start.
            .kill_on_drop(false);

        // On Unix the child would otherwise share the panel's process group and
        // receive the same terminal signals, which would defeat the above.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid only detaches the child from the controlling
            // terminal; it touches no shared state in the parent.
            unsafe {
                command.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }

        let spawned = command.spawn();

        let mut child: Child = match spawned {
            Ok(child) => child,
            Err(e) => {
                // Release the slot we just claimed.
                self.set_status(Status::Stopped).await;
                return Err(AppError::BadRequest(format!(
                    "failed to launch {}: {e}",
                    spec.binary_path.display()
                )));
            }
        };

        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (kill_tx, mut kill_rx) = mpsc::channel::<()>(4);

        {
            let mut state = self.state.lock().await;
            state.status = Status::Running;
            state.reattached = false;
            state.stdin = stdin;
            state.kill = Some(kill_tx);
            state.pid = pid;
            state.started_at = Some(now());
            state.exit_code = None;
        }

        self.push(
            "system",
            format!("[panel] started {}", spec.binary_path.display()),
        )
        .await;

        // Core limit is applied to the live process, so it takes effect for
        // every thread the server goes on to create.
        if let Some(pid) = pid {
            match crate::affinity::limit_to_cores(pid, spec.cpu_cores) {
                Ok(()) if spec.cpu_cores > 0 => {
                    self.push(
                        "system",
                        format!("[panel] limited to {} CPU core(s)", spec.cpu_cores),
                    )
                    .await;
                }
                Ok(()) => {}
                Err(e) => {
                    self.push("system", format!("[panel] core limit failed: {e}")).await;
                }
            }

            self.sample_process(pid);
        }

        if let Some(out) = stdout {
            let this = Arc::clone(self);
            tokio::spawn(async move {
                let mut lines = BufReader::new(out).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    this.push("stdout", line).await;
                }
            });
        }
        if let Some(err) = stderr {
            let this = Arc::clone(self);
            tokio::spawn(async move {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    this.push("stderr", line).await;
                }
            });
        }

        let spec_id = spec.id.clone();

        // The monitor owns the child so it can both wait on it and kill it.
        //
        // Exit is detected by polling rather than by awaiting `child.wait()`
        // inside the select: an in-flight `wait()` future holds a mutable
        // borrow of the child for the whole select, which the kill branch would
        // also need. A quarter-second poll is imperceptible here.
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let exit: Option<std::process::ExitStatus> = loop {
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                        match child.try_wait() {
                            Ok(Some(status)) => break Some(status),
                            Ok(None) => {}
                            Err(e) => {
                                tracing::warn!(error = %e, "could not poll child process");
                                break None;
                            }
                        }
                    }
                    killed = kill_rx.recv() => {
                        if killed.is_some() {
                            let _ = child.start_kill();
                        }
                    }
                }
            };

            let code = exit.and_then(|status| status.code());
            let graceful = {
                let mut state = this.state.lock().await;
                let was_stopping = state.status == Status::Stopping;
                state.status = if was_stopping || code == Some(0) {
                    Status::Stopped
                } else {
                    Status::Crashed
                };
                state.stdin = None;
                state.kill = None;
                state.pid = None;
                state.exit_code = code;
                was_stopping || code == Some(0)
            };

            let note = match code {
                Some(c) if graceful => format!("[panel] server exited (code {c})"),
                Some(c) => format!("[panel] server exited unexpectedly (code {c})"),
                None => "[panel] server was terminated".to_string(),
            };
            this.push("system", note).await;

            // Only unexpected exits are reported; a requested stop is already
            // recorded by whoever pressed the button.
            if let Some(tx) = &this.crashes {
                let _ = tx
                    .send(Lifecycle {
                        server_id: spec_id.clone(),
                        exit_code: code,
                        crashed: !graceful,
                    })
                    .await;
            }
        });

        Ok(())
    }

    /// Takes over a server that was already running when the panel started.
    ///
    /// The panel holds no pipes to an adopted process, so console output is
    /// recovered by following the server's own log file, and commands stay
    /// unavailable until it is restarted. That is a deliberate trade: keeping a
    /// world alive across a panel update matters more than keeping the console
    /// wired up for the few minutes until someone restarts it.
    pub async fn adopt(
        self: &Arc<Self>,
        spec: &ServerSpec,
        pid: u32,
        started_at: i64,
    ) -> AppResult<()> {
        {
            let mut state = self.state.lock().await;
            if state.status != Status::Stopped {
                return Err(AppError::Conflict("server is already tracked".into()));
            }
            state.status = Status::Running;
            state.reattached = true;
            state.pid = Some(pid);
            state.started_at = Some(started_at);
            state.stdin = None;
            state.kill = None;
            state.exit_code = None;
        }

        self.push(
            "system",
            format!("[panel] reattached to the running server (pid {pid})"),
        )
        .await;
        self.push(
            "system",
            "[panel] following logs/latest.log; restart the server to regain console input"
                .to_string(),
        )
        .await;

        // The version and protocol numbers are read from the startup banner,
        // and an adopted server printed that long before we arrived. Pull the
        // line out of its log so the overview is not left blank.
        let log = spec.working_dir.join("logs").join("latest.log");
        if let Ok(text) = tokio::fs::read_to_string(&log).await {
            if let Some(banner) = text.lines().take(50).find(|l| l.contains("Starting Pumpkin ")) {
                self.push("stdout", banner.to_string()).await;
            }
        }

        self.follow_log(log);
        self.watch_foreign_process(spec.id.clone(), pid, spec.binary_path.clone());
        self.sample_process(pid);
        Ok(())
    }

    /// Streams new lines from a log file into the console view.
    fn follow_log(self: &Arc<Self>, path: PathBuf) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            use tokio::io::AsyncSeekExt;

            // Start at the end: the backlog is already on disk and replaying it
            // would flood the console with a whole session of history.
            let mut offset = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);

            loop {
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;

                if !this.is_running().await {
                    return;
                }

                let Ok(mut file) = tokio::fs::File::open(&path).await else {
                    continue;
                };
                let size = file.metadata().await.map(|m| m.len()).unwrap_or(0);

                // A shrinking file means the server rotated its log.
                if size < offset {
                    offset = 0;
                }
                if size == offset {
                    continue;
                }
                if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
                    continue;
                }

                let mut lines = BufReader::new(file).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    this.push("stdout", line).await;
                }
                offset = size;
            }
        });
    }

    /// Watches a process the panel did not spawn, since `wait` is unavailable
    /// for anything that is not our own child.
    fn watch_foreign_process(self: &Arc<Self>, server_id: String, pid: u32, binary: PathBuf) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;

                let binary = binary.clone();
                let alive = tokio::task::spawn_blocking(move || {
                    crate::metrics::process_matches(pid, &binary)
                })
                .await
                .unwrap_or(false);

                if alive {
                    continue;
                }

                {
                    let mut state = this.state.lock().await;
                    // Something else may have taken over in the meantime.
                    if state.pid != Some(pid) {
                        return;
                    }
                    state.status = Status::Stopped;
                    state.pid = None;
                    state.reattached = false;
                }

                this.push("system", "[panel] the server has exited".to_string())
                    .await;

                if let Some(tx) = &this.crashes {
                    let _ = tx
                        .send(Lifecycle {
                            server_id,
                            exit_code: None,
                            // Without the pipes the panel cannot tell a clean
                            // shutdown from a crash, so it does not guess.
                            crashed: false,
                        })
                        .await;
                }
                return;
            }
        });
    }

    async fn is_running(&self) -> bool {
        self.state.lock().await.status == Status::Running
    }

    /// Writes a line to the stdin of the server, exactly as if typed in its console.
    pub async fn send_command(&self, command: &str) -> AppResult<()> {
        let mut state = self.state.lock().await;
        if state.status != Status::Running {
            return Err(AppError::Conflict("server is not running".into()));
        }
        let stdin = state
            .stdin
            .as_mut()
            .ok_or_else(|| AppError::Conflict("server stdin is unavailable".into()))?;

        stdin.write_all(format!("{command}\n").as_bytes()).await?;
        stdin.flush().await?;
        drop(state);

        self.push("system", format!("> {command}")).await;
        Ok(())
    }

    /// Asks the server to shut down, then kills it if it overstays the grace period.
    pub async fn stop(self: &Arc<Self>, spec: &ServerSpec) -> AppResult<()> {
        {
            let state = self.state.lock().await;
            if matches!(state.status, Status::Stopped | Status::Crashed) {
                return Err(AppError::Conflict("server is not running".into()));
            }
        }

        // A server the panel spawned is asked politely over stdin. An adopted
        // one has no stdin, so the request goes to the operating system.
        let has_stdin = self.state.lock().await.stdin.is_some();
        if has_stdin {
            let _ = self.send_command(&spec.stop_command).await;
        } else if let Some(pid) = self.state.lock().await.pid {
            match crate::affinity::request_stop(pid) {
                Ok(()) => {
                    self.push(
                        "system",
                        "[panel] asked the adopted server to stop".to_string(),
                    )
                    .await;
                }
                Err(e) => {
                    self.push("system", format!("[panel] could not stop it: {e}")).await;
                    return Err(AppError::BadRequest(e));
                }
            }
        }

        self.set_status(Status::Stopping).await;

        let this = Arc::clone(self);
        tokio::spawn(async move {
            for _ in 0..GRACEFUL_STOP_SECS {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let status = this.state.lock().await.status;
                if matches!(status, Status::Stopped | Status::Crashed) {
                    return;
                }
            }
            this.push(
                "system",
                "[panel] graceful stop timed out, killing process".to_string(),
            )
            .await;
            this.kill().await;
        });

        Ok(())
    }

    /// Terminates the process immediately.
    pub async fn kill(&self) {
        let (sender, pid) = {
            let state = self.state.lock().await;
            (state.kill.clone(), state.pid)
        };

        if let Some(tx) = sender {
            let _ = tx.send(()).await;
            return;
        }

        // Adopted servers have no kill channel, only a process id.
        if let Some(pid) = pid {
            if let Err(e) = crate::affinity::request_stop(pid) {
                tracing::warn!(pid, error = %e, "could not terminate adopted server");
            }
        }
    }

    pub async fn restart(self: &Arc<Self>, spec: &ServerSpec) -> AppResult<()> {
        let running = {
            let state = self.state.lock().await;
            !matches!(state.status, Status::Stopped | Status::Crashed)
        };

        if running {
            self.stop(spec).await?;
            for _ in 0..GRACEFUL_STOP_SECS + 5 {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let status = self.state.lock().await.status;
                if matches!(status, Status::Stopped | Status::Crashed) {
                    break;
                }
            }
        }

        self.start(spec).await
    }
}

/// Owns one [`Instance`] per configured server, for the lifetime of the panel.
#[derive(Default)]
pub struct Supervisor {
    instances: RwLock<HashMap<String, Arc<Instance>>>,
    crashes: Option<mpsc::Sender<Lifecycle>>,
}

impl Supervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a supervisor that reports unexpected exits on `crashes`.
    pub fn with_crash_reporting(crashes: mpsc::Sender<Lifecycle>) -> Self {
        Self {
            instances: RwLock::new(HashMap::new()),
            crashes: Some(crashes),
        }
    }

    pub async fn instance(&self, server_id: &str) -> Arc<Instance> {
        if let Some(existing) = self.instances.read().await.get(server_id) {
            return Arc::clone(existing);
        }
        let mut instances = self.instances.write().await;
        Arc::clone(
            instances
                .entry(server_id.to_string())
                .or_insert_with(|| Arc::new(Instance::new(self.crashes.clone()))),
        )
    }

    pub async fn forget(&self, server_id: &str) {
        if let Some(instance) = self.instances.write().await.remove(server_id) {
            instance.kill().await;
        }
    }
}
