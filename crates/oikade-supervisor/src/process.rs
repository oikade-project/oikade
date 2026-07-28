use std::{
    collections::VecDeque,
    process::Stdio,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant, SystemTime},
};

use nix::{
    errno::Errno,
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
    sync::{broadcast, oneshot, watch},
    time::{sleep, timeout},
};

use super::{
    BoxError, Event, LogSink, LogStream, MAX_LOG_LINE, ProcessSpec, RestartPolicy, Shared, State,
    Status, lock_shared,
};

pub(super) async fn run(
    spec: Arc<ProcessSpec>,
    shared: Arc<StdMutex<Shared>>,
    events: broadcast::Sender<Event>,
    mut cancelled: watch::Receiver<bool>,
    initial_ready: oneshot::Sender<Result<(), String>>,
) {
    let mut initial_ready = Some(initial_ready);
    let mut restarts = 0usize;
    let mut consecutive_failures = 0u32;
    let mut failures = VecDeque::new();

    loop {
        if *cancelled.borrow() {
            transition(&shared, &events, State::Stopped, None, restarts, None);
            send_initial(
                &mut initial_ready,
                Err("cancelled before readiness".to_owned()),
            );
            finish(&shared);
            return;
        }
        transition(&shared, &events, State::Starting, None, restarts, None);
        let mut child = match launch(&spec) {
            Ok(child) => child,
            Err(error) => {
                initial_failure(
                    &spec,
                    &shared,
                    &events,
                    &mut initial_ready,
                    error.to_string(),
                );
                finish(&shared);
                return;
            }
        };
        let pid = child.id();
        transition(&shared, &events, State::Starting, pid, restarts, None);

        if let Err(failure) = wait_ready(&spec, &mut child, pid, &mut cancelled).await {
            let _ = terminate(&mut child, pid, spec.policy.stop_timeout).await;
            initial_failure(&spec, &shared, &events, &mut initial_ready, failure);
            finish(&shared);
            return;
        }
        transition(&shared, &events, State::Running, pid, restarts, None);
        send_initial(&mut initial_ready, Ok(()));
        let started_at = Instant::now();

        let failure = tokio::select! {
            result = child.wait() => Some(unexpected_exit(result)),
            changed = cancelled.changed() => {
                let _ = changed;
                let termination = terminate(&mut child, pid, spec.policy.stop_timeout).await;
                transition(
                    &shared,
                    &events,
                    State::Stopped,
                    None,
                    restarts,
                    termination.as_ref().err().map(ToString::to_string),
                );
                finish(&shared);
                return;
            }
        };
        let failure = failure.unwrap_or_else(|| "process exited unexpectedly".to_owned());
        if started_at.elapsed() >= spec.policy.stable_after {
            consecutive_failures = 0;
            failures.clear();
        }
        consecutive_failures = consecutive_failures.saturating_add(1);
        let now = Instant::now();
        failures.push_back(now);
        while failures
            .front()
            .is_some_and(|failure| now.duration_since(*failure) > spec.policy.restart_window)
        {
            failures.pop_front();
        }
        if failures.len() > spec.policy.max_restarts {
            transition(
                &shared,
                &events,
                State::Quarantined,
                None,
                restarts,
                Some(failure),
            );
            finish(&shared);
            return;
        }
        let delay = restart_backoff(spec.policy, consecutive_failures);
        transition(
            &shared,
            &events,
            State::Backoff,
            None,
            restarts,
            Some(failure),
        );
        tokio::select! {
            () = sleep(delay) => restarts += 1,
            changed = cancelled.changed() => {
                let _ = changed;
                transition(&shared, &events, State::Stopped, None, restarts, None);
                finish(&shared);
                return;
            }
        }
    }
}

fn launch(spec: &ProcessSpec) -> Result<Child, BoxError> {
    let mut command = Command::new(&spec.executable);
    command
        .args(&spec.args)
        .env_clear()
        .envs(&spec.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    if let Some(current_dir) = &spec.current_dir {
        command.current_dir(current_dir);
    }
    if let Some(prepare) = &spec.prepare {
        prepare(&mut command)?;
    }
    let mut child = command.spawn()?;
    if let Some(stdout) = child.stdout.take() {
        spawn_log_reader(stdout, LogStream::Stdout, spec.log_sink.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_log_reader(stderr, LogStream::Stderr, spec.log_sink.clone());
    }
    Ok(child)
}

async fn wait_ready(
    spec: &ProcessSpec,
    child: &mut Child,
    pid: Option<u32>,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let Some(pid) = pid else {
        return Err("child process has no PID".to_owned());
    };
    let Some(probe) = &spec.readiness else {
        return match child.try_wait() {
            Ok(None) => Ok(()),
            Ok(Some(status)) => Err(format!("process exited before readiness: {status}")),
            Err(error) => Err(format!("inspect child before readiness: {error}")),
        };
    };
    tokio::select! {
        result = timeout(spec.policy.ready_timeout, probe.wait_ready(pid)) => match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(format!("readiness probe: {error}")),
            Err(_) => Err("readiness probe timed out".to_owned()),
        },
        result = child.wait() => Err(unexpected_exit(result)),
        changed = cancelled.changed() => {
            let _ = changed;
            Err("cancelled during readiness".to_owned())
        }
    }
}

async fn terminate(child: &mut Child, pid: Option<u32>, grace: Duration) -> Result<(), BoxError> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    if let Some(pid) = pid {
        signal_group(pid, Signal::SIGTERM)?;
    }
    if timeout(grace, child.wait()).await.is_ok() {
        return Ok(());
    }
    if let Some(pid) = pid {
        signal_group(pid, Signal::SIGKILL)?;
    } else {
        child.kill().await?;
    }
    let _ = child.wait().await?;
    Ok(())
}

fn signal_group(pid: u32, signal: Signal) -> Result<(), Errno> {
    let raw = i32::try_from(pid).map_err(|_| Errno::EINVAL)?;
    match kill(Pid::from_raw(-raw), signal) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Immediately terminates one supervised process group after a protocol or
/// trust-boundary violation. The supervisor observes the exit and applies its
/// configured restart policy.
pub fn kill_process_group(pid: u32) -> Result<(), Errno> {
    signal_group(pid, Signal::SIGKILL)
}

fn spawn_log_reader<R>(reader: R, stream: LogStream, sink: Option<LogSink>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    let truncated = line.len() > MAX_LOG_LINE;
                    line.truncate(MAX_LOG_LINE);
                    while line
                        .last()
                        .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
                    {
                        line.pop();
                    }
                    if let Some(sink) = &sink {
                        let text = String::from_utf8_lossy(&line);
                        if truncated {
                            sink(stream, &format!("{text} [truncated]"));
                        } else {
                            sink(stream, &text);
                        }
                    }
                }
            }
        }
    });
}

fn unexpected_exit(result: std::io::Result<std::process::ExitStatus>) -> String {
    match result {
        Ok(status) if status.success() => {
            "process exited unexpectedly with success status".to_owned()
        }
        Ok(status) => format!("process exited unexpectedly: {status}"),
        Err(error) => format!("wait for process: {error}"),
    }
}

fn restart_backoff(policy: RestartPolicy, consecutive_failures: u32) -> Duration {
    let exponent = consecutive_failures.saturating_sub(1).min(31);
    policy
        .initial_backoff
        .checked_mul(1u32 << exponent)
        .unwrap_or(policy.max_backoff)
        .min(policy.max_backoff)
}

fn transition(
    shared: &Arc<StdMutex<Shared>>,
    events: &broadcast::Sender<Event>,
    state: State,
    pid: Option<u32>,
    restarts: usize,
    error: Option<String>,
) {
    let status = {
        let mut shared = lock_shared(shared);
        shared.status = Status {
            name: shared.status.name.clone(),
            state,
            pid,
            restarts,
            last_error: error,
        };
        shared.status.clone()
    };
    let _ = events.send(Event {
        at: SystemTime::now(),
        status,
    });
}

fn initial_failure(
    spec: &ProcessSpec,
    shared: &Arc<StdMutex<Shared>>,
    events: &broadcast::Sender<Event>,
    initial_ready: &mut Option<oneshot::Sender<Result<(), String>>>,
    message: String,
) {
    transition(
        shared,
        events,
        State::Stopped,
        None,
        0,
        Some(message.clone()),
    );
    send_initial(
        initial_ready,
        Err(format!("process {:?}: {message}", spec.name)),
    );
}

fn send_initial(
    sender: &mut Option<oneshot::Sender<Result<(), String>>>,
    result: Result<(), String>,
) {
    if let Some(sender) = sender.take() {
        let _ = sender.send(result);
    }
}

fn finish(shared: &Arc<StdMutex<Shared>>) {
    let mut shared = lock_shared(shared);
    shared.running = false;
    shared.cancel = None;
}
