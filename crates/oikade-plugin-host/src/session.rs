use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};

use oikade_plugin_api as api;
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::{
    net::{
        UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{Mutex, mpsc, oneshot, watch},
    time::timeout,
};

const OUTBOUND_QUEUE: usize = 128;
const NOTIFICATION_QUEUE: usize = 256;
const MAX_PENDING_REQUESTS: usize = 1024;
const MAX_IGNORED_RESPONSES: usize = 2048;

#[derive(Debug)]
pub enum Notification {
    Event(api::Event),
    Reconcile(api::Reconcile),
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("plugin session closed: {0}")]
    Closed(String),
    #[error("plugin protocol error {code}: {message}")]
    Protocol { code: String, message: String },
    #[error("plugin request timed out")]
    Timeout,
    #[error("plugin pending request limit reached")]
    PendingLimit,
    #[error("plugin protocol: {0}")]
    Invalid(String),
}

type Response = Result<api::Envelope, String>;

pub struct Session {
    outbound: mpsc::Sender<api::Envelope>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Response>>>,
    ignored: Mutex<HashSet<u64>>,
    next_id: AtomicU64,
    hello: Mutex<Option<oneshot::Receiver<Result<api::Hello, String>>>>,
    notifications: Mutex<mpsc::Receiver<Notification>>,
    failure_tx: watch::Sender<Option<String>>,
    failure_rx: watch::Receiver<Option<String>>,
    pid: AtomicU32,
}

impl Session {
    pub fn spawn(stream: UnixStream) -> Arc<Self> {
        let (reader, writer) = stream.into_split();
        let (outbound, outbound_rx) = mpsc::channel(OUTBOUND_QUEUE);
        let (notifications_tx, notifications) = mpsc::channel(NOTIFICATION_QUEUE);
        let (hello_tx, hello) = oneshot::channel();
        let (failure_tx, failure_rx) = watch::channel(None);
        let session = Arc::new(Self {
            outbound,
            pending: Mutex::new(HashMap::new()),
            ignored: Mutex::new(HashSet::new()),
            next_id: AtomicU64::new(0),
            hello: Mutex::new(Some(hello)),
            notifications: Mutex::new(notifications),
            failure_tx,
            failure_rx,
            pid: AtomicU32::new(0),
        });
        tokio::spawn(write_loop(Arc::clone(&session), writer, outbound_rx));
        tokio::spawn(read_loop(
            Arc::clone(&session),
            reader,
            hello_tx,
            notifications_tx,
        ));
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
        let body =
            api::encode_body(request).map_err(|error| SessionError::Invalid(error.to_string()))?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.len() >= MAX_PENDING_REQUESTS {
                return Err(SessionError::PendingLimit);
            }
            pending.insert(id, sender);
        }
        let outgoing = api::Envelope {
            version: api::VERSION,
            id,
            method: method.to_owned(),
            body: Some(body),
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
                self.send_cancel(id).await;
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

    pub async fn next_notification(&self) -> Option<Notification> {
        self.notifications.lock().await.recv().await
    }

    pub async fn wait_failure(&self) -> String {
        let mut failure = self.failure_rx.clone();
        loop {
            if let Some(error) = failure.borrow().clone() {
                return error;
            }
            if failure.changed().await.is_err() {
                return "plugin session closed".to_owned();
            }
        }
    }

    pub async fn fail(&self, error: impl Into<String>) {
        let error = error.into();
        if self.failure_tx.borrow().is_some() {
            return;
        }
        let _ = self.failure_tx.send(Some(error.clone()));
        let pending = std::mem::take(&mut *self.pending.lock().await);
        for (_, sender) in pending {
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
                .unwrap_or_else(|| "plugin session closed".to_owned()),
        )
    }

    async fn abandon(&self, id: u64) {
        if self.pending.lock().await.remove(&id).is_some() {
            let mut ignored = self.ignored.lock().await;
            if ignored.len() >= MAX_IGNORED_RESPONSES {
                drop(ignored);
                self.fail("plugin abandoned response limit reached").await;
            } else {
                ignored.insert(id);
            }
        }
    }

    async fn send_cancel(&self, request_id: u64) {
        let Ok(body) = api::encode_body(&api::CancelRequest { request_id }) else {
            return;
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        {
            let mut ignored = self.ignored.lock().await;
            if ignored.len() >= MAX_IGNORED_RESPONSES {
                drop(ignored);
                self.fail("plugin ignored response limit reached").await;
                return;
            }
            ignored.insert(id);
        }
        if self
            .outbound
            .try_send(api::Envelope {
                version: api::VERSION,
                id,
                method: api::METHOD_CANCEL.to_owned(),
                body: Some(body),
                error: None,
            })
            .is_err()
        {
            self.ignored.lock().await.remove(&id);
        }
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
    notifications: mpsc::Sender<Notification>,
) {
    let mut decoder = api::AsyncDecoder::new(reader);
    let mut hello_tx = Some(hello_tx);
    let mut hello_received = false;
    loop {
        let incoming = match decoder.decode().await {
            Ok(Some(incoming)) => incoming,
            Ok(None) => {
                session.fail("plugin closed the RPC transport").await;
                return;
            }
            Err(error) => {
                session.fail(error.to_string()).await;
                return;
            }
        };
        if incoming.id == 0 {
            let result = handle_notification(
                &session,
                incoming,
                &mut hello_received,
                &mut hello_tx,
                &notifications,
            )
            .await;
            if let Err(error) = result {
                session.fail(error).await;
                return;
            }
            continue;
        }

        if session.ignored.lock().await.remove(&incoming.id) {
            continue;
        }
        let pending = session.pending.lock().await.remove(&incoming.id);
        let Some(pending) = pending else {
            session
                .fail(format!(
                    "plugin response has unknown request ID {}",
                    incoming.id
                ))
                .await;
            return;
        };
        let _ = pending.send(Ok(incoming));
    }
}

async fn handle_notification(
    _session: &Arc<Session>,
    incoming: api::Envelope,
    hello_received: &mut bool,
    hello_tx: &mut Option<oneshot::Sender<Result<api::Hello, String>>>,
    notifications: &mpsc::Sender<Notification>,
) -> Result<(), String> {
    let body = incoming
        .body
        .as_deref()
        .ok_or_else(|| format!("plugin {:?} notification body is required", incoming.method))?;
    match incoming.method.as_str() {
        api::METHOD_HELLO => {
            if *hello_received {
                return Err("plugin sent duplicate hello".to_owned());
            }
            let hello: api::Hello = api::decode_body(body).map_err(|error| error.to_string())?;
            *hello_received = true;
            if let Some(sender) = hello_tx.take() {
                let _ = sender.send(Ok(hello));
            }
            Ok(())
        }
        api::METHOD_EVENT if *hello_received => {
            let event = api::decode_body(body).map_err(|error| error.to_string())?;
            notifications
                .try_send(Notification::Event(event))
                .map_err(|_| "plugin event queue is full".to_owned())
        }
        api::METHOD_RECONCILE if *hello_received => {
            let reconcile = api::decode_body(body).map_err(|error| error.to_string())?;
            notifications
                .try_send(Notification::Reconcile(reconcile))
                .map_err(|_| "plugin reconciliation queue is full".to_owned())
        }
        api::METHOD_EVENT | api::METHOD_RECONCILE => {
            Err("plugin sent notification before hello".to_owned())
        }
        other => Err(format!("unsupported unsolicited plugin method {other:?}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn queues_notifications_and_correlates_responses() {
        let (host, peer) = UnixStream::pair().unwrap();
        let session = Session::spawn(host);
        let (peer_reader, peer_writer) = peer.into_split();
        let peer = tokio::spawn(async move {
            let mut encoder = api::AsyncEncoder::new(peer_writer);
            encoder
                .encode(&api::Envelope {
                    version: 1,
                    id: 0,
                    method: api::METHOD_HELLO.to_owned(),
                    body: Some(
                        api::encode_body(&api::Hello {
                            plugin_id: "example.plugin".to_owned(),
                            plugin_version: "1.0.0".to_owned(),
                            min_api_version: 1,
                            max_api_version: 1,
                        })
                        .unwrap(),
                    ),
                    error: None,
                })
                .await
                .unwrap();
            let mut decoder = api::AsyncDecoder::new(peer_reader);
            let request = decoder.decode().await.unwrap().unwrap();
            encoder
                .encode(&api::Envelope {
                    version: 1,
                    id: request.id,
                    method: request.method,
                    body: Some(
                        api::encode_body(&api::HealthResponse {
                            healthy: true,
                            detail: String::new(),
                        })
                        .unwrap(),
                    ),
                    error: None,
                })
                .await
                .unwrap();
        });
        assert_eq!(
            session
                .wait_hello(Duration::from_secs(1))
                .await
                .unwrap()
                .plugin_id,
            "example.plugin"
        );
        let health: api::HealthResponse = session
            .call(
                api::METHOD_HEALTH,
                &serde_json::json!({}),
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert!(health.healthy);
        peer.await.unwrap();
    }
}
