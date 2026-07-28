use std::{
    collections::BTreeMap,
    os::fd::OwnedFd,
    os::unix::net::UnixStream as StdUnixStream,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, Weak},
    time::Duration,
};

use async_trait::async_trait;
use command_fds::{CommandFdExt, FdMapping};
use oikade_core::{
    BoxError, CapabilityId, Command, CommandHandler, Device, DeviceId, Runtime, RuntimeError, Value,
};
use oikade_plugin_api as api;
use oikade_runtime::Component;
use oikade_supervisor::{LogStream, ProcessSpec, ReadinessProbe, RestartPolicy, State, Supervisor};
use serde_json::Value as JsonValue;
use thiserror::Error;
use tokio::{net::UnixStream, sync::Mutex, time::interval};

use crate::{
    convert::{device_from_api, value_from_api, value_to_api},
    manifest::{Manifest, load_manifest},
    session::{Notification, Session},
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_INTERVAL: Duration = Duration::from_secs(10);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const PLUGIN_FD: i32 = 3;

#[derive(Debug, Clone)]
pub struct InstanceSpec {
    pub id: String,
    pub artifact: PathBuf,
    pub config: JsonValue,
    pub command_timeout: Duration,
    pub health_interval: Duration,
    pub health_timeout: Duration,
    pub restart_policy: RestartPolicy,
}

impl InstanceSpec {
    pub fn new(id: impl Into<String>, artifact: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            artifact: artifact.into(),
            config: JsonValue::Object(Default::default()),
            command_timeout: COMMAND_TIMEOUT,
            health_interval: HEALTH_INTERVAL,
            health_timeout: HEALTH_TIMEOUT,
            restart_policy: RestartPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginStatus {
    pub instance_id: String,
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub artifact: PathBuf,
    pub state: State,
    pub pid: Option<u32>,
    pub restarts: usize,
    pub devices: usize,
    pub last_error: Option<String>,
    pub integration_healthy: bool,
    pub health_detail: String,
}

impl PluginStatus {
    pub fn healthy(&self) -> bool {
        self.state == State::Running && self.integration_healthy
    }
}

#[derive(Debug, Error)]
pub enum InstanceError {
    #[error("invalid plugin instance: {0}")]
    Invalid(String),
    #[error(transparent)]
    Manifest(#[from] crate::manifest::ManifestError),
    #[error(transparent)]
    Supervisor(#[from] oikade_supervisor::SupervisorError),
}

struct Active {
    session: Arc<Session>,
    local_to_global: BTreeMap<String, DeviceId>,
    global_to_local: BTreeMap<DeviceId, String>,
    definitions: BTreeMap<String, Device>,
    registered: Vec<DeviceId>,
    healthy: bool,
    health_detail: String,
}

struct Inner {
    runtime: Runtime,
    spec: InstanceSpec,
    manifest: Manifest,
    prepared: StdMutex<Option<Arc<Session>>>,
    active: Mutex<Option<Active>>,
    transition: Mutex<()>,
}

pub struct Instance {
    name: String,
    inner: Arc<Inner>,
    supervisor: Supervisor,
}

impl Instance {
    pub fn new(runtime: Runtime, spec: InstanceSpec) -> Result<Self, InstanceError> {
        oikade_core::validate_identifier(&spec.id)
            .map_err(|error| InstanceError::Invalid(format!("instance ID: {error}")))?;
        if spec.command_timeout.is_zero()
            || spec.health_interval.is_zero()
            || spec.health_timeout.is_zero()
        {
            return Err(InstanceError::Invalid(
                "command and health durations must be positive".to_owned(),
            ));
        }
        let (manifest, executable) = load_manifest(&spec.artifact)?;
        let inner = Arc::new(Inner {
            runtime,
            spec: spec.clone(),
            manifest: manifest.clone(),
            prepared: StdMutex::new(None),
            active: Mutex::new(None),
            transition: Mutex::new(()),
        });

        let prepared = Arc::clone(&inner);
        let prepare = Arc::new(move |command: &mut tokio::process::Command| {
            let (parent, child) = StdUnixStream::pair()?;
            parent.set_nonblocking(true)?;
            let parent = UnixStream::from_std(parent)?;
            let session = Session::spawn(parent);
            let child: OwnedFd = child.into();
            command.fd_mappings(vec![FdMapping {
                parent_fd: child,
                child_fd: PLUGIN_FD,
            }])?;
            command.env(api::TRANSPORT_FD_ENVIRONMENT, PLUGIN_FD.to_string());
            *prepared
                .prepared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session);
            Ok(())
        });

        let mut process = ProcessSpec::new(format!("plugin.{}", spec.id), &executable);
        process.args = manifest.args.iter().map(Into::into).collect();
        process.current_dir = executable.parent().map(PathBuf::from);
        process.prepare = Some(prepare);
        process.readiness = Some(Arc::new(PluginReadiness {
            inner: Arc::clone(&inner),
        }));
        process.policy = spec.restart_policy;
        let instance_id = spec.id.clone();
        let plugin_id = manifest.id.clone();
        process.log_sink = Some(Arc::new(move |stream, line| {
            if !line.trim().is_empty() {
                tracing::info!(
                    plugin_instance = %instance_id,
                    plugin_id = %plugin_id,
                    stream = match stream { LogStream::Stdout => "stdout", LogStream::Stderr => "stderr" },
                    line,
                    "plugin output"
                );
            }
        }));
        let supervisor = Supervisor::new(process)?;
        Ok(Self {
            name: format!("plugin.{}", spec.id),
            inner,
            supervisor,
        })
    }

    pub async fn status(&self) -> PluginStatus {
        let process = self.supervisor.status();
        let active = self.inner.active.lock().await;
        PluginStatus {
            instance_id: self.inner.spec.id.clone(),
            plugin_id: self.inner.manifest.id.clone(),
            name: self.inner.manifest.name.clone(),
            version: self.inner.manifest.version.clone(),
            api_version: api::VERSION,
            artifact: self.inner.spec.artifact.clone(),
            state: process.state,
            pid: process.pid,
            restarts: process.restarts,
            devices: active.as_ref().map_or(0, |active| active.registered.len()),
            last_error: process.last_error,
            integration_healthy: active.as_ref().is_some_and(|active| active.healthy),
            health_detail: active
                .as_ref()
                .map_or_else(String::new, |active| active.health_detail.clone()),
        }
    }

    pub async fn start(&self) -> Result<(), InstanceError> {
        self.supervisor.start().await.map_err(Into::into)
    }

    pub async fn stop(&self) -> Result<(), InstanceError> {
        self.supervisor.stop(STOP_TIMEOUT).await?;
        self.inner.deactivate(None).await;
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
            .map_err(|error| Box::new(error) as BoxError)
    }

    async fn stop(&self) -> Result<(), BoxError> {
        Instance::stop(self)
            .await
            .map_err(|error| Box::new(error) as BoxError)
    }
}

struct PluginReadiness {
    inner: Arc<Inner>,
}

#[async_trait]
impl ReadinessProbe for PluginReadiness {
    async fn wait_ready(&self, pid: u32) -> Result<(), BoxError> {
        let session = self
            .inner
            .prepared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or("plugin RPC session was not prepared")?;
        session.set_pid(pid);
        let hello = session.wait_hello(self.inner.spec.health_timeout).await?;
        if hello.plugin_id != self.inner.manifest.id {
            return Err(format!(
                "plugin hello ID {:?} does not match manifest ID {:?}",
                hello.plugin_id, self.inner.manifest.id
            )
            .into());
        }
        if hello.plugin_version != self.inner.manifest.version {
            return Err(format!(
                "plugin hello version {:?} does not match manifest version {:?}",
                hello.plugin_version, self.inner.manifest.version
            )
            .into());
        }
        if hello.min_api_version > api::VERSION || hello.max_api_version < api::VERSION {
            return Err(format!(
                "plugin supports API versions {}-{}; core requires {}",
                hello.min_api_version,
                hello.max_api_version,
                api::VERSION
            )
            .into());
        }
        let initialized: api::InitializeResponse = session
            .call(
                api::METHOD_INITIALIZE,
                &api::InitializeRequest {
                    api_version: api::VERSION,
                    instance_id: self.inner.spec.id.clone(),
                    config: self.inner.spec.config.clone(),
                },
                self.inner.spec.command_timeout,
            )
            .await?;
        let definitions = convert_devices(&self.inner.spec.id, initialized.devices)?;
        self.inner.activate(session, definitions).await?;
        Ok(())
    }
}

impl Inner {
    async fn activate(
        self: &Arc<Self>,
        session: Arc<Session>,
        definitions: BTreeMap<String, Device>,
    ) -> Result<(), BoxError> {
        let _transition = self.transition.lock().await;
        self.deactivate_locked(None).await;
        self.install_locked(Arc::clone(&session), definitions)
            .await?;
        let inner = Arc::clone(self);
        tokio::spawn(async move { inner.monitor(session).await });
        Ok(())
    }

    async fn install_locked(
        self: &Arc<Self>,
        session: Arc<Session>,
        definitions: BTreeMap<String, Device>,
    ) -> Result<(), BoxError> {
        let local_to_global = definitions
            .iter()
            .map(|(local, device)| (local.clone(), device.id.clone()))
            .collect::<BTreeMap<_, _>>();
        let global_to_local = local_to_global
            .iter()
            .map(|(local, global)| (global.clone(), local.clone()))
            .collect::<BTreeMap<_, _>>();
        *self.active.lock().await = Some(Active {
            session: Arc::clone(&session),
            local_to_global,
            global_to_local,
            definitions: definitions.clone(),
            registered: Vec::new(),
            healthy: true,
            health_detail: String::new(),
        });

        let handler: Arc<dyn CommandHandler> = Arc::new(PluginCommandHandler {
            inner: Arc::downgrade(self),
        });
        let mut registered = Vec::new();
        for device in definitions.values() {
            if let Err(error) = self
                .runtime
                .register(device.clone(), Some(Arc::clone(&handler)))
                .await
            {
                for id in registered.iter().rev() {
                    let _ = self.runtime.unregister(id).await;
                }
                *self.active.lock().await = None;
                return Err(
                    format!("register plugin device {:?}: {error}", device.id.as_str()).into(),
                );
            }
            registered.push(device.id.clone());
        }
        if let Some(active) = self.active.lock().await.as_mut()
            && Arc::ptr_eq(&active.session, &session)
        {
            active.registered = registered;
        }
        tracing::info!(
            plugin_instance = %self.spec.id,
            devices = definitions.len(),
            "plugin ready"
        );
        Ok(())
    }

    async fn deactivate(self: &Arc<Self>, expected: Option<&Arc<Session>>) {
        let _transition = self.transition.lock().await;
        self.deactivate_locked(expected).await;
    }

    async fn deactivate_locked(&self, expected: Option<&Arc<Session>>) {
        let registered = {
            let mut active = self.active.lock().await;
            if expected.is_some_and(|expected| {
                active
                    .as_ref()
                    .is_none_or(|active| !Arc::ptr_eq(&active.session, expected))
            }) {
                return;
            }
            active
                .take()
                .map_or_else(Vec::new, |active| active.registered)
        };
        for id in registered.iter().rev() {
            if let Err(error) = self.runtime.unregister(id).await
                && !matches!(error, RuntimeError::NotFound(_))
            {
                tracing::warn!(plugin_instance = %self.spec.id, %error, "unregister plugin device");
            }
        }
    }

    async fn monitor(self: Arc<Self>, session: Arc<Session>) {
        let mut health = interval(self.spec.health_interval);
        health.tick().await;
        loop {
            tokio::select! {
                notification = session.next_notification() => {
                    let result = match notification {
                        Some(Notification::Event(event)) => self.publish(&session, event).await,
                        Some(Notification::Reconcile(reconcile)) => self.reconcile(&session, reconcile.devices).await,
                        None => Err("plugin notification stream closed".into()),
                    };
                    if let Err(error) = result {
                        session.fail(format!("invalid plugin notification: {error}")).await;
                        break;
                    }
                }
                _ = health.tick() => {
                    let result: Result<api::HealthResponse, _> = session
                        .call(api::METHOD_HEALTH, &serde_json::json!({}), self.spec.health_timeout)
                        .await;
                    match result {
                        Ok(health) => {
                            let mut active = self.active.lock().await;
                            if let Some(active) = active.as_mut()
                                && Arc::ptr_eq(&active.session, &session)
                            {
                                active.healthy = health.healthy;
                                active.health_detail = health.detail;
                            }
                        }
                        Err(error) => {
                            session.fail(format!("plugin health check failed: {error}")).await;
                            break;
                        }
                    }
                }
                _ = session.wait_failure() => break,
            }
        }
        self.deactivate(Some(&session)).await;
    }

    async fn publish(&self, session: &Arc<Session>, event: api::Event) -> Result<(), BoxError> {
        let device_id = {
            let active = self.active.lock().await;
            let active = active.as_ref().ok_or("plugin is not active")?;
            if !Arc::ptr_eq(&active.session, session) {
                return Err("event came from an inactive plugin session".into());
            }
            active
                .local_to_global
                .get(&event.device_id)
                .cloned()
                .ok_or_else(|| format!("unknown plugin device {:?}", event.device_id))?
        };
        let capability_id = CapabilityId::new(event.capability_id)?;
        self.runtime
            .publish(&device_id, &capability_id, value_from_api(&event.value)?)
            .await?;
        Ok(())
    }

    async fn reconcile(
        self: &Arc<Self>,
        session: &Arc<Session>,
        devices: Vec<api::Device>,
    ) -> Result<(), BoxError> {
        let definitions = convert_devices(&self.spec.id, devices)?;
        let _transition = self.transition.lock().await;
        {
            let active = self.active.lock().await;
            let active = active.as_ref().ok_or("plugin is not active")?;
            if !Arc::ptr_eq(&active.session, session) {
                return Err("reconciliation came from an inactive plugin session".into());
            }
            if active.definitions == definitions {
                return Ok(());
            }
        }
        let previous = {
            let active = self.active.lock().await;
            active
                .as_ref()
                .ok_or("plugin is not active")?
                .definitions
                .clone()
        };
        self.deactivate_locked(Some(session)).await;
        if let Err(error) = self.install_locked(Arc::clone(session), definitions).await {
            if let Err(rollback) = self.install_locked(Arc::clone(session), previous).await {
                return Err(format!(
                    "reconcile plugin devices: {error}; restore previous devices: {rollback}"
                )
                .into());
            }
            return Err(error);
        }
        tracing::info!(plugin_instance = %self.spec.id, "plugin devices reconciled");
        Ok(())
    }

    async fn command(&self, command: Command) -> Result<Value, BoxError> {
        let (session, local_id) = {
            let active = self.active.lock().await;
            let active = active.as_ref().ok_or("plugin is not available")?;
            let local_id = active
                .global_to_local
                .get(&command.device_id)
                .cloned()
                .ok_or("plugin does not own the device")?;
            (Arc::clone(&active.session), local_id)
        };
        let response: api::CommandResponse = session
            .call(
                api::METHOD_COMMAND,
                &api::CommandRequest {
                    device_id: local_id,
                    capability_id: command.capability_id.as_str().to_owned(),
                    value: value_to_api(&command.value),
                },
                self.spec.command_timeout,
            )
            .await?;
        let effective = value_from_api(&response.value)?;
        if effective.kind() != command.value.kind() {
            let error = format!(
                "command response kind {:?} does not match requested kind {:?}",
                effective.kind(),
                command.value.kind()
            );
            session.fail(error.clone()).await;
            return Err(error.into());
        }
        Ok(effective)
    }
}

struct PluginCommandHandler {
    inner: Weak<Inner>,
}

#[async_trait]
impl CommandHandler for PluginCommandHandler {
    async fn handle_command(&self, command: Command) -> Result<Value, BoxError> {
        self.inner
            .upgrade()
            .ok_or("plugin host was dropped")?
            .command(command)
            .await
    }
}

fn convert_devices(
    instance_id: &str,
    devices: Vec<api::Device>,
) -> Result<BTreeMap<String, Device>, BoxError> {
    let mut definitions = BTreeMap::new();
    for device in devices {
        if definitions.contains_key(&device.id) {
            return Err(format!("plugin returned duplicate device ID {:?}", device.id).into());
        }
        definitions.insert(device.id.clone(), device_from_api(instance_id, &device)?);
    }
    Ok(definitions)
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

    pub async fn statuses(&self) -> Vec<PluginStatus> {
        let mut statuses = Vec::with_capacity(self.instances.len());
        for instance in &self.instances {
            statuses.push(instance.status().await);
        }
        statuses.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        statuses
    }
}
