use std::sync::Arc;

use async_trait::async_trait;
use oikade_config::VirtualSwitchConfig;
use oikade_core::{
    BoxError, CAPABILITY_SWITCH_ON, Capability, CapabilityId, CapabilityType, Command,
    CommandHandler, Device, DeviceId, Permissions, Runtime, RuntimeError, Value, ValueKind,
};
use thiserror::Error;

use crate::Component;

#[derive(Debug, Error)]
pub enum VirtualIntegrationError {
    #[error("invalid virtual switch {switch_id}: {message}")]
    Invalid { switch_id: String, message: String },
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

pub struct VirtualIntegration {
    runtime: Runtime,
    devices: Vec<Device>,
}

impl VirtualIntegration {
    pub fn new(
        runtime: Runtime,
        switches: &[VirtualSwitchConfig],
    ) -> Result<Self, VirtualIntegrationError> {
        let mut devices = Vec::with_capacity(switches.len());
        for switch in switches {
            let device = switch_device(switch)?;
            device
                .validate()
                .map_err(|error| VirtualIntegrationError::Invalid {
                    switch_id: switch.id.clone(),
                    message: error.to_string(),
                })?;
            devices.push(device);
        }
        Ok(Self { runtime, devices })
    }
}

#[async_trait]
impl Component for VirtualIntegration {
    fn name(&self) -> &str {
        "builtin.virtual"
    }

    async fn start(&self) -> Result<(), BoxError> {
        let handler: Arc<dyn CommandHandler> =
            Arc::new(|command: Command| async move { Ok::<Value, BoxError>(command.value) });
        let mut registered = Vec::new();
        for device in &self.devices {
            if let Err(error) = self
                .runtime
                .register(device.clone(), Some(Arc::clone(&handler)))
                .await
            {
                for device_id in registered.iter().rev() {
                    let _ = self.runtime.unregister(device_id).await;
                }
                return Err(Box::new(error));
            }
            registered.push(device.id.clone());
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), BoxError> {
        let mut first_error = None;
        for device in self.devices.iter().rev() {
            if let Err(error) = self.runtime.unregister(&device.id).await
                && !matches!(error, RuntimeError::NotFound(_) | RuntimeError::Stopped)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), |error| Err(Box::new(error) as BoxError))
    }
}

fn switch_device(config: &VirtualSwitchConfig) -> Result<Device, VirtualIntegrationError> {
    let invalid = |message: String| VirtualIntegrationError::Invalid {
        switch_id: config.id.clone(),
        message,
    };
    Ok(Device {
        id: DeviceId::new(format!("builtin.virtual.{}", config.id))
            .map_err(|error| invalid(error.to_string()))?,
        name: config.name.clone(),
        manufacturer: "Oikade".to_owned(),
        model: "Virtual Switch".to_owned(),
        capabilities: vec![Capability {
            id: CapabilityId::new("on").map_err(|error| invalid(error.to_string()))?,
            capability_type: CapabilityType::new(CAPABILITY_SWITCH_ON)
                .map_err(|error| invalid(error.to_string()))?,
            name: "On".to_owned(),
            kind: ValueKind::Bool,
            permissions: Permissions {
                read: true,
                write: true,
                observe: true,
            },
            initial_value: Value::Bool(config.initial_on),
        }],
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registers_stable_virtual_identity() {
        let runtime = Runtime::new(None);
        runtime.start().await.unwrap();
        let integration = VirtualIntegration::new(
            runtime.clone(),
            &[VirtualSwitchConfig {
                id: "desk".to_owned(),
                name: "Desk switch".to_owned(),
                initial_on: true,
            }],
        )
        .unwrap();
        integration.start().await.unwrap();
        assert_eq!(
            runtime
                .read(
                    &DeviceId::new("builtin.virtual.desk").unwrap(),
                    &CapabilityId::new("on").unwrap(),
                )
                .await
                .unwrap(),
            Value::Bool(true)
        );
        integration.stop().await.unwrap();
        assert!(runtime.snapshot().await.devices.is_empty());
    }
}
