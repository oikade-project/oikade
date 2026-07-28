use std::{future::Future, sync::Arc};

use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("start component {component}: {message}")]
    Start { component: String, message: String },
    #[error("stop component {component}: {message}")]
    Stop { component: String, message: String },
    #[error("component startup failed and rollback also failed: {0}")]
    Rollback(String),
}

#[async_trait]
pub trait Component: Send + Sync {
    fn name(&self) -> &str;
    async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

#[async_trait]
impl Component for oikade_core::Runtime {
    fn name(&self) -> &str {
        "core"
    }

    async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        oikade_core::Runtime::start(self)
            .await
            .map_err(|error| Box::new(error) as _)
    }

    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        oikade_core::Runtime::stop(self).await;
        Ok(())
    }
}

pub struct Daemon {
    components: Vec<Arc<dyn Component>>,
}

impl Daemon {
    pub fn new(components: impl IntoIterator<Item = Arc<dyn Component>>) -> Self {
        Self {
            components: components.into_iter().collect(),
        }
    }

    pub async fn run_until<F>(&self, shutdown: F) -> Result<(), DaemonError>
    where
        F: Future<Output = ()>,
    {
        let mut started = Vec::new();
        for component in &self.components {
            tracing::debug!(component = component.name(), "starting component");
            if let Err(error) = component.start().await {
                let original = DaemonError::Start {
                    component: component.name().to_owned(),
                    message: error.to_string(),
                };
                if let Err(rollback) = stop_reverse(&started).await {
                    return Err(DaemonError::Rollback(format!(
                        "{original}; rollback: {rollback}"
                    )));
                }
                return Err(original);
            }
            started.push(Arc::clone(component));
        }
        tracing::info!(components = started.len(), "runtime started");
        shutdown.await;
        stop_reverse(&started).await?;
        tracing::info!("runtime stopped");
        Ok(())
    }
}

async fn stop_reverse(components: &[Arc<dyn Component>]) -> Result<(), DaemonError> {
    let mut first_error = None;
    for component in components.iter().rev() {
        tracing::debug!(component = component.name(), "stopping component");
        if let Err(error) = component.stop().await
            && first_error.is_none()
        {
            first_error = Some(DaemonError::Stop {
                component: component.name().to_owned(),
                message: error.to_string(),
            });
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct RecordingComponent {
        name: &'static str,
        calls: Arc<Mutex<Vec<String>>>,
        fail_start: bool,
    }

    #[async_trait]
    impl Component for RecordingComponent {
        fn name(&self) -> &str {
            self.name
        }

        async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("start:{}", self.name));
            if self.fail_start {
                Err("start failed".into())
            } else {
                Ok(())
            }
        }

        async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("stop:{}", self.name));
            Ok(())
        }
    }

    #[tokio::test]
    async fn starts_in_order_and_stops_in_reverse() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let components: Vec<Arc<dyn Component>> = ["core", "plugin", "admin"]
            .into_iter()
            .map(|name| {
                Arc::new(RecordingComponent {
                    name,
                    calls: Arc::clone(&calls),
                    fail_start: false,
                }) as Arc<dyn Component>
            })
            .collect();
        Daemon::new(components).run_until(async {}).await.unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            [
                "start:core",
                "start:plugin",
                "start:admin",
                "stop:admin",
                "stop:plugin",
                "stop:core",
            ]
        );
    }

    #[tokio::test]
    async fn rolls_back_started_components_after_failure() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let components: Vec<Arc<dyn Component>> = vec![
            Arc::new(RecordingComponent {
                name: "core",
                calls: Arc::clone(&calls),
                fail_start: false,
            }),
            Arc::new(RecordingComponent {
                name: "plugin",
                calls: Arc::clone(&calls),
                fail_start: true,
            }),
        ];
        assert!(Daemon::new(components).run_until(async {}).await.is_err());
        assert_eq!(
            *calls.lock().unwrap(),
            ["start:core", "start:plugin", "stop:core"]
        );
    }
}
