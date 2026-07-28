use std::sync::Arc;

use oikade_adapter_api as api;
use oikade_core::{BoxError, Subscription, TopologySubscription};
use tokio::time::interval;

use super::{Inner, Run, validate_diagnostics, validate_resources};
use crate::projection::{
    accepted_projection_keys, contains, event_request, projection_keys, sync_request,
};

impl Inner {
    pub(super) async fn activate(
        self: &Arc<Self>,
        run: Arc<Run>,
        states: Subscription,
        topology: TopologySubscription,
    ) {
        let _transition = self.transition.lock().await;
        if let Some(previous) = self.active.lock().await.replace(Arc::clone(&run)) {
            previous.session.fail("adapter session replaced").await;
        }
        let state = run.state.lock().await;
        tracing::info!(
            adapter_instance = %self.spec.id,
            generation = state.generation,
            revision = state.snapshot_revision,
            devices = state.known.len(),
            "adapter ready"
        );
        drop(state);
        let inner = Arc::clone(self);
        tokio::spawn(async move { inner.monitor(run, states, topology).await });
    }

    pub(super) async fn deactivate(&self, expected: Option<&Arc<Run>>) {
        let _transition = self.transition.lock().await;
        let run = {
            let mut active = self.active.lock().await;
            if expected.is_some_and(|expected| {
                active
                    .as_ref()
                    .is_none_or(|active| !Arc::ptr_eq(active, expected))
            }) {
                return;
            }
            active.take()
        };
        if let Some(run) = run {
            run.session.fail("adapter session deactivated").await;
        }
    }

    pub(super) async fn synchronize(&self, run: &Arc<Run>) -> Result<(), BoxError> {
        let generation = run.state.lock().await.generation + 1;
        let request = sync_request(&self.runtime.snapshot().await, generation)?;
        let response: api::SyncResponse = run
            .session
            .call(api::METHOD_SYNC, &request, self.spec.request_timeout)
            .await?;
        if response.generation != request.generation || response.devices != request.devices.len() {
            return Err("adapter acknowledged the wrong sync generation or device count".into());
        }
        let diagnostics = validate_diagnostics(response.diagnostics)?;
        let snapshot = projection_keys(&request.devices);
        let known = accepted_projection_keys(response.projections.as_deref(), &snapshot)?;
        let mut state = run.state.lock().await;
        state.generation = request.generation;
        state.snapshot_revision = request.revision;
        state.known = known;
        state.snapshot = snapshot;
        state.diagnostics = diagnostics;
        Ok(())
    }

    pub(super) async fn refresh_health(&self, run: &Arc<Run>) -> Result<(), BoxError> {
        let health: api::HealthResponse = run
            .session
            .call(
                api::METHOD_HEALTH,
                &serde_json::json!({}),
                self.spec.health_timeout,
            )
            .await?;
        let resources = validate_resources(health.resources)?;
        let mut state = run.state.lock().await;
        state.healthy = health.healthy;
        state.health_detail = health.detail;
        state.resources = resources;
        Ok(())
    }

    async fn monitor(
        self: Arc<Self>,
        run: Arc<Run>,
        mut states: Subscription,
        mut topology: TopologySubscription,
    ) {
        let mut health = interval(self.spec.health_interval);
        health.tick().await;
        loop {
            tokio::select! {
                event = states.recv() => {
                    let Some(event) = event else { break; };
                    let (revision, in_snapshot, accepted) = {
                        let state = run.state.lock().await;
                        (state.snapshot_revision, contains(&state.snapshot, &event), contains(&state.known, &event))
                    };
                    if event.revision <= revision {
                        continue;
                    }
                    if !in_snapshot {
                        if let Err(error) = self.synchronize(&run).await {
                            run.session.fail(format!("synchronize adapter: {error}")).await;
                            break;
                        }
                        continue;
                    }
                    if accepted {
                        let result: Result<api::EventResponse, _> = run.session
                            .call(api::METHOD_EVENT, &event_request(&event), self.spec.request_timeout)
                            .await;
                        if let Err(error) = result {
                            run.session.fail(format!("forward adapter event: {error}")).await;
                            break;
                        }
                    }
                }
                topology_event = topology.recv() => {
                    let Some(topology_event) = topology_event else { break; };
                    if topology_event.revision > run.state.lock().await.snapshot_revision
                        && let Err(error) = self.synchronize(&run).await
                    {
                        run.session.fail(format!("synchronize adapter topology: {error}")).await;
                        break;
                    }
                }
                _ = health.tick() => {
                    if let Err(error) = self.refresh_health(&run).await {
                        run.session.fail(format!("adapter health check failed: {error}")).await;
                        break;
                    }
                }
                _ = run.session.wait_failure() => break,
            }
        }
        states.cancel().await;
        topology.cancel().await;
        self.deactivate(Some(&run)).await;
    }
}
