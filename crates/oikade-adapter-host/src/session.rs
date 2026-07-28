use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use oikade_adapter_api as api;
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::{
    net::{
        UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{Mutex, Semaphore, mpsc, oneshot, watch},
    time::timeout,
};

const OUTBOUND_QUEUE: usize = 128;
const MAX_PENDING_CALLS: usize = 1024;
const MAX_IGNORED_RESPONSES: usize = 2048;
const MAX_INBOUND_CALLS: usize = 128;

#[async_trait]
pub trait CommandHandler: Send + Sync {
    async fn handle(
        &self,
        request: api::CommandRequest,
    ) -> Result<api::CommandResponse, api::ProtocolError>;
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("adapter session closed: {0}")]
    Closed(String),
    #[error("adapter protocol error {code}: {message}")]
    Protocol { code: String, message: String },
    #[error("adapter request timed out")]
    Timeout,
    #[error("adapter pending request limit reached")]
    PendingLimit,
    #[error("adapter protocol: {0}")]
    Invalid(String),
}

type Response = Result<api::Envelope, String>;

pub struct Session {
    outbound: mpsc::Sender<api::Envelope>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Response>>>,
    ignored: Mutex<HashSet<u64>>,
    next_id: AtomicU64,
    hello: Mutex<Option<oneshot::Receiver<Result<api::Hello, String>>>>,
    failure_tx: watch::Sender<Option<String>>,
    failure_rx: watch::Receiver<Option<String>>,
    pid: AtomicU32,
    handler: Arc<dyn CommandHandler>,
    inbound: Arc<Semaphore>,
}

impl Session {
    pub fn spawn(stream: UnixStream, handler: Arc<dyn CommandHandler>) -> Arc<Self> {
        let (reader, writer) = stream.into_split();
        let (outbound, outbound_rx) = mpsc::channel(OUTBOUND_QUEUE);
        let (hello_tx, hello) = oneshot::channel();
        let (failure_tx, failure_rx) = watch::channel(None);
        let session = Arc::new(Self {
            outbound,
            pending: Mutex::new(HashMap::new()),
            ignored: Mutex::new(HashSet::new()),
            next_id: AtomicU64::new(0),
            hello: Mutex::new(Some(hello)),
            failure_tx,
            failure_rx,
            pid: AtomicU32::new(0),
            handler,
            inbound: Arc::new(Semaphore::new(MAX_INBOUND_CALLS)),
        });
        tokio::spawn(write_loop(Arc::clone(&session), writer, outbound_rx));
        tokio::spawn(read_loop(Arc::clone(&session), reader, hello_tx));
        session
    }

    pub fn set_pid(&self, pid: u32) {
        self.pid.store(pid, Ordering::Release);
    }

    pub async fn wait_hello(&self, deadline: Duration) -> Result<api::Hello, SessionError> {
        let receiver = self
            .hello
            .lock()
            .await
            .take()
            .ok_or_else(|| SessionError::Invalid("hello already consumed".to_owned()))?;
        match timeout(deadline, receiver).await {
            Ok(Ok(Ok(hello))) => Ok(hello),
            Ok(Ok(Err(error))) => Err(SessionError::Closed(error)),
            Ok(Err(_)) => Err(self.closed()),
            Err(_) => Err(SessionError::Timeout),
        }
    }

    pub async fn call<Request, ResponseBody>(
        &self,
        method: &str,
        request: &Request,
        deadline: Duration,
    ) -> Result<ResponseBody, SessionError>
    where
        Request: Serialize,
        ResponseBody: DeserializeOwned,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.len() >= MAX_PENDING_CALLS {
                return Err(SessionError::PendingLimit);
            }
            pending.insert(id, sender);
        }
        let outgoing = api::Envelope {
            version: api::VERSION,
            kind: api::FrameKind::Request,
            id,
            method: method.to_owned(),
            body: Some(
                api::encode_body(request)
                    .map_err(|error| SessionError::Invalid(error.to_string()))?,
            ),
            error: None,
        };
        if self.outbound.send(outgoing).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err(self.closed());
        }
        let envelope = match timeout(deadline, receiver).await {
            Ok(Ok(Ok(envelope))) => envelope,
            Ok(Ok(Err(error))) => return Err(SessionError::Closed(error)),
            Ok(Err(_)) => return Err(self.closed()),
            Err(_) => {
                self.abandon(id).await;
                return Err(SessionError::Timeout);
            }
        };
        if envelope.method != method {
            let error = format!(
                "response method {:?} does not match request {method:?}",
                envelope.method
            );
            self.fail(error.clone()).await;
            return Err(SessionError::Invalid(error));
        }
        if let Some(error) = envelope.error {
            return Err(SessionError::Protocol {
                code: error.code,
                message: error.message,
            });
        }
        let body = envelope
            .body
            .as_deref()
            .ok_or_else(|| SessionError::Invalid("response body is required".to_owned()))?;
        api::decode_body(body).map_err(|error| SessionError::Invalid(error.to_string()))
    }

    pub async fn wait_failure(&self) -> String {
        let mut failure = self.failure_rx.clone();
        loop {
            if let Some(error) = failure.borrow().clone() {
                return error;
            }
            if failure.changed().await.is_err() {
                return "adapter session closed".to_owned();
            }
        }
    }

    pub async fn fail(&self, error: impl Into<String>) {
        let error = error.into();
        if self.failure_tx.borrow().is_some() {
            return;
        }
        let _ = self.failure_tx.send(Some(error.clone()));
        for (_, sender) in std::mem::take(&mut *self.pending.lock().await) {
            let _ = sender.send(Err(error.clone()));
        }
        let pid = self.pid.load(Ordering::Acquire);
        if pid != 0 {
            let _ = oikade_supervisor::kill_process_group(pid);
        }
    }

    fn closed(&self) -> SessionError {
        SessionError::Closed(
            self.failure_rx
                .borrow()
                .clone()
                .unwrap_or_else(|| "adapter session closed".to_owned()),
        )
    }

    async fn abandon(&self, id: u64) {
        if self.pending.lock().await.remove(&id).is_some() {
            let mut ignored = self.ignored.lock().await;
            if ignored.len() >= MAX_IGNORED_RESPONSES {
                drop(ignored);
                self.fail("adapter abandoned response limit reached").await;
            } else {
                ignored.insert(id);
            }
        }
    }

    async fn respond(
        &self,
        request: &api::Envelope,
        result: Result<api::CommandResponse, api::ProtocolError>,
    ) {
        let (body, error) = match result {
            Ok(response) => match api::encode_body(&response) {
                Ok(body) => (Some(body), None),
                Err(error) => {
                    self.fail(error.to_string()).await;
                    return;
                }
            },
            Err(error) => (None, Some(error)),
        };
        let _ = self
            .outbound
            .send(api::Envelope {
                version: api::VERSION,
                kind: api::FrameKind::Response,
                id: request.id,
                method: request.method.clone(),
                body,
                error,
            })
            .await;
    }
}

async fn write_loop(
    session: Arc<Session>,
    writer: OwnedWriteHalf,
    mut outbound: mpsc::Receiver<api::Envelope>,
) {
    let mut encoder = api::AsyncEncoder::new(writer);
    while let Some(envelope) = outbound.recv().await {
        if let Err(error) = encoder.encode(&envelope).await {
            session.fail(error.to_string()).await;
            return;
        }
    }
}

async fn read_loop(
    session: Arc<Session>,
    reader: OwnedReadHalf,
    hello_tx: oneshot::Sender<Result<api::Hello, String>>,
) {
    let mut decoder = api::AsyncDecoder::new(reader);
    let mut hello_tx = Some(hello_tx);
    let mut hello_received = false;
    loop {
        let incoming = match decoder.decode().await {
            Ok(Some(incoming)) => incoming,
            Ok(None) => {
                session.fail("adapter closed the RPC transport").await;
                return;
            }
            Err(error) => {
                session.fail(error.to_string()).await;
                return;
            }
        };
        let result = match incoming.kind {
            api::FrameKind::Notification => {
                if incoming.method != api::METHOD_HELLO || hello_received {
                    Err("adapter sent an unsupported or duplicate notification".to_owned())
                } else {
                    let body = incoming
                        .body
                        .as_deref()
                        .ok_or_else(|| "adapter hello body is required".to_owned());
                    match body
                        .and_then(|body| api::decode_body(body).map_err(|error| error.to_string()))
                    {
                        Ok(hello) => {
                            hello_received = true;
                            if let Some(sender) = hello_tx.take() {
                                let _ = sender.send(Ok(hello));
                            }
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
            }
            api::FrameKind::Response if hello_received => {
                if session.ignored.lock().await.remove(&incoming.id) {
                    Ok(())
                } else if let Some(sender) = session.pending.lock().await.remove(&incoming.id) {
                    let _ = sender.send(Ok(incoming));
                    Ok(())
                } else {
                    Err(format!(
                        "adapter response references unknown request {}",
                        incoming.id
                    ))
                }
            }
            api::FrameKind::Request if hello_received && incoming.method == api::METHOD_COMMAND => {
                dispatch_command(Arc::clone(&session), incoming);
                Ok(())
            }
            api::FrameKind::Response | api::FrameKind::Request => {
                Err("adapter sent traffic before hello or an unsupported request".to_owned())
            }
        };
        if let Err(error) = result {
            session.fail(error).await;
            return;
        }
    }
}

fn dispatch_command(session: Arc<Session>, request: api::Envelope) {
    tokio::spawn(async move {
        let permit = match Arc::clone(&session.inbound).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                session
                    .respond(
                        &request,
                        Err(api::ProtocolError {
                            code: "busy".to_owned(),
                            message: "adapter command concurrency limit reached".to_owned(),
                        }),
                    )
                    .await;
                return;
            }
        };
        let result = match request.body.as_deref() {
            Some(body) => match api::decode_body(body) {
                Ok(command) => session.handler.handle(command).await,
                Err(error) => Err(api::ProtocolError {
                    code: "invalid_request".to_owned(),
                    message: error.to_string(),
                }),
            },
            None => Err(api::ProtocolError {
                code: "invalid_request".to_owned(),
                message: "adapter command body is required".to_owned(),
            }),
        };
        session.respond(&request, result).await;
        drop(permit);
    });
}
