use std::{
    collections::BTreeMap,
    error::Error,
    future::Future,
    sync::{Arc, Mutex as StdMutex},
    time::SystemTime,
};

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock, mpsc};

use crate::{Capability, CapabilityId, Device, DeviceId, Value};

mod subscription;

pub use subscription::{DeviceState, Snapshot, Subscription, TopologySubscription};
use subscription::{
    close_all, dispatch_events, dispatch_topology, ensure_started, validate_capability_value,
};

pub const MAX_SUBSCRIPTION_BUFFER: usize = 4096;

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("core runtime is stopped")]
    Stopped,
    #[error("core runtime is already started")]
    AlreadyStarted,
    #[error("device already exists: {0}")]
    AlreadyExists(String),
    #[error("device or capability not found: {0}")]
    NotFound(String),
    #[error("capability is not readable: {0}")]
    NotReadable(String),
    #[error("capability is not writable: {0}")]
    NotWritable(String),
    #[error("invalid capability value: {0}")]
    InvalidValue(String),
    #[error("event subscriber is too slow")]
    SlowConsumer,
    #[error("subscription buffer must be between 1 and {MAX_SUBSCRIPTION_BUFFER}")]
    InvalidSubscriptionBuffer,
    #[error("writable device has no command handler: {0}")]
    MissingCommandHandler(String),
    #[error("restore device {device}: {message}")]
    Restore { device: String, message: String },
    #[error("persist state for {device}/{capability}: {message}")]
    Persist {
        device: String,
        capability: String,
        message: String,
    },
    #[error("handle command for {device}/{capability}: {message}")]
    Command {
        device: String,
        capability: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub device_id: DeviceId,
    pub capability_id: CapabilityId,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub device_id: DeviceId,
    pub capability_id: CapabilityId,
    pub value: Value,
    pub revision: u64,
    pub occurred_at: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopologyEvent {
    pub revision: u64,
}

#[async_trait]
pub trait StateStore: Send + Sync {
    async fn load_device(
        &self,
        device_id: &DeviceId,
    ) -> Result<Option<BTreeMap<CapabilityId, Value>>, BoxError>;

    async fn save_device(
        &self,
        device_id: &DeviceId,
        values: &BTreeMap<CapabilityId, Value>,
    ) -> Result<(), BoxError>;
}

#[async_trait]
pub trait CommandHandler: Send + Sync {
    async fn handle_command(&self, command: Command) -> Result<Value, BoxError>;
}

#[async_trait]
impl<F, Fut> CommandHandler for F
where
    F: Fn(Command) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Value, BoxError>> + Send,
{
    async fn handle_command(&self, command: Command) -> Result<Value, BoxError> {
        self(command).await
    }
}

struct DeviceValues {
    current: BTreeMap<CapabilityId, Value>,
    persisted: BTreeMap<CapabilityId, Value>,
}

struct RegisteredDevice {
    definition: Device,
    capabilities: BTreeMap<CapabilityId, Capability>,
    values: Mutex<DeviceValues>,
    handler: Option<Arc<dyn CommandHandler>>,
    command_serial: Mutex<()>,
}

struct SubscriptionState<T> {
    sender: mpsc::Sender<T>,
    terminal: Arc<StdMutex<Option<RuntimeError>>>,
}

struct RuntimeState {
    started: bool,
    devices: BTreeMap<DeviceId, Arc<RegisteredDevice>>,
    subscriptions: BTreeMap<u64, SubscriptionState<Event>>,
    next_subscriber: u64,
    topology_subscriptions: BTreeMap<u64, SubscriptionState<TopologyEvent>>,
    next_topology_subscriber: u64,
    revision: u64,
}

struct RuntimeInner {
    lifecycle: RwLock<()>,
    state: Mutex<RuntimeState>,
    store: Option<Arc<dyn StateStore>>,
}

#[derive(Clone)]
pub struct Runtime {
    inner: Arc<RuntimeInner>,
}

impl Runtime {
    pub fn new(store: Option<Arc<dyn StateStore>>) -> Self {
        Self {
            inner: Arc::new(RuntimeInner {
                lifecycle: RwLock::new(()),
                state: Mutex::new(RuntimeState {
                    started: false,
                    devices: BTreeMap::new(),
                    subscriptions: BTreeMap::new(),
                    next_subscriber: 0,
                    topology_subscriptions: BTreeMap::new(),
                    next_topology_subscriber: 0,
                    revision: 0,
                }),
                store,
            }),
        }
    }

    pub async fn start(&self) -> Result<(), RuntimeError> {
        let _lifecycle = self.inner.lifecycle.write().await;
        let mut state = self.inner.state.lock().await;
        if state.started {
            return Err(RuntimeError::AlreadyStarted);
        }
        state.started = true;
        Ok(())
    }

    pub async fn stop(&self) {
        let _lifecycle = self.inner.lifecycle.write().await;
        let mut state = self.inner.state.lock().await;
        if !state.started {
            return;
        }
        state.started = false;
        state.devices.clear();
        close_all(&mut state.subscriptions, RuntimeError::Stopped);
        close_all(&mut state.topology_subscriptions, RuntimeError::Stopped);
    }

    pub async fn register(
        &self,
        device: Device,
        handler: Option<Arc<dyn CommandHandler>>,
    ) -> Result<(), RuntimeError> {
        device
            .validate()
            .map_err(|error| RuntimeError::InvalidValue(error.to_string()))?;
        for capability in &device.capabilities {
            if capability.permissions.write && handler.is_none() {
                return Err(RuntimeError::MissingCommandHandler(format!(
                    "{}/{}",
                    device.id.as_str(),
                    capability.id.as_str()
                )));
            }
        }

        let _lifecycle = self.inner.lifecycle.write().await;
        {
            let state = self.inner.state.lock().await;
            ensure_started(&state)?;
            if state.devices.contains_key(&device.id) {
                return Err(RuntimeError::AlreadyExists(device.id.as_str().to_owned()));
            }
        }

        let loaded = if let Some(store) = &self.inner.store {
            store
                .load_device(&device.id)
                .await
                .map_err(|error| RuntimeError::Restore {
                    device: device.id.as_str().to_owned(),
                    message: error.to_string(),
                })?
        } else {
            None
        };
        let found = loaded.is_some();
        let mut persisted = loaded.unwrap_or_default();
        let mut current = BTreeMap::new();
        let mut dirty = !found;
        let mut capabilities = BTreeMap::new();
        for capability in &device.capabilities {
            let value = if let Some(value) = persisted.get(&capability.id) {
                value.clone()
            } else {
                let value = capability.initial_value.clone();
                persisted.insert(capability.id.clone(), value.clone());
                dirty = true;
                value
            };
            validate_capability_value(capability, &value).map_err(|error| {
                RuntimeError::Restore {
                    device: device.id.as_str().to_owned(),
                    message: error.to_string(),
                }
            })?;
            current.insert(capability.id.clone(), value);
            capabilities.insert(capability.id.clone(), capability.clone());
        }
        if dirty && let Some(store) = &self.inner.store {
            store
                .save_device(&device.id, &persisted)
                .await
                .map_err(|error| RuntimeError::Persist {
                    device: device.id.as_str().to_owned(),
                    capability: "initial-state".to_owned(),
                    message: error.to_string(),
                })?;
        }

        let mut state = self.inner.state.lock().await;
        ensure_started(&state)?;
        if state.devices.contains_key(&device.id) {
            return Err(RuntimeError::AlreadyExists(device.id.as_str().to_owned()));
        }
        let device_id = device.id.clone();
        state.devices.insert(
            device_id,
            Arc::new(RegisteredDevice {
                definition: device,
                capabilities,
                values: Mutex::new(DeviceValues { current, persisted }),
                handler,
                command_serial: Mutex::new(()),
            }),
        );
        state.revision += 1;
        let revision = state.revision;
        dispatch_topology(&mut state, TopologyEvent { revision });
        Ok(())
    }

    pub async fn unregister(&self, device_id: &DeviceId) -> Result<(), RuntimeError> {
        let _lifecycle = self.inner.lifecycle.write().await;
        let registered =
            {
                let state = self.inner.state.lock().await;
                ensure_started(&state)?;
                state.devices.get(device_id).cloned().ok_or_else(|| {
                    RuntimeError::NotFound(format!("device {}", device_id.as_str()))
                })?
            };
        let _command = registered.command_serial.lock().await;
        let mut state = self.inner.state.lock().await;
        if state
            .devices
            .get(device_id)
            .is_none_or(|current| !Arc::ptr_eq(current, &registered))
        {
            return Err(RuntimeError::NotFound(format!(
                "device {}",
                device_id.as_str()
            )));
        }
        state.devices.remove(device_id);
        state.revision += 1;
        let revision = state.revision;
        dispatch_topology(&mut state, TopologyEvent { revision });
        Ok(())
    }

    pub async fn read(
        &self,
        device_id: &DeviceId,
        capability_id: &CapabilityId,
    ) -> Result<Value, RuntimeError> {
        let _lifecycle = self.inner.lifecycle.read().await;
        let (registered, capability) = self.lookup(device_id, capability_id).await?;
        if !capability.permissions.read {
            return Err(RuntimeError::NotReadable(format!(
                "{}/{}",
                device_id.as_str(),
                capability_id.as_str()
            )));
        }
        let values = registered.values.lock().await;
        values.current.get(capability_id).cloned().ok_or_else(|| {
            RuntimeError::NotFound(format!(
                "capability {}/{}",
                device_id.as_str(),
                capability_id.as_str()
            ))
        })
    }

    pub async fn write(&self, command: Command) -> Result<(), RuntimeError> {
        self.write_value(command).await.map(|_| ())
    }

    pub async fn write_value(&self, command: Command) -> Result<Value, RuntimeError> {
        let _lifecycle = self.inner.lifecycle.read().await;
        let (registered, capability) = self
            .lookup(&command.device_id, &command.capability_id)
            .await?;
        if !capability.permissions.write {
            return Err(RuntimeError::NotWritable(format!(
                "{}/{}",
                command.device_id.as_str(),
                command.capability_id.as_str()
            )));
        }
        validate_capability_value(&capability, &command.value)?;
        let _command_serial = registered.command_serial.lock().await;
        self.confirm_registration(&command.device_id, &registered)
            .await?;
        let handler = registered.handler.as_ref().ok_or_else(|| {
            RuntimeError::MissingCommandHandler(format!(
                "{}/{}",
                command.device_id.as_str(),
                command.capability_id.as_str()
            ))
        })?;
        let effective = handler
            .handle_command(command.clone())
            .await
            .map_err(|error| RuntimeError::Command {
                device: command.device_id.as_str().to_owned(),
                capability: command.capability_id.as_str().to_owned(),
                message: error.to_string(),
            })?;
        validate_capability_value(&capability, &effective).map_err(|error| {
            RuntimeError::InvalidValue(format!("integration returned invalid value: {error}"))
        })?;
        self.commit(
            &registered,
            &command.device_id,
            &capability,
            effective.clone(),
        )
        .await?;
        Ok(effective)
    }

    pub async fn publish(
        &self,
        device_id: &DeviceId,
        capability_id: &CapabilityId,
        value: Value,
    ) -> Result<(), RuntimeError> {
        let _lifecycle = self.inner.lifecycle.read().await;
        let (registered, capability) = self.lookup(device_id, capability_id).await?;
        validate_capability_value(&capability, &value)?;
        let _command_serial = registered.command_serial.lock().await;
        self.confirm_registration(device_id, &registered).await?;
        self.commit(&registered, device_id, &capability, value)
            .await
    }

    pub async fn subscribe(&self, buffer: usize) -> Result<Subscription, RuntimeError> {
        if !(1..=MAX_SUBSCRIPTION_BUFFER).contains(&buffer) {
            return Err(RuntimeError::InvalidSubscriptionBuffer);
        }
        let _lifecycle = self.inner.lifecycle.read().await;
        let mut state = self.inner.state.lock().await;
        ensure_started(&state)?;
        state.next_subscriber += 1;
        let id = state.next_subscriber;
        let (sender, receiver) = mpsc::channel(buffer);
        let terminal = Arc::new(StdMutex::new(None));
        state.subscriptions.insert(
            id,
            SubscriptionState {
                sender,
                terminal: Arc::clone(&terminal),
            },
        );
        Ok(Subscription {
            id,
            receiver,
            terminal,
            runtime: Arc::downgrade(&self.inner),
        })
    }

    pub async fn subscribe_topology(
        &self,
        buffer: usize,
    ) -> Result<TopologySubscription, RuntimeError> {
        if !(1..=MAX_SUBSCRIPTION_BUFFER).contains(&buffer) {
            return Err(RuntimeError::InvalidSubscriptionBuffer);
        }
        let _lifecycle = self.inner.lifecycle.read().await;
        let mut state = self.inner.state.lock().await;
        ensure_started(&state)?;
        state.next_topology_subscriber += 1;
        let id = state.next_topology_subscriber;
        let (sender, receiver) = mpsc::channel(buffer);
        let terminal = Arc::new(StdMutex::new(None));
        state.topology_subscriptions.insert(
            id,
            SubscriptionState {
                sender,
                terminal: Arc::clone(&terminal),
            },
        );
        Ok(TopologySubscription {
            id,
            receiver,
            terminal,
            runtime: Arc::downgrade(&self.inner),
        })
    }

    pub async fn snapshot(&self) -> Snapshot {
        let (started, revision, subscriptions, devices) = {
            let state = self.inner.state.lock().await;
            (
                state.started,
                state.revision,
                state.subscriptions.len(),
                state.devices.values().cloned().collect::<Vec<_>>(),
            )
        };
        let mut device_states = Vec::with_capacity(devices.len());
        for registered in devices {
            let values = registered.values.lock().await;
            device_states.push(DeviceState {
                device: registered.definition.clone(),
                values: values.current.clone(),
            });
        }
        Snapshot {
            started,
            revision,
            devices: device_states,
            subscriptions,
        }
    }

    async fn lookup(
        &self,
        device_id: &DeviceId,
        capability_id: &CapabilityId,
    ) -> Result<(Arc<RegisteredDevice>, Capability), RuntimeError> {
        let state = self.inner.state.lock().await;
        ensure_started(&state)?;
        let registered = state
            .devices
            .get(device_id)
            .cloned()
            .ok_or_else(|| RuntimeError::NotFound(format!("device {}", device_id.as_str())))?;
        let capability = registered
            .capabilities
            .get(capability_id)
            .cloned()
            .ok_or_else(|| {
                RuntimeError::NotFound(format!(
                    "capability {}/{}",
                    device_id.as_str(),
                    capability_id.as_str()
                ))
            })?;
        Ok((registered, capability))
    }

    async fn confirm_registration(
        &self,
        device_id: &DeviceId,
        registered: &Arc<RegisteredDevice>,
    ) -> Result<(), RuntimeError> {
        let state = self.inner.state.lock().await;
        ensure_started(&state)?;
        if state
            .devices
            .get(device_id)
            .is_none_or(|current| !Arc::ptr_eq(current, registered))
        {
            return Err(RuntimeError::NotFound(format!(
                "device {}",
                device_id.as_str()
            )));
        }
        Ok(())
    }

    async fn commit(
        &self,
        registered: &Arc<RegisteredDevice>,
        device_id: &DeviceId,
        capability: &Capability,
        value: Value,
    ) -> Result<(), RuntimeError> {
        let mut values = registered.values.lock().await;
        if values.current.get(&capability.id) == Some(&value) {
            return Ok(());
        }
        let mut next_persisted = values.persisted.clone();
        next_persisted.insert(capability.id.clone(), value.clone());
        if let Some(store) = &self.inner.store {
            store
                .save_device(device_id, &next_persisted)
                .await
                .map_err(|error| RuntimeError::Persist {
                    device: device_id.as_str().to_owned(),
                    capability: capability.id.as_str().to_owned(),
                    message: error.to_string(),
                })?;
        }
        self.confirm_registration(device_id, registered).await?;
        values.persisted = next_persisted;
        values.current.insert(capability.id.clone(), value.clone());

        let mut state = self.inner.state.lock().await;
        state.revision += 1;
        let revision = state.revision;
        if capability.permissions.observe {
            dispatch_events(
                &mut state,
                Event {
                    device_id: device_id.clone(),
                    capability_id: capability.id.clone(),
                    value,
                    revision,
                    occurred_at: SystemTime::now(),
                },
            );
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
