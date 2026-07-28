use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use futures_util::StreamExt;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{Method, StatusCode};
use serde::{Serialize, de::DeserializeOwned};

use crate::wire::{
    Adapter, AdapterReset, AdapterResource, AdapterResourcesResponse, AdaptersResponse, Capability,
    CommissioningInfo, CommissioningRequest, CommissioningWindow, Device, DevicesResponse,
    ErrorPayload, Event, Plugin, PluginsResponse, ResetRequest, Status, StreamRecord, Value,
    WriteRequest,
};

const UNARY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BODY: usize = 1 << 20;

#[derive(Debug, thiserror::Error)]
#[error("admin API {code}: {message}")]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("admin socket path is required")]
    MissingSocket,
    #[error("resolve admin socket path: {0}")]
    Resolve(#[source] std::io::Error),
    #[error("build admin client: {0}")]
    Build(#[source] reqwest::Error),
    #[error("call admin API: {0}")]
    Request(#[source] reqwest::Error),
    #[error("admin request timed out")]
    Timeout,
    #[error("admin response exceeded {MAX_RESPONSE_BODY} bytes")]
    ResponseTooLarge,
    #[error("decode admin response: {0}")]
    Decode(#[source] serde_json::Error),
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("admin event stream closed unexpectedly")]
    StreamClosed,
    #[error("admin event stream record has no event or error")]
    InvalidStreamRecord,
    #[error("event receiver: {0}")]
    Receiver(String),
}

#[derive(Clone)]
pub struct Client {
    socket_path: PathBuf,
    http: reqwest::Client,
}

impl Client {
    pub fn new(socket_path: impl AsRef<Path>) -> Result<Self, ClientError> {
        let socket_path = socket_path.as_ref();
        if socket_path.as_os_str().is_empty() {
            return Err(ClientError::MissingSocket);
        }
        let socket_path = if socket_path.is_absolute() {
            socket_path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(ClientError::Resolve)?
                .join(socket_path)
        };
        let http = reqwest::Client::builder()
            .unix_socket(socket_path.clone())
            .no_proxy()
            .build()
            .map_err(ClientError::Build)?;
        Ok(Self { socket_path, http })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn status(&self) -> Result<Status, ClientError> {
        self.unary(Method::GET, "/v1/status", None::<&()>).await
    }

    pub async fn devices(&self) -> Result<Vec<Device>, ClientError> {
        let response: DevicesResponse = self.unary(Method::GET, "/v1/devices", None::<&()>).await?;
        Ok(response.devices)
    }

    pub async fn plugins(&self) -> Result<Vec<Plugin>, ClientError> {
        let response: PluginsResponse = self.unary(Method::GET, "/v1/plugins", None::<&()>).await?;
        Ok(response.plugins)
    }

    pub async fn plugin(&self, instance: &str) -> Result<Plugin, ClientError> {
        self.unary(
            Method::GET,
            &format!("/v1/plugins/{}", segment(instance)),
            None::<&()>,
        )
        .await
    }

    pub async fn adapters(&self) -> Result<Vec<Adapter>, ClientError> {
        let response: AdaptersResponse =
            self.unary(Method::GET, "/v1/adapters", None::<&()>).await?;
        Ok(response.adapters)
    }

    pub async fn adapter(&self, instance: &str) -> Result<Adapter, ClientError> {
        self.unary(
            Method::GET,
            &format!("/v1/adapters/{}", segment(instance)),
            None::<&()>,
        )
        .await
    }

    pub async fn capability(
        &self,
        device: &str,
        capability: &str,
    ) -> Result<Capability, ClientError> {
        self.unary(
            Method::GET,
            &capability_path(device, capability),
            None::<&()>,
        )
        .await
    }

    pub async fn set_capability(
        &self,
        device: &str,
        capability: &str,
        value: Value,
    ) -> Result<Capability, ClientError> {
        self.unary(
            Method::PUT,
            &capability_path(device, capability),
            Some(&WriteRequest { value }),
        )
        .await
    }

    pub async fn open_commissioning_window(
        &self,
        instance: &str,
        duration_seconds: u16,
    ) -> Result<CommissioningWindow, ClientError> {
        self.unary(
            Method::POST,
            &format!("/v1/adapters/{}/commissioning-window", segment(instance)),
            Some(&CommissioningRequest { duration_seconds }),
        )
        .await
    }

    pub async fn commissioning_info(
        &self,
        instance: &str,
    ) -> Result<CommissioningInfo, ClientError> {
        self.unary(
            Method::GET,
            &format!("/v1/adapters/{}/commissioning-window", segment(instance)),
            None::<&()>,
        )
        .await
    }

    pub async fn remove_adapter_resource(
        &self,
        instance: &str,
        resource_type: &str,
        id: &str,
    ) -> Result<Vec<AdapterResource>, ClientError> {
        let response: AdapterResourcesResponse = self
            .unary(
                Method::DELETE,
                &format!(
                    "/v1/adapters/{}/resources/{}/{}",
                    segment(instance),
                    segment(resource_type),
                    segment(id)
                ),
                None::<&()>,
            )
            .await?;
        Ok(response.resources)
    }

    pub async fn reset_adapter_state(
        &self,
        instance: &str,
        confirmation: &str,
    ) -> Result<AdapterReset, ClientError> {
        self.unary(
            Method::POST,
            &format!("/v1/adapters/{}/reset", segment(instance)),
            Some(&ResetRequest {
                confirmation: confirmation.to_owned(),
            }),
        )
        .await
    }

    pub async fn watch<F>(&self, mut receive: F) -> Result<(), ClientError>
    where
        F: FnMut(Event) -> Result<(), String>,
    {
        let response = self
            .http
            .get("http://oikade/v1/events")
            .send()
            .await
            .map_err(ClientError::Request)?;
        let status = response.status();
        if !status.is_success() {
            return Err(decode_api_error(status, read_bounded(response).await?));
        }
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(ClientError::Request)?;
            buffer.extend_from_slice(&chunk);
            if buffer.len() > MAX_RESPONSE_BODY {
                return Err(ClientError::ResponseTooLarge);
            }
            while let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = buffer.drain(..=index).collect();
                if line.len() == 1 {
                    continue;
                }
                receive_record(
                    status,
                    serde_json::from_slice(&line[..line.len() - 1]).map_err(ClientError::Decode)?,
                    &mut receive,
                )?;
            }
        }
        if !buffer.is_empty() {
            receive_record(
                status,
                serde_json::from_slice(&buffer).map_err(ClientError::Decode)?,
                &mut receive,
            )?;
        }
        Err(ClientError::StreamClosed)
    }

    async fn unary<T, B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, ClientError>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        let mut request = self.http.request(method, format!("http://oikade{path}"));
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = tokio::time::timeout(UNARY_TIMEOUT, request.send())
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(ClientError::Request)?;
        let status = response.status();
        let bytes = read_bounded(response).await?;
        if !status.is_success() {
            return Err(decode_api_error(status, bytes));
        }
        serde_json::from_slice(&bytes).map_err(ClientError::Decode)
    }
}

fn receive_record<F>(
    status: StatusCode,
    record: StreamRecord,
    receive: &mut F,
) -> Result<(), ClientError>
where
    F: FnMut(Event) -> Result<(), String>,
{
    if let Some(error) = record.error {
        return Err(ClientError::Api(ApiError {
            status,
            code: error.code,
            message: error.message,
        }));
    }
    let event = record.event.ok_or(ClientError::InvalidStreamRecord)?;
    receive(event).map_err(ClientError::Receiver)
}

async fn read_bounded(response: reqwest::Response) -> Result<Vec<u8>, ClientError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BODY as u64)
    {
        return Err(ClientError::ResponseTooLarge);
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(ClientError::Request)?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BODY {
            return Err(ClientError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn decode_api_error(status: StatusCode, body: Vec<u8>) -> ClientError {
    match serde_json::from_slice::<ErrorPayload>(&body) {
        Ok(payload) => ClientError::Api(ApiError {
            status,
            code: payload.code,
            message: payload.message,
        }),
        Err(_) => ClientError::Api(ApiError {
            status,
            code: "invalid_error_response".to_owned(),
            message: format!("admin API returned HTTP {status} with an invalid error body"),
        }),
    }
}

fn segment(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn capability_path(device: &str, capability: &str) -> String {
    format!(
        "/v1/devices/{}/capabilities/{}",
        segment(device),
        segment(capability)
    )
}
