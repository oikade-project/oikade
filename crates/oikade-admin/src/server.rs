use std::{
    convert::Infallible,
    fs,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path as AxumPath, Request, State, rejection::JsonRejection},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use bytes::Bytes;
use oikade_adapter_host::{Instance as AdapterInstance, InstanceError as AdapterInstanceError};
use oikade_core::{CapabilityId, Command, DeviceId, Runtime, RuntimeError};
use oikade_plugin_host::Instance as PluginInstance;
use oikade_runtime::Component;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::{
    net::UnixListener,
    sync::{Mutex, oneshot},
    task::JoinHandle,
};

use crate::wire::{
    API_VERSION, Adapter, AdapterDiagnostic, AdapterResource, AdapterResourcesResponse,
    AdaptersResponse, Capability, CommissioningInfo, CommissioningRequest, CommissioningWindow,
    DevicesResponse, ErrorPayload, Plugin, PluginsResponse, ResetRequest, Status, StreamRecord,
    WriteRequest, capability_from_state, device_from_state, event_from_core, value_to_core,
};

mod socket;

use socket::{prepare_parent, remove_socket_if_same, remove_stale_socket, socket_identity};

const MAX_REQUEST_BODY: usize = 64 << 10;
const EVENT_BUFFER: usize = 128;
const MAX_SOCKET_PATH_BYTES: usize = 99;
const STALE_DIAL_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("admin socket path is required")]
    MissingSocket,
    #[error("admin socket path exceeds {MAX_SOCKET_PATH_BYTES} bytes")]
    SocketPathTooLong,
    #[error("admin server is already started")]
    AlreadyStarted,
    #[error("admin socket: {0}")]
    Io(#[from] std::io::Error),
    #[error("admin server task failed: {0}")]
    Task(String),
}

struct Running {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<Result<(), std::io::Error>>,
    identity: SocketIdentity,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

struct ServerState {
    runtime: Runtime,
    build: String,
    plugins: Vec<Arc<PluginInstance>>,
    adapters: Vec<Arc<AdapterInstance>>,
    started: StdMutex<Option<(OffsetDateTime, Instant)>>,
}

pub struct Server {
    socket_path: PathBuf,
    state: Arc<ServerState>,
    running: Mutex<Option<Running>>,
}

impl Server {
    pub fn new(
        runtime: Runtime,
        socket_path: impl Into<PathBuf>,
        plugins: Vec<Arc<PluginInstance>>,
        adapters: Vec<Arc<AdapterInstance>>,
    ) -> Result<Self, ServerError> {
        Self::new_with_build(
            runtime,
            socket_path,
            plugins,
            adapters,
            env!("CARGO_PKG_VERSION"),
        )
    }

    pub fn new_with_build(
        runtime: Runtime,
        socket_path: impl Into<PathBuf>,
        plugins: Vec<Arc<PluginInstance>>,
        adapters: Vec<Arc<AdapterInstance>>,
        build: impl Into<String>,
    ) -> Result<Self, ServerError> {
        let socket_path = socket_path.into();
        if socket_path.as_os_str().is_empty() {
            return Err(ServerError::MissingSocket);
        }
        let socket_path = if socket_path.is_absolute() {
            socket_path
        } else {
            std::env::current_dir()?.join(socket_path)
        };
        if socket_path.as_os_str().as_bytes().len() > MAX_SOCKET_PATH_BYTES {
            return Err(ServerError::SocketPathTooLong);
        }
        Ok(Self {
            socket_path,
            state: Arc::new(ServerState {
                runtime,
                build: build.into(),
                plugins,
                adapters,
                started: StdMutex::new(None),
            }),
            running: Mutex::new(None),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn start(&self) -> Result<(), ServerError> {
        let mut running = self.running.lock().await;
        if running.is_some() {
            return Err(ServerError::AlreadyStarted);
        }
        prepare_parent(&self.socket_path)?;
        remove_stale_socket(&self.socket_path).await?;
        let listener = UnixListener::bind(&self.socket_path)?;
        fs::set_permissions(&self.socket_path, fs::Permissions::from_mode(0o600))?;
        let identity = socket_identity(&self.socket_path)?;
        *self
            .state
            .started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((OffsetDateTime::now_utc(), Instant::now()));
        let (shutdown, wait_shutdown) = oneshot::channel();
        let router = router(Arc::clone(&self.state));
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = wait_shutdown.await;
                })
                .await
        });
        *running = Some(Running {
            shutdown,
            task,
            identity,
        });
        tracing::info!(socket = %self.socket_path.display(), api_version = API_VERSION, "admin API started");
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), ServerError> {
        let Some(running) = self.running.lock().await.take() else {
            return Ok(());
        };
        let _ = running.shutdown.send(());
        match running.task.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error.into()),
            Err(error) => return Err(ServerError::Task(error.to_string())),
        }
        remove_socket_if_same(&self.socket_path, running.identity)?;
        *self
            .state
            .started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        tracing::info!("admin API stopped");
        Ok(())
    }
}

#[async_trait]
impl Component for Server {
    fn name(&self) -> &str {
        "admin"
    }

    async fn start(&self) -> Result<(), oikade_core::BoxError> {
        Server::start(self)
            .await
            .map_err(|error| Box::new(error) as _)
    }

    async fn stop(&self) -> Result<(), oikade_core::BoxError> {
        Server::stop(self)
            .await
            .map_err(|error| Box::new(error) as _)
    }
}

fn router(state: Arc<ServerState>) -> Router {
    Router::new()
        .route("/v1/status", get(get_status))
        .route("/v1/devices", get(get_devices))
        .route("/v1/plugins", get(get_plugins))
        .route("/v1/plugins/{instance}", get(get_plugin))
        .route("/v1/adapters", get(get_adapters))
        .route("/v1/adapters/{instance}", get(get_adapter))
        .route(
            "/v1/adapters/{instance}/commissioning-window",
            get(get_commissioning_info).post(open_commissioning_window),
        )
        .route("/v1/adapters/{instance}/reset", post(reset_adapter_state))
        .route(
            "/v1/adapters/{instance}/resources/{resource_type}/{id}",
            delete(remove_adapter_resource),
        )
        .route(
            "/v1/devices/{device}/capabilities/{capability}",
            get(get_capability).put(put_capability),
        )
        .route("/v1/events", get(get_events))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY))
        .layer(middleware::from_fn(no_store))
        .with_state(state)
}

async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn get_status(State(state): State<Arc<ServerState>>) -> Json<Status> {
    let snapshot = state.runtime.snapshot().await;
    let plugins = plugin_statuses(&state).await;
    let adapters = adapter_statuses(&state).await;
    let unhealthy_plugins = plugins.iter().filter(|plugin| !plugin.healthy).count();
    let unhealthy_adapters = adapters.iter().filter(|adapter| !adapter.healthy).count();
    let (started_at, uptime) = state
        .started
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map(|(started, instant)| (*started, instant.elapsed()))
        .unwrap_or((OffsetDateTime::UNIX_EPOCH, Duration::ZERO));
    Json(Status {
        api_version: API_VERSION.to_owned(),
        build: state.build.clone(),
        healthy: snapshot.started && unhealthy_plugins == 0 && unhealthy_adapters == 0,
        started_at: started_at
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned()),
        uptime_ms: u64::try_from(uptime.as_millis()).unwrap_or(u64::MAX),
        devices: snapshot.devices.len(),
        subscribers: snapshot.subscriptions,
        plugins: plugins.len(),
        unhealthy_plugins,
        adapters: adapters.len(),
        unhealthy_adapters,
    })
}

async fn get_devices(State(state): State<Arc<ServerState>>) -> Json<DevicesResponse> {
    Json(DevicesResponse {
        devices: state
            .runtime
            .snapshot()
            .await
            .devices
            .iter()
            .map(device_from_state)
            .collect(),
    })
}

async fn get_plugins(State(state): State<Arc<ServerState>>) -> Json<PluginsResponse> {
    Json(PluginsResponse {
        plugins: plugin_statuses(&state).await,
    })
}

async fn get_plugin(
    State(state): State<Arc<ServerState>>,
    AxumPath(instance): AxumPath<String>,
) -> Result<Json<Plugin>, ApiFailure> {
    plugin_statuses(&state)
        .await
        .into_iter()
        .find(|plugin| plugin.instance_id == instance)
        .map(Json)
        .ok_or_else(not_found)
}

async fn get_adapters(State(state): State<Arc<ServerState>>) -> Json<AdaptersResponse> {
    Json(AdaptersResponse {
        adapters: adapter_statuses(&state).await,
    })
}

async fn get_adapter(
    State(state): State<Arc<ServerState>>,
    AxumPath(instance): AxumPath<String>,
) -> Result<Json<Adapter>, ApiFailure> {
    adapter_statuses(&state)
        .await
        .into_iter()
        .find(|adapter| adapter.instance_id == instance)
        .map(Json)
        .ok_or_else(not_found)
}

async fn open_commissioning_window(
    State(state): State<Arc<ServerState>>,
    AxumPath(instance): AxumPath<String>,
    body: Result<Json<CommissioningRequest>, JsonRejection>,
) -> Result<Json<CommissioningWindow>, ApiFailure> {
    let request = strict_body(body)?;
    let adapter = find_adapter(&state, &instance)?;
    let result = adapter
        .open_commissioning_window(Duration::from_secs(u64::from(request.duration_seconds)))
        .await
        .map_err(adapter_failure)?;
    Ok(Json(CommissioningWindow {
        duration_seconds: result.duration_seconds,
        remaining_seconds: result.remaining_seconds,
        manual_code: result.manual_code,
        qr_code: result.qr_code,
    }))
}

async fn get_commissioning_info(
    State(state): State<Arc<ServerState>>,
    AxumPath(instance): AxumPath<String>,
) -> Result<Json<CommissioningInfo>, ApiFailure> {
    let adapter = find_adapter(&state, &instance)?;
    let result = adapter
        .commissioning_info()
        .await
        .map_err(adapter_failure)?;
    Ok(Json(CommissioningInfo {
        open: result.open,
        duration_seconds: result.duration_seconds,
        remaining_seconds: result.remaining_seconds,
        manual_code: result.manual_code,
        qr_code: result.qr_code,
    }))
}

async fn remove_adapter_resource(
    State(state): State<Arc<ServerState>>,
    AxumPath((instance, resource_type, id)): AxumPath<(String, String, String)>,
) -> Result<Json<AdapterResourcesResponse>, ApiFailure> {
    let adapter = find_adapter(&state, &instance)?;
    let resources = adapter
        .remove_resource(resource_type, id)
        .await
        .map_err(adapter_failure)?;
    Ok(Json(AdapterResourcesResponse {
        resources: resources
            .into_iter()
            .map(|resource| AdapterResource {
                resource_type: resource.resource_type,
                id: resource.id,
                name: resource.name,
                attributes: resource.attributes,
            })
            .collect(),
    }))
}

async fn reset_adapter_state(
    State(state): State<Arc<ServerState>>,
    AxumPath(instance): AxumPath<String>,
    body: Result<Json<ResetRequest>, JsonRejection>,
) -> Result<Json<crate::wire::AdapterReset>, ApiFailure> {
    let request = strict_body(body)?;
    if request.confirmation != instance {
        return Err(ApiFailure::bad_request(
            "confirmation_required",
            "confirmation must exactly match the adapter instance ID",
        ));
    }
    let adapter = find_adapter(&state, &instance)?;
    adapter.reset_state().await.map_err(adapter_failure)?;
    Ok(Json(crate::wire::AdapterReset {
        instance_id: instance,
        state: "running".to_owned(),
    }))
}

async fn get_capability(
    State(state): State<Arc<ServerState>>,
    AxumPath((device, capability)): AxumPath<(String, String)>,
) -> Result<Json<Capability>, ApiFailure> {
    find_capability(&state.runtime, &device, &capability)
        .await
        .map(Json)
}

async fn put_capability(
    State(state): State<Arc<ServerState>>,
    AxumPath((device, capability)): AxumPath<(String, String)>,
    body: Result<Json<WriteRequest>, JsonRejection>,
) -> Result<Json<Capability>, ApiFailure> {
    let request = strict_body(body)?;
    let value = value_to_core(&request.value)
        .map_err(|message| ApiFailure::bad_request("invalid_value", message))?;
    let device_id = DeviceId::new(device.clone())
        .map_err(|_| ApiFailure::bad_request("invalid_request", "invalid device ID"))?;
    let capability_id = CapabilityId::new(capability.clone())
        .map_err(|_| ApiFailure::bad_request("invalid_request", "invalid capability ID"))?;
    state
        .runtime
        .write(Command {
            device_id,
            capability_id,
            value,
        })
        .await
        .map_err(ApiFailure::from_core)?;
    find_capability(&state.runtime, &device, &capability)
        .await
        .map(Json)
}

async fn get_events(State(state): State<Arc<ServerState>>) -> Result<Response, ApiFailure> {
    let mut subscription = state
        .runtime
        .subscribe(EVENT_BUFFER)
        .await
        .map_err(ApiFailure::from_core)?;
    let stream = async_stream::stream! {
        while let Some(event) = subscription.recv().await {
            let record = StreamRecord { event: Some(event_from_core(&event)), error: None };
            match serde_json::to_vec(&record) {
                Ok(mut encoded) => {
                    encoded.push(b'\n');
                    yield Ok::<Bytes, Infallible>(Bytes::from(encoded));
                }
                Err(_) => break,
            }
        }
        if let Some(error) = subscription.error()
            && !matches!(error, RuntimeError::Stopped)
        {
            let record = StreamRecord {
                event: None,
                error: Some(ErrorPayload {
                    code: "stream_closed".to_owned(),
                    message: error.to_string(),
                }),
            };
            if let Ok(mut encoded) = serde_json::to_vec(&record) {
                encoded.push(b'\n');
                yield Ok::<Bytes, Infallible>(Bytes::from(encoded));
            }
        }
        subscription.cancel().await;
    };
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn find_capability(
    runtime: &Runtime,
    device_id: &str,
    capability_id: &str,
) -> Result<Capability, ApiFailure> {
    for state in runtime.snapshot().await.devices {
        if state.device.id.as_str() != device_id {
            continue;
        }
        for capability in &state.device.capabilities {
            if capability.id.as_str() == capability_id {
                return Ok(capability_from_state(
                    capability,
                    state.values.get(&capability.id),
                ));
            }
        }
        return Err(not_found());
    }
    Err(not_found())
}

async fn plugin_statuses(state: &ServerState) -> Vec<Plugin> {
    let mut result = Vec::with_capacity(state.plugins.len());
    for instance in &state.plugins {
        let status = instance.status().await;
        let healthy = status.healthy();
        result.push(Plugin {
            instance_id: status.instance_id,
            plugin_id: status.plugin_id,
            name: status.name,
            version: status.version,
            api_version: status.api_version,
            artifact: status.artifact.display().to_string(),
            state: status.state.as_str().to_owned(),
            healthy,
            pid: status.pid,
            restarts: status.restarts,
            devices: status.devices,
            last_error: status.last_error,
            health_detail: status.health_detail,
        });
    }
    result.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    result
}

async fn adapter_statuses(state: &ServerState) -> Vec<Adapter> {
    let mut result = Vec::with_capacity(state.adapters.len());
    for instance in &state.adapters {
        let status = instance.status().await;
        let healthy = status.is_healthy();
        result.push(Adapter {
            instance_id: status.instance_id,
            adapter_id: status.adapter_id,
            version: status.adapter_version,
            protocol: status.protocol,
            state: status.state.as_str().to_owned(),
            healthy,
            pid: status.pid,
            restarts: status.restarts,
            generation: status.generation,
            snapshot_revision: status.snapshot_revision,
            devices: status.devices,
            diagnostics: status
                .diagnostics
                .into_iter()
                .map(|diagnostic| AdapterDiagnostic {
                    severity: diagnostic.severity,
                    code: diagnostic.code,
                    device_id: diagnostic.device_id,
                    capability_id: diagnostic.capability_id,
                    message: diagnostic.message,
                })
                .collect(),
            resources: status
                .resources
                .into_iter()
                .map(|resource| AdapterResource {
                    resource_type: resource.resource_type,
                    id: resource.id,
                    name: resource.name,
                    attributes: resource.attributes,
                })
                .collect(),
            last_error: status.last_error,
            health_detail: status.health_detail,
        });
    }
    result.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    result
}

fn find_adapter(state: &ServerState, instance: &str) -> Result<Arc<AdapterInstance>, ApiFailure> {
    state
        .adapters
        .iter()
        .find(|adapter| adapter.instance_id() == instance)
        .cloned()
        .ok_or_else(not_found)
}

fn adapter_failure(error: AdapterInstanceError) -> ApiFailure {
    if let Some((code, message)) = error.protocol_error() {
        ApiFailure::gateway(code, message)
    } else {
        ApiFailure::gateway("adapter_error", "adapter operation failed")
    }
}

#[derive(Debug)]
struct ApiFailure {
    status: StatusCode,
    payload: ErrorPayload,
}

impl ApiFailure {
    fn bad_request(code: &str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            payload: ErrorPayload {
                code: code.to_owned(),
                message: message.into(),
            },
        }
    }

    fn gateway(code: &str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            payload: ErrorPayload {
                code: code.to_owned(),
                message: message.into(),
            },
        }
    }

    fn from_core(error: RuntimeError) -> Self {
        let (status, code) = match error {
            RuntimeError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            RuntimeError::NotReadable(_) => (StatusCode::CONFLICT, "not_readable"),
            RuntimeError::NotWritable(_) => (StatusCode::CONFLICT, "not_writable"),
            RuntimeError::InvalidValue(_) => (StatusCode::BAD_REQUEST, "invalid_value"),
            RuntimeError::Stopped => (StatusCode::SERVICE_UNAVAILABLE, "runtime_stopped"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };
        Self {
            status,
            payload: ErrorPayload {
                code: code.to_owned(),
                message: error.to_string(),
            },
        }
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.payload)).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

fn not_found() -> ApiFailure {
    ApiFailure {
        status: StatusCode::NOT_FOUND,
        payload: ErrorPayload {
            code: "not_found".to_owned(),
            message: "resource was not found".to_owned(),
        },
    }
}

fn strict_body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiFailure> {
    body.map(|Json(value)| value).map_err(|_| {
        ApiFailure::bad_request(
            "invalid_request",
            "request body must be one valid JSON object with no unknown fields",
        )
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
