use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};

use tokio::sync::mpsc;

use super::{Event, RuntimeError, RuntimeInner, RuntimeState, SubscriptionState, TopologyEvent};
use crate::{Capability, CapabilityId, Device, Value, lookup_builtin_capability};

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceState {
    pub device: Device,
    pub values: BTreeMap<CapabilityId, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub started: bool,
    pub revision: u64,
    pub devices: Vec<DeviceState>,
    pub subscriptions: usize,
}

pub struct Subscription {
    pub(super) id: u64,
    pub(super) receiver: mpsc::Receiver<Event>,
    pub(super) terminal: Arc<StdMutex<Option<RuntimeError>>>,
    pub(super) runtime: Weak<RuntimeInner>,
}

impl Subscription {
    pub async fn recv(&mut self) -> Option<Event> {
        self.receiver.recv().await
    }

    pub fn error(&self) -> Option<RuntimeError> {
        self.terminal.lock().ok().and_then(|error| error.clone())
    }

    pub async fn cancel(&mut self) {
        if let Some(runtime) = self.runtime.upgrade() {
            runtime.state.lock().await.subscriptions.remove(&self.id);
        }
        self.receiver.close();
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let id = self.id;
        if let Some(runtime) = self.runtime.upgrade()
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            handle.spawn(async move {
                runtime.state.lock().await.subscriptions.remove(&id);
            });
        }
    }
}

pub struct TopologySubscription {
    pub(super) id: u64,
    pub(super) receiver: mpsc::Receiver<TopologyEvent>,
    pub(super) terminal: Arc<StdMutex<Option<RuntimeError>>>,
    pub(super) runtime: Weak<RuntimeInner>,
}

impl TopologySubscription {
    pub async fn recv(&mut self) -> Option<TopologyEvent> {
        self.receiver.recv().await
    }

    pub fn error(&self) -> Option<RuntimeError> {
        self.terminal.lock().ok().and_then(|error| error.clone())
    }

    pub async fn cancel(&mut self) {
        if let Some(runtime) = self.runtime.upgrade() {
            runtime
                .state
                .lock()
                .await
                .topology_subscriptions
                .remove(&self.id);
        }
        self.receiver.close();
    }
}

impl Drop for TopologySubscription {
    fn drop(&mut self) {
        let id = self.id;
        if let Some(runtime) = self.runtime.upgrade()
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            handle.spawn(async move {
                runtime
                    .state
                    .lock()
                    .await
                    .topology_subscriptions
                    .remove(&id);
            });
        }
    }
}

pub(super) fn ensure_started(state: &RuntimeState) -> Result<(), RuntimeError> {
    if state.started {
        Ok(())
    } else {
        Err(RuntimeError::Stopped)
    }
}

pub(super) fn validate_capability_value(
    capability: &Capability,
    value: &Value,
) -> Result<(), RuntimeError> {
    if let Some(builtin) = lookup_builtin_capability(&capability.capability_type) {
        return builtin
            .validate_value(value)
            .map_err(|error| RuntimeError::InvalidValue(error.to_string()));
    }
    value
        .validate()
        .map_err(|error| RuntimeError::InvalidValue(error.to_string()))?;
    if value.kind() != capability.kind {
        return Err(RuntimeError::InvalidValue(format!(
            "capability {:?} requires {:?}, got {:?}",
            capability.id.as_str(),
            capability.kind.as_str(),
            value.kind().as_str()
        )));
    }
    Ok(())
}

pub(super) fn dispatch_events(state: &mut RuntimeState, event: Event) {
    let mut closed = Vec::new();
    for (id, subscription) in &state.subscriptions {
        match subscription.sender.try_send(event.clone()) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                set_terminal(&subscription.terminal, RuntimeError::SlowConsumer);
                closed.push(*id);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => closed.push(*id),
        }
    }
    for id in closed {
        state.subscriptions.remove(&id);
    }
}

pub(super) fn dispatch_topology(state: &mut RuntimeState, event: TopologyEvent) {
    let mut closed = Vec::new();
    for (id, subscription) in &state.topology_subscriptions {
        match subscription.sender.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                set_terminal(&subscription.terminal, RuntimeError::SlowConsumer);
                closed.push(*id);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => closed.push(*id),
        }
    }
    for id in closed {
        state.topology_subscriptions.remove(&id);
    }
}

pub(super) fn close_all<T>(
    subscriptions: &mut BTreeMap<u64, SubscriptionState<T>>,
    error: RuntimeError,
) {
    for subscription in subscriptions.values() {
        set_terminal(&subscription.terminal, error.clone());
    }
    subscriptions.clear();
}

fn set_terminal(terminal: &StdMutex<Option<RuntimeError>>, error: RuntimeError) {
    if let Ok(mut terminal) = terminal.lock() {
        *terminal = Some(error);
    }
}
