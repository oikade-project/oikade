//! Supervision for isolated Oikade child processes.

use std::{
    collections::BTreeMap,
    error::Error,
    ffi::{OsStr, OsString},
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use thiserror::Error;
use tokio::{
    process::Command,
    sync::{broadcast, oneshot, watch},
    time::timeout,
};

mod process;

pub use process::kill_process_group;
use process::run;

pub const MAX_LOG_LINE: usize = 16 << 10;
const EVENT_BUFFER: usize = 32;

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;
pub type Prepare = Arc<dyn Fn(&mut Command) -> Result<(), BoxError> + Send + Sync>;
pub type LogSink = Arc<dyn Fn(LogStream, &str) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    Starting,
    Running,
    Backoff,
    Quarantined,
}

impl State {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Backoff => "backoff",
            Self::Quarantined => "quarantined",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub name: String,
    pub state: State,
    pub pid: Option<u32>,
    pub restarts: usize,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub at: SystemTime,
    pub status: Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub max_restarts: usize,
    pub restart_window: Duration,
    pub stable_after: Duration,
    pub ready_timeout: Duration,
    pub stop_timeout: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            initial_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(30),
            max_restarts: 5,
            restart_window: Duration::from_secs(60),
            stable_after: Duration::from_secs(60),
            ready_timeout: Duration::from_secs(10),
            stop_timeout: Duration::from_secs(5),
        }
    }
}

impl RestartPolicy {
    fn validate(self) -> Result<(), SupervisorError> {
        if self.initial_backoff.is_zero() {
            return Err(SupervisorError::InvalidPolicy(
                "initial restart backoff must be positive".to_owned(),
            ));
        }
        if self.max_backoff < self.initial_backoff {
            return Err(SupervisorError::InvalidPolicy(
                "maximum restart backoff must not be less than the initial backoff".to_owned(),
            ));
        }
        if self.restart_window.is_zero() {
            return Err(SupervisorError::InvalidPolicy(
                "restart window must be positive".to_owned(),
            ));
        }
        if self.stable_after.is_zero() {
            return Err(SupervisorError::InvalidPolicy(
                "stable duration must be positive".to_owned(),
            ));
        }
        if self.ready_timeout.is_zero() {
            return Err(SupervisorError::InvalidPolicy(
                "readiness timeout must be positive".to_owned(),
            ));
        }
        if self.stop_timeout.is_zero() {
            return Err(SupervisorError::InvalidPolicy(
                "stop timeout must be positive".to_owned(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
pub trait ReadinessProbe: Send + Sync {
    async fn wait_ready(&self, pid: u32) -> Result<(), BoxError>;
}

pub struct ProcessSpec {
    pub name: String,
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub current_dir: Option<PathBuf>,
    /// The complete child environment. The supervisor never inherits implicitly.
    pub environment: BTreeMap<OsString, OsString>,
    pub prepare: Option<Prepare>,
    pub readiness: Option<Arc<dyn ReadinessProbe>>,
    pub log_sink: Option<LogSink>,
    pub policy: RestartPolicy,
}

impl ProcessSpec {
    pub fn new(name: impl Into<String>, executable: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            executable: executable.into(),
            args: Vec::new(),
            current_dir: None,
            environment: sanitized_environment(std::env::vars_os()),
            prepare: None,
            readiness: None,
            log_sink: None,
            policy: RestartPolicy::default(),
        }
    }
}

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("process name is required")]
    MissingName,
    #[error("process executable is required")]
    MissingExecutable,
    #[error("supervisor is already started")]
    AlreadyStarted,
    #[error("invalid restart policy: {0}")]
    InvalidPolicy(String),
    #[error("process {name:?} did not become ready: {message}")]
    InitialFailure { name: String, message: String },
    #[error("stop process {name:?} timed out")]
    StopTimeout { name: String },
}

struct Shared {
    status: Status,
    running: bool,
    cancel: Option<watch::Sender<bool>>,
    done: Option<watch::Receiver<bool>>,
}

pub struct Supervisor {
    spec: Arc<ProcessSpec>,
    shared: Arc<StdMutex<Shared>>,
    events: broadcast::Sender<Event>,
}

impl Supervisor {
    pub fn new(spec: ProcessSpec) -> Result<Self, SupervisorError> {
        if spec.name.is_empty() {
            return Err(SupervisorError::MissingName);
        }
        if spec.executable.as_os_str().is_empty() {
            return Err(SupervisorError::MissingExecutable);
        }
        spec.policy.validate()?;
        let status = Status {
            name: spec.name.clone(),
            state: State::Stopped,
            pid: None,
            restarts: 0,
            last_error: None,
        };
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Ok(Self {
            spec: Arc::new(spec),
            shared: Arc::new(StdMutex::new(Shared {
                status,
                running: false,
                cancel: None,
                done: None,
            })),
            events,
        })
    }

    pub fn status(&self) -> Status {
        lock_shared(&self.shared).status.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub async fn start(&self) -> Result<(), SupervisorError> {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (done_tx, done_rx) = watch::channel(false);
        let (ready_tx, ready_rx) = oneshot::channel();
        {
            let mut shared = lock_shared(&self.shared);
            if shared.running {
                return Err(SupervisorError::AlreadyStarted);
            }
            shared.running = true;
            shared.cancel = Some(cancel_tx);
            shared.done = Some(done_rx.clone());
        }
        let spec = Arc::clone(&self.spec);
        let shared = Arc::clone(&self.shared);
        let events = self.events.clone();
        tokio::spawn(async move {
            run(spec, shared, events, cancel_rx, ready_tx).await;
            let _ = done_tx.send(true);
        });

        match ready_rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => {
                wait_done(done_rx).await;
                Err(SupervisorError::InitialFailure {
                    name: self.spec.name.clone(),
                    message,
                })
            }
            Err(_) => {
                wait_done(done_rx).await;
                Err(SupervisorError::InitialFailure {
                    name: self.spec.name.clone(),
                    message: "supervisor ended before readiness".to_owned(),
                })
            }
        }
    }

    pub async fn stop(&self, deadline: Duration) -> Result<(), SupervisorError> {
        let mut done = {
            let shared = lock_shared(&self.shared);
            if !shared.running {
                return Ok(());
            }
            if let Some(cancel) = &shared.cancel {
                let _ = cancel.send(true);
            }
            shared.done.clone()
        };
        let Some(ref mut done) = done else {
            return Ok(());
        };
        timeout(deadline, wait_done(done.clone()))
            .await
            .map_err(|_| SupervisorError::StopTimeout {
                name: self.spec.name.clone(),
            })?;
        Ok(())
    }
}

async fn wait_done(mut done: watch::Receiver<bool>) {
    while !*done.borrow() {
        if done.changed().await.is_err() {
            return;
        }
    }
}

fn lock_shared(shared: &Arc<StdMutex<Shared>>) -> std::sync::MutexGuard<'_, Shared> {
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Builds the default child environment from a deliberately small allowlist.
pub fn sanitized_environment<I, K, V>(source: I) -> BTreeMap<OsString, OsString>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<OsString>,
    V: Into<OsString>,
{
    source
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .filter(|(key, _)| environment_key_allowed(key))
        .collect()
}

fn environment_key_allowed(key: &OsStr) -> bool {
    matches!(
        key.to_str(),
        Some("PATH" | "LANG" | "TZ" | "TMPDIR" | "TMP" | "TEMP" | "SSL_CERT_FILE" | "SSL_CERT_DIR")
    ) || key.to_str().is_some_and(|key| key.starts_with("LC_"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use tokio::time::sleep;

    struct FileReady(PathBuf);

    #[async_trait]
    impl ReadinessProbe for FileReady {
        async fn wait_ready(&self, _pid: u32) -> Result<(), BoxError> {
            for _ in 0..100 {
                if self.0.exists() {
                    return Ok(());
                }
                sleep(Duration::from_millis(10)).await;
            }
            Err("ready marker not created".into())
        }
    }

    fn shell_spec(script: &str, ready: &std::path::Path) -> ProcessSpec {
        let mut spec = ProcessSpec::new("test", "/bin/sh");
        spec.args = vec![
            "-c".into(),
            script.into(),
            "oikade-test".into(),
            ready.into(),
        ];
        spec.readiness = Some(Arc::new(FileReady(ready.to_path_buf())));
        spec.policy.ready_timeout = Duration::from_secs(2);
        spec.policy.stop_timeout = Duration::from_millis(100);
        spec
    }

    #[test]
    fn environment_is_allowlisted() {
        let environment = sanitized_environment([
            ("PATH", "/bin"),
            ("LC_ALL", "C"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("OIKADE_PLUGIN_RPC_FD", "99"),
        ]);
        assert_eq!(
            environment.get(OsStr::new("PATH")),
            Some(&OsString::from("/bin"))
        );
        assert_eq!(
            environment.get(OsStr::new("LC_ALL")),
            Some(&OsString::from("C"))
        );
        assert!(!environment.contains_key(OsStr::new("AWS_SECRET_ACCESS_KEY")));
        assert!(!environment.contains_key(OsStr::new("OIKADE_PLUGIN_RPC_FD")));
    }

    #[tokio::test]
    async fn start_waits_for_readiness_and_stop_terminates_group() {
        let root = tempfile::tempdir().unwrap();
        let ready = root.path().join("ready");
        let script = "touch \"$1\"; trap 'exit 0' TERM; while :; do sleep 1; done";
        let supervisor = Supervisor::new(shell_spec(script, &ready)).unwrap();
        supervisor.start().await.unwrap();
        assert_eq!(supervisor.status().state, State::Running);
        assert!(supervisor.status().pid.is_some());
        supervisor.stop(Duration::from_secs(2)).await.unwrap();
        assert_eq!(supervisor.status().state, State::Stopped);
    }

    #[tokio::test]
    async fn child_receives_exact_sanitized_environment() {
        let root = tempfile::tempdir().unwrap();
        let ready = root.path().join("ready");
        let result = root.path().join("environment");
        let script = "if [ -z \"${SECRET+x}\" ] && [ \"$SAFE\" = ok ]; then echo clean > \"$2\"; fi; touch \"$1\"; trap 'exit 0' TERM; while :; do sleep 1; done";
        let mut spec = shell_spec(script, &ready);
        spec.args.push(result.clone().into());
        spec.environment.clear();
        spec.environment.insert("SAFE".into(), "ok".into());
        spec.environment.insert("SECRET".into(), "leaked".into());
        spec.environment.remove(OsStr::new("SECRET"));
        let supervisor = Supervisor::new(spec).unwrap();
        supervisor.start().await.unwrap();
        assert_eq!(fs::read_to_string(result).unwrap().trim(), "clean");
        supervisor.stop(Duration::from_secs(2)).await.unwrap();
    }

    #[test]
    fn validates_restart_policy() {
        let mut spec = ProcessSpec::new("test", "/bin/sh");
        spec.policy.stop_timeout = Duration::ZERO;
        assert!(matches!(
            Supervisor::new(spec),
            Err(SupervisorError::InvalidPolicy(_))
        ));
    }
}
