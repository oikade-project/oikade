use super::*;
use crate::{CAPABILITY_LIGHT_LEVEL, CAPABILITY_SWITCH_ON, CapabilityType, Permissions, ValueKind};

fn device_id(value: &str) -> DeviceId {
    DeviceId::new(value).unwrap()
}

fn capability_id(value: &str) -> CapabilityId {
    CapabilityId::new(value).unwrap()
}

fn test_switch() -> Device {
    Device {
        id: device_id("test.switch"),
        name: "Test switch".to_owned(),
        manufacturer: String::new(),
        model: String::new(),
        capabilities: vec![Capability {
            id: capability_id("on"),
            capability_type: CapabilityType::new(CAPABILITY_SWITCH_ON).unwrap(),
            name: "On".to_owned(),
            kind: ValueKind::Bool,
            permissions: Permissions {
                read: true,
                write: true,
                observe: true,
            },
            initial_value: Value::Bool(false),
        }],
    }
}

fn echo_handler() -> Arc<dyn CommandHandler> {
    Arc::new(|command: Command| async move { Ok(command.value) })
}

#[tokio::test]
async fn moves_commands_through_model_and_publishes_events() {
    let runtime = Runtime::new(None);
    runtime.start().await.unwrap();
    runtime
        .register(test_switch(), Some(echo_handler()))
        .await
        .unwrap();
    let mut subscription = runtime.subscribe(2).await.unwrap();
    assert_eq!(
        runtime
            .read(&device_id("test.switch"), &capability_id("on"))
            .await
            .unwrap(),
        Value::Bool(false)
    );
    runtime
        .write(Command {
            device_id: device_id("test.switch"),
            capability_id: capability_id("on"),
            value: Value::Bool(true),
        })
        .await
        .unwrap();
    let event = subscription.recv().await.unwrap();
    assert_eq!(event.value, Value::Bool(true));
    assert_eq!(event.revision, 2);
    assert_ne!(event.occurred_at, SystemTime::UNIX_EPOCH);
    let snapshot = runtime.snapshot().await;
    assert!(snapshot.started);
    assert_eq!(snapshot.devices.len(), 1);
    assert_eq!(snapshot.subscriptions, 1);
}

#[tokio::test]
async fn disconnects_only_the_slow_subscriber() {
    let runtime = Runtime::new(None);
    runtime.start().await.unwrap();
    runtime
        .register(test_switch(), Some(echo_handler()))
        .await
        .unwrap();
    let slow = runtime.subscribe(1).await.unwrap();
    let mut healthy = runtime.subscribe(1).await.unwrap();
    runtime
        .publish(
            &device_id("test.switch"),
            &capability_id("on"),
            Value::Bool(true),
        )
        .await
        .unwrap();
    healthy.recv().await.unwrap();
    runtime
        .publish(
            &device_id("test.switch"),
            &capability_id("on"),
            Value::Bool(false),
        )
        .await
        .unwrap();
    assert_eq!(slow.error(), Some(RuntimeError::SlowConsumer));
    assert_eq!(healthy.recv().await.unwrap().value, Value::Bool(false));
    assert_eq!(runtime.snapshot().await.subscriptions, 1);
}

#[tokio::test]
async fn topology_and_state_share_one_revision_sequence() {
    let runtime = Runtime::new(None);
    runtime.start().await.unwrap();
    let mut topology = runtime.subscribe_topology(2).await.unwrap();
    let device = test_switch();
    runtime
        .register(device.clone(), Some(echo_handler()))
        .await
        .unwrap();
    assert_eq!(topology.recv().await.unwrap().revision, 1);
    runtime
        .write(Command {
            device_id: device.id.clone(),
            capability_id: capability_id("on"),
            value: Value::Bool(true),
        })
        .await
        .unwrap();
    runtime.unregister(&device.id).await.unwrap();
    assert_eq!(topology.recv().await.unwrap().revision, 3);
    assert_eq!(runtime.snapshot().await.revision, 3);
}

#[tokio::test]
async fn rejects_wrong_kinds_and_light_levels() {
    let runtime = Runtime::new(None);
    assert!(matches!(
        runtime
            .read(&device_id("test.switch"), &capability_id("on"))
            .await,
        Err(RuntimeError::Stopped)
    ));
    runtime.start().await.unwrap();
    runtime
        .register(test_switch(), Some(echo_handler()))
        .await
        .unwrap();
    assert!(matches!(
        runtime
            .write(Command {
                device_id: device_id("test.switch"),
                capability_id: capability_id("on"),
                value: Value::String("true".to_owned()),
            })
            .await,
        Err(RuntimeError::InvalidValue(_))
    ));

    let light = Device {
        id: device_id("test.light"),
        name: "Test light".to_owned(),
        manufacturer: String::new(),
        model: String::new(),
        capabilities: vec![Capability {
            id: capability_id("level"),
            capability_type: CapabilityType::new(CAPABILITY_LIGHT_LEVEL).unwrap(),
            name: "Level".to_owned(),
            kind: ValueKind::Number,
            permissions: Permissions {
                read: true,
                write: true,
                observe: true,
            },
            initial_value: Value::Number(50.0),
        }],
    };
    runtime.register(light, Some(echo_handler())).await.unwrap();
    assert!(matches!(
        runtime
            .publish(
                &device_id("test.light"),
                &capability_id("level"),
                Value::Number(100.01),
            )
            .await,
        Err(RuntimeError::InvalidValue(_))
    ));
}
