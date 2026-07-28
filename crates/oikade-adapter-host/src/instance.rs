use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    os::{
        fd::OwnedFd,
        unix::{fs::PermissionsExt, net::UnixStream as StdUnixStream},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use async_trait::async_trait;
use command_fds::{CommandFdExt, FdMapping};
use oikade_adapter_api as api;
use oikade_core::{BoxError, Runtime};
use oikade_runtime::Component;
use oikade_supervisor::{ProcessSpec, ReadinessProbe, RestartPolicy, State, Supervisor};
use serde_json::Value as JsonValue;
use thiserror::Error;
use tokio::{net::UnixStream, sync::Mutex};

use crate::{
    projection::{ProjectionKeys, RuntimeCommandHandler},
    session::Session,
    state::{StateError, ensure_state_directory, reset_state_directory},
};

mod output;
mod worker;

use output::emit_adapter_output;

const ADAPTER_FD: i32 = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_INTERVAL: Duration = Duration::from_secs(10);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const EVENT_BUFFER: usize = 1024;
const MAX_DIAGNOSTICS: usize = 256;
const MAX_DIAGNOSTIC_MESSAGE: usize = 1024;
const MAX_RESOURCES: usize = 256;
const MAX_RESOURCE_ATTRIBUTES: usize = 32;

#[derive(Debug, Clone)]
pub struct InstanceSpec {
    pub id: String,
    pub adapter_id: String,
    pub adapter_version: Option<String>,
    pub protocol: String,
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub config: JsonValue,
    pub state_dir: PathBuf,
    pub environment: BTreeMap<OsString, OsString>,
    pub request_timeout: Duration,
    pub health_interval: Duration,
    pub health_timeout: Duration,
    pub event_buffer: usize,
    pub restart_policy: RestartPolicy,
}

impl InstanceSpec {
    pub fn new(
        id: impl Into<String>,
        adapter_id: impl Into<String>,
        protocol: impl Into<String>,
        executable: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            id: id.into(),
            adapter_id: adapter_id.into(),
            adapter_version: None,
            protocol: protocol.into(),
            executable: executable.into(),
            args: Vec::new(),
            config: JsonValue::Object(Default::default()),
            state_dir: state_dir.into(),
            environment: BTreeMap::new(),
            request_timeout: REQUEST_TIMEOUT,
            health_interval: HEALTH_INTERVAL,
            health_timeout: HEALTH_TIMEOUT,
            event_buffer: EVENT_BUFFER,
            restart_policy: RestartPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterStatus {
    pub instance_id: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub protocol: String,
    pub state: State,
    pub pid: Option<u32>,
    pub restarts: usize,
    pub healthy: bool,
    pub health_detail: String,
    pub generation: u64,
    pub snapshot_revision: u64,
    pub devices: usize,
    pub diagnostics: Vec<api::Diagnostic>,
    pub resources: Vec<api::Resource>,
    pub last_error: Option<String>,
}

impl AdapterStatus {
    pub fn is_healthy(&self) -> bool {
        self.state == State::Running && self.healthy
    }
}

#[derive(Debug, Error)]
pub enum InstanceError {
    #[error("invalid adapter instance: {0}")]
    Invalid(String),
    #[error(transparent)]
    State(#[from] StateError),
    #[error("adapter executable: {0}")]
    Executable(#[source] std::io::Error),
    #[error(transparent)]
    Supervisor(#[from] oikade_supervisor::SupervisorError),
    #[error("adapter operation: {0}")]
    Operation(String),
}

struct RunState {
    generation: u64,
    snapshot_revision: u64,
    known: ProjectionKeys,
    snapshot: ProjectionKeys,
    diagnostics: Vec<api::Diagnostic>,
    resources: Vec<api::Resource>,
    healthy: bool,
    health_detail: String,
}

struct Run {
    session: Arc<Session>,
    state: Mutex<RunState>,
}

struct Inner {
    runtime: Runtime,
    spec: InstanceSpec,
    prepared: StdMutex<Option<Arc<Session>>>,
    active: Mutex<Option<Arc<Run>>>,
    reported_version: Mutex<Option<String>>,
    transition: Mutex<()>,
    operation: Mutex<()>,
}

pub struct Instance {
    name: String,
    inner: Arc<Inner>,
    supervisor: Supervisor,
}

impl Instance {
    pub fn new(runtime: Runtime, mut spec: InstanceSpec) -> Result<Self, InstanceError> {
        for (name, value) in [
            ("instance ID", spec.id.as_str()),
            ("adapter ID", spec.adapter_id.as_str()),
            ("protocol", spec.protocol.as_str()),
        ] {
            oikade_core::validate_identifier(value)
                .map_err(|error| InstanceError::Invalid(format!("{name}: {error}")))?;
        }
        if spec
            .adapter_version
            .as_ref()
            .is_some_and(|version| version.is_empty() || version.as_str() != version.trim())
        {
            return Err(InstanceError::Invalid(
                "adapter version must be non-empty without surrounding whitespace".to_owned(),
            ));
        }
        if spec.request_timeout.is_zero()
            || spec.health_interval.is_zero()
            || spec.health_timeout.is_zero()
        {
            return Err(InstanceError::Invalid(
                "request and health durations must be positive".to_owned(),
            ));
        }
        if !(1..=oikade_core::MAX_SUBSCRIPTION_BUFFER).contains(&spec.event_buffer) {
            return Err(InstanceError::Invalid(format!(
                "event buffer must be between 1 and {}",
                oikade_core::MAX_SUBSCRIPTION_BUFFER
            )));
        }
        let executable = validate_executable(&spec.executable)?;
        let state_dir = ensure_state_directory(&spec.state_dir, &spec.id)?;
        for reserved in [
            api::TRANSPORT_FD_ENVIRONMENT,
            api::STATE_DIRECTORY_ENVIRONMENT,
        ] {
            if spec.environment.contains_key(OsStr::new(reserved)) {
                return Err(InstanceError::Invalid(format!(
                    "adapter environment variable {reserved:?} is reserved"
                )));
            }
        }
        for (key, value) in &spec.environment {
            if !valid_environment_key(key) || value.as_encoded_bytes().contains(&0) {
                return Err(InstanceError::Invalid(
                    "adapter environment contains an invalid key or NUL byte".to_owned(),
                ));
            }
        }
        spec.executable = executable.clone();
        spec.state_dir = state_dir.clone();
        let inner = Arc::new(Inner {
            runtime: runtime.clone(),
            spec: spec.clone(),
            prepared: StdMutex::new(None),
            active: Mutex::new(None),
            reported_version: Mutex::new(None),
            transition: Mutex::new(()),
            operation: Mutex::new(()),
        });

        let prepared = Arc::clone(&inner);
        let handler = Arc::new(RuntimeCommandHandler { runtime });
        let prepare = Arc::new(move |command: &mut tokio::process::Command| {
            let (parent, child) = StdUnixStream::pair()?;
            parent.set_nonblocking(true)?;
            let session = Session::spawn(UnixStream::from_std(parent)?, handler.clone());
            let child: OwnedFd = child.into();
            command.fd_mappings(vec![FdMapping {
                parent_fd: child,
                child_fd: ADAPTER_FD,
            }])?;
            command.env(api::TRANSPORT_FD_ENVIRONMENT, ADAPTER_FD.to_string());
            *prepared
                .prepared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session);
            Ok(())
        });

        let mut process = ProcessSpec::new(format!("adapter.{}", spec.id), executable);
        process.args = spec.args.clone();
        process.current_dir = spec.executable.parent().map(PathBuf::from);
        process.environment.extend(spec.environment.clone());
        process.environment.insert(
            OsString::from(api::STATE_DIRECTORY_ENVIRONMENT),
            state_dir.as_os_str().to_owned(),
        );
        process.prepare = Some(prepare);
        process.readiness = Some(Arc::new(AdapterReadiness {
            inner: Arc::clone(&inner),
        }));
        process.policy = spec.restart_policy;
        let instance_id = spec.id.clone();
        let adapter_id = spec.adapter_id.clone();
        process.log_sink = Some(Arc::new(move |stream, line| {
            emit_adapter_output(&instance_id, &adapter_id, stream, line);
        }));
        let supervisor = Supervisor::new(process)?;
        Ok(Self {
            name: format!("adapter.{}", spec.id),
            inner,
            supervisor,
        })
    }

    pub async fn start(&self) -> Result<(), InstanceError> {
        self.supervisor.start().await?;
        Ok(())
    }

    pub fn instance_id(&self) -> &str {
        &self.inner.spec.id
    }

    pub async fn stop(&self) -> Result<(), InstanceError> {
        let active = self.inner.active.lock().await.clone();
        if let Some(run) = active {
            let _: Result<api::ShutdownResponse, _> = run
                .session
                .call(
                    api::METHOD_SHUTDOWN,
                    &serde_json::json!({}),
                    self.inner.spec.request_timeout,
                )
                .await;
        }
        self.supervisor.stop(STOP_TIMEOUT).await?;
        self.inner.deactivate(None).await;
        Ok(())
    }

    pub async fn status(&self) -> AdapterStatus {
        let process = self.supervisor.status();
        let version = self
            .inner
            .reported_version
            .lock()
            .await
            .clone()
            .or_else(|| self.inner.spec.adapter_version.clone())
            .unwrap_or_default();
        let active = self.inner.active.lock().await.clone();
        let state = if let Some(active) = active {
            let state = active.state.lock().await;
            Some((
                state.healthy,
                state.health_detail.clone(),
                state.generation,
                state.snapshot_revision,
                state.known.len(),
                state.diagnostics.clone(),
                state.resources.clone(),
            ))
        } else {
            None
        };
        let (healthy, detail, generation, revision, devices, diagnostics, resources) =
            state.unwrap_or_else(|| (false, String::new(), 0, 0, 0, Vec::new(), Vec::new()));
        AdapterStatus {
            instance_id: self.inner.spec.id.clone(),
            adapter_id: self.inner.spec.adapter_id.clone(),
            adapter_version: version,
            protocol: self.inner.spec.protocol.clone(),
            state: process.state,
            pid: process.pid,
            restarts: process.restarts,
            healthy,
            health_detail: detail,
            generation,
            snapshot_revision: revision,
            devices,
            diagnostics,
            resources,
            last_error: process.last_error,
        }
    }

    pub async fn open_commissioning_window(
        &self,
        duration: Duration,
    ) -> Result<api::OpenCommissioningWindowResponse, InstanceError> {
        let _operation = self.inner.operation.lock().await;
        let seconds = u16::try_from(duration.as_secs()).map_err(|_| {
            InstanceError::Operation("commissioning duration is out of range".to_owned())
        })?;
        if duration.subsec_nanos() != 0
            || !(api::MINIMUM_COMMISSIONING_WINDOW_SECONDS
                ..=api::MAXIMUM_COMMISSIONING_WINDOW_SECONDS)
                .contains(&seconds)
        {
            return Err(InstanceError::Operation(
                "commissioning duration must be between 180 and 900 whole seconds".to_owned(),
            ));
        }
        let run = self
            .inner
            .active
            .lock()
            .await
            .clone()
            .ok_or_else(|| InstanceError::Operation("adapter is not running".to_owned()))?;
        let response: api::OpenCommissioningWindowResponse = run
            .session
            .call(
                api::METHOD_OPEN_COMMISSIONING_WINDOW,
                &api::OpenCommissioningWindowRequest {
                    duration_seconds: seconds,
                },
                self.inner.spec.request_timeout,
            )
            .await
            .map_err(|error| InstanceError::Operation(error.to_string()))?;
        validate_commissioning_response(&response, seconds)?;
        Ok(response)
    }

    pub async fn remove_resource(
        &self,
        resource_type: String,
        id: String,
    ) -> Result<Vec<api::Resource>, InstanceError> {
        let _operation = self.inner.operation.lock().await;
        if resource_type.is_empty()
            || resource_type != resource_type.trim()
            || id.is_empty()
            || id != id.trim()
        {
            return Err(InstanceError::Operation(
                "resource type and ID must be non-empty and trimmed".to_owned(),
            ));
        }
        let run = self
            .inner
            .active
            .lock()
            .await
            .clone()
            .ok_or_else(|| InstanceError::Operation("adapter is not running".to_owned()))?;
        let response: api::RemoveResourceResponse = run
            .session
            .call(
                api::METHOD_REMOVE_RESOURCE,
                &api::RemoveResourceRequest { resource_type, id },
                self.inner.spec.request_timeout,
            )
            .await
            .map_err(|error| InstanceError::Operation(error.to_string()))?;
        let resources = validate_resources(response.resources)?;
        run.state.lock().await.resources = resources.clone();
        Ok(resources)
    }

    pub async fn reset_state(&self) -> Result<(), InstanceError> {
        let _operation = self.inner.operation.lock().await;
        if self.inner.active.lock().await.is_none() {
            return Err(InstanceError::Operation(
                "adapter is not running".to_owned(),
            ));
        }
        self.stop().await.map_err(|error| {
            InstanceError::Operation(format!("stop adapter before state reset: {error}"))
        })?;
        if let Err(error) = reset_state_directory(&self.inner.spec.state_dir, &self.inner.spec.id) {
            let restart = self.start().await;
            return Err(InstanceError::Operation(match restart {
                Ok(()) => format!("reset adapter state: {error}"),
                Err(restart) => format!(
                    "reset adapter state: {error}; restart adapter after failed reset: {restart}"
                ),
            }));
        }
        self.start().await.map_err(|error| {
            InstanceError::Operation(format!("restart adapter after state reset: {error}"))
        })?;
        tracing::warn!(adapter_instance = %self.inner.spec.id, "adapter protocol state reset");
        Ok(())
    }
}

#[async_trait]
impl Component for Instance {
    fn name(&self) -> &str {
        &self.name
    }

    async fn start(&self) -> Result<(), BoxError> {
        Instance::start(self)
            .await
            .map_err(|error| Box::new(error) as _)
    }

    async fn stop(&self) -> Result<(), BoxError> {
        Instance::stop(self)
            .await
            .map_err(|error| Box::new(error) as _)
    }
}

struct AdapterReadiness {
    inner: Arc<Inner>,
}

#[async_trait]
impl ReadinessProbe for AdapterReadiness {
    async fn wait_ready(&self, pid: u32) -> Result<(), BoxError> {
        let session = self
            .inner
            .prepared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or("adapter RPC session was not prepared")?;
        session.set_pid(pid);
        let hello = session.wait_hello(self.inner.spec.health_timeout).await?;
        validate_hello(&self.inner.spec, &hello)?;
        *self.inner.reported_version.lock().await = Some(hello.adapter_version);
        let initialized: api::InitializeResponse = session
            .call(
                api::METHOD_INITIALIZE,
                &api::InitializeRequest {
                    api_version: api::VERSION,
                    instance_id: self.inner.spec.id.clone(),
                    config: self.inner.spec.config.clone(),
                },
                self.inner.spec.request_timeout,
            )
            .await?;
        if !initialized.ready {
            return Err(format!("adapter is not ready: {}", initialized.detail).into());
        }
        let states = self
            .inner
            .runtime
            .subscribe(self.inner.spec.event_buffer)
            .await?;
        let topology = self
            .inner
            .runtime
            .subscribe_topology(self.inner.spec.event_buffer)
            .await?;
        let run = Arc::new(Run {
            session,
            state: Mutex::new(RunState {
                generation: 0,
                snapshot_revision: 0,
                known: ProjectionKeys::new(),
                snapshot: ProjectionKeys::new(),
                diagnostics: Vec::new(),
                resources: Vec::new(),
                healthy: false,
                health_detail: String::new(),
            }),
        });
        self.inner.synchronize(&run).await?;
        self.inner.refresh_health(&run).await?;
        self.inner.activate(run, states, topology).await;
        Ok(())
    }
}

fn validate_hello(spec: &InstanceSpec, hello: &api::Hello) -> Result<(), BoxError> {
    if hello.adapter_id != spec.adapter_id {
        return Err("adapter hello ID does not match configured ID".into());
    }
    if hello.adapter_version.is_empty()
        || spec
            .adapter_version
            .as_ref()
            .is_some_and(|version| version != &hello.adapter_version)
    {
        return Err("adapter hello version is empty or does not match".into());
    }
    if hello.min_api_version > api::VERSION
        || hello.max_api_version < api::VERSION
        || !hello.protocols.contains(&spec.protocol)
    {
        return Err("adapter does not support the configured API or protocol".into());
    }
    Ok(())
}

fn validate_executable(path: &Path) -> Result<PathBuf, InstanceError> {
    let path = path.canonicalize().map_err(InstanceError::Executable)?;
    let metadata = fs::metadata(&path).map_err(InstanceError::Executable)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(InstanceError::Invalid(
            "adapter executable must be an executable regular file".to_owned(),
        ));
    }
    Ok(path)
}

fn valid_environment_key(key: &OsStr) -> bool {
    key.to_str().is_some_and(|key| {
        !key.is_empty()
            && key.bytes().enumerate().all(|(index, byte)| {
                byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
            })
    })
}

fn validate_diagnostics(
    diagnostics: Vec<api::Diagnostic>,
) -> Result<Vec<api::Diagnostic>, BoxError> {
    if diagnostics.len() > MAX_DIAGNOSTICS {
        return Err("adapter returned too many diagnostics".into());
    }
    for diagnostic in &diagnostics {
        if !matches!(diagnostic.severity.as_str(), "info" | "warning" | "error")
            || oikade_core::validate_identifier(&diagnostic.code).is_err()
            || (!diagnostic.device_id.is_empty()
                && oikade_core::validate_identifier(&diagnostic.device_id).is_err())
            || (!diagnostic.capability_id.is_empty()
                && oikade_core::validate_identifier(&diagnostic.capability_id).is_err())
            || diagnostic.message.is_empty()
            || diagnostic.message != diagnostic.message.trim()
            || diagnostic.message.len() > MAX_DIAGNOSTIC_MESSAGE
        {
            return Err("adapter returned an invalid diagnostic".into());
        }
    }
    Ok(diagnostics)
}

fn validate_resources(
    mut resources: Vec<api::Resource>,
) -> Result<Vec<api::Resource>, InstanceError> {
    if resources.len() > MAX_RESOURCES {
        return Err(InstanceError::Operation(
            "adapter returned too many resources".to_owned(),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for resource in &resources {
        if resource.resource_type.is_empty()
            || resource.resource_type != resource.resource_type.trim()
            || resource.id.is_empty()
            || resource.id != resource.id.trim()
            || resource.name != resource.name.trim()
            || resource.attributes.len() > MAX_RESOURCE_ATTRIBUTES
            || resource
                .attributes
                .iter()
                .any(|(key, value)| key.is_empty() || key != key.trim() || value != value.trim())
            || !seen.insert((resource.resource_type.clone(), resource.id.clone()))
        {
            return Err(InstanceError::Operation(
                "adapter returned an invalid resource".to_owned(),
            ));
        }
    }
    resources.sort_by(|left, right| {
        (&left.resource_type, &left.id).cmp(&(&right.resource_type, &right.id))
    });
    Ok(resources)
}

fn validate_commissioning_response(
    response: &api::OpenCommissioningWindowResponse,
    expected: u16,
) -> Result<(), InstanceError> {
    if response.duration_seconds != expected
        || !matches!(response.manual_code.len(), 11 | 21)
        || !response
            .manual_code
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        || !response.qr_code.starts_with("MT:")
        || !(4..=512).contains(&response.qr_code.len())
        || response.qr_code != response.qr_code.trim()
    {
        return Err(InstanceError::Operation(
            "adapter returned invalid commissioning data".to_owned(),
        ));
    }
    Ok(())
}

pub struct Inventory {
    instances: Vec<Arc<Instance>>,
}

impl Inventory {
    pub fn new(instances: impl IntoIterator<Item = Arc<Instance>>) -> Self {
        Self {
            instances: instances.into_iter().collect(),
        }
    }

    pub async fn statuses(&self) -> Vec<AdapterStatus> {
        let mut statuses = Vec::with_capacity(self.instances.len());
        for instance in &self.instances {
            statuses.push(instance.status().await);
        }
        statuses.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        statuses
    }
}
