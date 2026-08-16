use std::{collections::BTreeMap, pin::Pin, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        Path, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
    routing::{get, post},
};
use futures_util::{
    SinkExt, Stream, StreamExt,
    stream::{SplitSink, SplitStream},
};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::{Mutex, mpsc, watch},
    task::JoinHandle,
    time,
};
use uuid::Uuid;

use crate::{
    AppState,
    errors::AgentHostError,
    models::KernelRuntimeSession,
    sessions::{ApiError, KernelRuntime},
};

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
const MAX_DIMENSION: u16 = 1_000;
const DEFAULT_QUEUE_CAPACITY: usize = 64;
const CLIENT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
const CLIENT_DISCOVERY_INTERVAL: Duration = Duration::from_millis(25);
const MAX_WEBSOCKET_MESSAGE_SIZE: usize = 1024 * 1024;

pub const CLOSE_NORMAL: u16 = 1000;
pub const CLOSE_INTERNAL: u16 = 1011;
pub const CLOSE_GONE: u16 = 4404;
pub const CLOSE_CONFLICT: u16 = 4409;
pub const CLOSE_BACKPRESSURE: u16 = 4429;
pub const CLOSE_UPSTREAM_UNAVAILABLE: u16 = 4503;
pub const ATTACHMENT_ID_ENV: &str = "AGENTSPACE_TERMINAL_ATTACHMENT_ID";

pub type TerminalExecInput = Pin<Box<dyn AsyncWrite + Send>>;
pub type TerminalExecOutput = Pin<Box<dyn Stream<Item = Result<Bytes, AgentHostError>> + Send>>;

pub struct TerminalExec {
    pub exec_id: String,
    pub input: TerminalExecInput,
    pub output: TerminalExecOutput,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalState {
    Missing,
    Running,
    Exited,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalAttachKind {
    Started,
    Attached,
    Resumed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalClient {
    pub id: String,
    pub tty: String,
    pub pid: i64,
    pub width: u16,
    pub height: u16,
    pub session_name: String,
    pub pane_id: String,
    #[serde(default)]
    pub attachment_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalStatus {
    pub state: TerminalState,
    pub exit_status: Option<i32>,
    pub attach_kind: Option<TerminalAttachKind>,
    pub session_name: String,
    pub target_session: String,
    pub socket_path: String,
    pub attach_argv: Vec<String>,
    pub pane_id: Option<String>,
    pub pane_pid: Option<i64>,
    pub attachment_count: usize,
    #[serde(default)]
    pub clients: Vec<TerminalClient>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DisconnectKind {
    Internal,
    Gone,
    Conflict,
    Backpressure,
    UpstreamUnavailable,
    ExecEnded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Disconnect {
    kind: DisconnectKind,
    message: String,
}

impl Disconnect {
    fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: DisconnectKind::Internal,
            message: message.into(),
        }
    }

    fn gone(message: impl Into<String>) -> Self {
        Self {
            kind: DisconnectKind::Gone,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            kind: DisconnectKind::Conflict,
            message: message.into(),
        }
    }

    fn backpressure(message: impl Into<String>) -> Self {
        Self {
            kind: DisconnectKind::Backpressure,
            message: message.into(),
        }
    }

    fn upstream_unavailable(message: impl Into<String>) -> Self {
        Self {
            kind: DisconnectKind::UpstreamUnavailable,
            message: message.into(),
        }
    }

    fn exec_ended() -> Self {
        Self {
            kind: DisconnectKind::ExecEnded,
            message: "terminal attachment ended".to_owned(),
        }
    }

    const fn close_code(&self) -> u16 {
        match self.kind {
            DisconnectKind::Internal => CLOSE_INTERNAL,
            DisconnectKind::Gone => CLOSE_GONE,
            DisconnectKind::Conflict | DisconnectKind::ExecEnded => CLOSE_CONFLICT,
            DisconnectKind::Backpressure => CLOSE_BACKPRESSURE,
            DisconnectKind::UpstreamUnavailable => CLOSE_UPSTREAM_UNAVAILABLE,
        }
    }
}

#[derive(Clone)]
pub struct TerminalService {
    inner: Arc<TerminalServiceInner>,
}

struct TerminalServiceInner {
    runtime: Arc<dyn KernelRuntime>,
    attachments: Mutex<BTreeMap<String, AttachmentLease>>,
    boundary_lock: Mutex<()>,
    queue_capacity: usize,
}

#[derive(Clone)]
struct AttachmentLease {
    attachment_id: String,
    session_id: String,
    exec_id: String,
    tmux_client_id: Option<String>,
    cols: u16,
    rows: u16,
    lifecycle: watch::Sender<Option<Disconnect>>,
}

pub struct TerminalConnection {
    pub attachment_id: String,
    pub status: TerminalStatus,
    pub cols: u16,
    pub rows: u16,
    input: mpsc::Sender<Bytes>,
    output: mpsc::Receiver<Bytes>,
    lifecycle: watch::Receiver<Option<Disconnect>>,
    tasks: Vec<JoinHandle<()>>,
}

impl TerminalConnection {
    fn abort_tasks(&self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl TerminalService {
    #[must_use]
    pub fn new(runtime: Arc<dyn KernelRuntime>) -> Self {
        Self::with_queue_capacity(runtime, DEFAULT_QUEUE_CAPACITY)
    }

    #[must_use]
    pub fn with_queue_capacity(runtime: Arc<dyn KernelRuntime>, queue_capacity: usize) -> Self {
        Self {
            inner: Arc::new(TerminalServiceInner {
                runtime,
                attachments: Mutex::new(BTreeMap::new()),
                boundary_lock: Mutex::new(()),
                queue_capacity: queue_capacity.max(1),
            }),
        }
    }

    pub async fn reconcile_adoption(
        &self,
        session_id: &str,
        session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        let _boundary = self.inner.boundary_lock.lock().await;
        self.reconcile_locked(session_id, session).await
    }

    pub async fn status(
        &self,
        session_id: &str,
        session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        let _boundary = self.inner.boundary_lock.lock().await;
        self.reconcile_locked(session_id, session).await
    }

    pub async fn ensure(
        &self,
        session_id: &str,
        session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        let _boundary = self.inner.boundary_lock.lock().await;
        self.reconcile_locked(session_id, session).await?;
        let status = self.inner.runtime.terminal_ensure(session).await?;
        let attach_kind = status.attach_kind;
        let mut observed = self
            .reconcile_status_locked(session_id, session, status)
            .await?;
        observed.attach_kind = attach_kind;
        Ok(observed)
    }

    pub async fn stop(
        &self,
        session_id: &str,
        session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        let _boundary = self.inner.boundary_lock.lock().await;
        let status = self.inner.runtime.terminal_stop(session).await?;
        self.reconcile_status_locked(session_id, session, status)
            .await
    }

    pub async fn resume(
        &self,
        session_id: &str,
        session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        let _boundary = self.inner.boundary_lock.lock().await;
        self.reconcile_locked(session_id, session).await?;
        let status = self.inner.runtime.terminal_resume(session).await?;
        let attach_kind = status.attach_kind;
        let mut observed = self
            .reconcile_status_locked(session_id, session, status)
            .await?;
        observed.attach_kind = attach_kind;
        Ok(observed)
    }

    pub async fn copy_mode(
        &self,
        session_id: &str,
        session: &KernelRuntimeSession,
        attachment_id: &str,
    ) -> Result<TerminalStatus, AgentHostError> {
        let _boundary = self.inner.boundary_lock.lock().await;
        let client_id = {
            let attachments = self.inner.attachments.lock().await;
            attachments
                .get(attachment_id)
                .filter(|lease| lease.session_id == session_id)
                .and_then(|lease| lease.tmux_client_id.clone())
        }
        .ok_or_else(|| AgentHostError::terminal_attachment_not_found(attachment_id))?;
        let status = self
            .inner
            .runtime
            .terminal_copy_mode(session, &client_id)
            .await?;
        self.reconcile_status_locked(session_id, session, status)
            .await
    }

    pub async fn attach(
        &self,
        session_id: &str,
        session: &KernelRuntimeSession,
    ) -> Result<TerminalConnection, AgentHostError> {
        let _boundary = self.inner.boundary_lock.lock().await;
        self.reconcile_locked(session_id, session).await?;
        let ensured = self.inner.runtime.terminal_ensure(session).await?;
        if ensured.state != TerminalState::Running {
            return Err(AgentHostError::conflict(format!(
                "terminal attachment requires a running terminal; observed {:?}",
                ensured.state
            )));
        }
        if ensured.attach_argv.is_empty() {
            return Err(AgentHostError::upstream_unavailable(
                "kernel terminal controller returned an empty attach argv",
            ));
        }

        let attachment_id = Uuid::now_v7().to_string();
        let exec = self
            .inner
            .runtime
            .terminal_attach(session, &attachment_id, &ensured.attach_argv)
            .await?;
        self.inner
            .runtime
            .terminal_resize(session, &exec.exec_id, DEFAULT_COLS, DEFAULT_ROWS)
            .await?;

        let (lifecycle_tx, lifecycle_rx) = watch::channel(None);
        self.inner.attachments.lock().await.insert(
            attachment_id.clone(),
            AttachmentLease {
                attachment_id: attachment_id.clone(),
                session_id: session_id.to_owned(),
                exec_id: exec.exec_id.clone(),
                tmux_client_id: None,
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
                lifecycle: lifecycle_tx.clone(),
            },
        );

        let discovery = self
            .discover_client_locked(session_id, session, &attachment_id)
            .await;
        let client_id = match discovery {
            Ok(client_id) => client_id,
            Err(error) => {
                self.inner.attachments.lock().await.remove(&attachment_id);
                drop(exec);
                return Err(error);
            }
        };
        if let Some(lease) = self.inner.attachments.lock().await.get_mut(&attachment_id) {
            lease.tmux_client_id = Some(client_id);
        }
        let attach_kind = ensured.attach_kind;
        let mut status = self.reconcile_locked(session_id, session).await?;
        status.attach_kind = attach_kind;

        let (input_tx, input_rx) = mpsc::channel(self.inner.queue_capacity);
        let (output_tx, output_rx) = mpsc::channel(self.inner.queue_capacity);
        let tasks = spawn_io_tasks(exec.input, exec.output, input_rx, output_tx, lifecycle_tx);

        Ok(TerminalConnection {
            attachment_id,
            status,
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            input: input_tx,
            output: output_rx,
            lifecycle: lifecycle_rx,
            tasks,
        })
    }

    pub async fn resize(
        &self,
        session_id: &str,
        session: &KernelRuntimeSession,
        attachment_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), AgentHostError> {
        validate_dimensions(cols, rows)?;
        let exec_id = {
            let attachments = self.inner.attachments.lock().await;
            attachments
                .get(attachment_id)
                .filter(|lease| lease.session_id == session_id)
                .map(|lease| lease.exec_id.clone())
        }
        .ok_or_else(|| AgentHostError::terminal_attachment_not_found(attachment_id))?;
        self.inner
            .runtime
            .terminal_resize(session, &exec_id, cols, rows)
            .await?;
        if let Some(lease) = self.inner.attachments.lock().await.get_mut(attachment_id) {
            lease.cols = cols;
            lease.rows = rows;
        }
        Ok(())
    }

    pub async fn detach(
        &self,
        session_id: &str,
        session: &KernelRuntimeSession,
        attachment_id: &str,
    ) -> Result<(), AgentHostError> {
        let _boundary = self.inner.boundary_lock.lock().await;
        let lease = self.inner.attachments.lock().await.remove(attachment_id);
        let Some(lease) = lease.filter(|lease| lease.session_id == session_id) else {
            return Ok(());
        };
        if let Some(client_id) = lease.tmux_client_id {
            match self
                .inner
                .runtime
                .terminal_detach_client(session, &client_id)
                .await
            {
                Ok(_) | Err(AgentHostError::TerminalAttachmentNotFound { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        let _ = lease.lifecycle.send(None);
        self.reconcile_locked(session_id, session).await?;
        Ok(())
    }

    pub async fn forget_session(&self, session_id: &str) {
        let mut attachments = self.inner.attachments.lock().await;
        let forgotten = attachments
            .extract_if(.., |_attachment_id, lease| lease.session_id == session_id)
            .map(|(_attachment_id, lease)| lease)
            .collect::<Vec<_>>();
        drop(attachments);
        for lease in forgotten {
            let _ = lease
                .lifecycle
                .send(Some(Disconnect::gone("session was removed")));
        }
    }

    async fn discover_client_locked(
        &self,
        session_id: &str,
        session: &KernelRuntimeSession,
        attachment_id: &str,
    ) -> Result<String, AgentHostError> {
        let deadline = time::Instant::now() + CLIENT_DISCOVERY_TIMEOUT;
        loop {
            let status = self.inner.runtime.terminal_status(session).await?;
            if let Some(client) = status
                .clients
                .iter()
                .find(|client| client.attachment_id.as_deref() == Some(attachment_id))
            {
                return Ok(client.id.clone());
            }
            if time::Instant::now() >= deadline {
                return Err(AgentHostError::upstream_unavailable(format!(
                    "Docker exec for terminal attachment {attachment_id:?} did not appear in tmux client state"
                )));
            }
            let still_active = self
                .inner
                .attachments
                .lock()
                .await
                .get(attachment_id)
                .is_some_and(|lease| lease.session_id == session_id);
            if !still_active {
                return Err(AgentHostError::conflict(
                    "terminal attachment disappeared during client discovery",
                ));
            }
            time::sleep(CLIENT_DISCOVERY_INTERVAL).await;
        }
    }

    async fn reconcile_locked(
        &self,
        session_id: &str,
        session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        let status = self.inner.runtime.terminal_status(session).await?;
        self.reconcile_status_locked(session_id, session, status)
            .await
    }

    async fn reconcile_status_locked(
        &self,
        session_id: &str,
        session: &KernelRuntimeSession,
        mut status: TerminalStatus,
    ) -> Result<TerminalStatus, AgentHostError> {
        let active = self
            .inner
            .attachments
            .lock()
            .await
            .values()
            .filter(|lease| lease.session_id == session_id)
            .cloned()
            .collect::<Vec<_>>();

        // Docker has no supported exec-list/remove API. Each exec receives an
        // AgentSpace attachment UUID in its environment, and kernel_host resolves
        // that marker through the tmux client PID inside the container. This
        // avoids relying on Docker's host-namespace PID. On adoption the active
        // set is empty and every observed tmux client is detached. Normal cleanup
        // targets the exact observed client ID; Docker owns disposal of completed
        // exec metadata after the detached tmux process exits.
        let stale_clients = status
            .clients
            .iter()
            .filter(|client| {
                !active.iter().any(|lease| {
                    client.attachment_id.as_deref() == Some(lease.attachment_id.as_str())
                        || lease.tmux_client_id.as_deref() == Some(client.id.as_str())
                })
            })
            .map(|client| client.id.clone())
            .collect::<Vec<_>>();
        for client_id in stale_clients {
            match self
                .inner
                .runtime
                .terminal_detach_client(session, &client_id)
                .await
            {
                Ok(observed) => status = observed,
                Err(AgentHostError::TerminalAttachmentNotFound { .. }) => {
                    status = self.inner.runtime.terminal_status(session).await?;
                }
                Err(error) => return Err(error),
            }
        }

        let observed_ids = status
            .clients
            .iter()
            .map(|client| client.id.as_str())
            .collect::<Vec<_>>();
        let phantom_ids = active
            .iter()
            .filter_map(|lease| {
                lease
                    .tmux_client_id
                    .as_deref()
                    .filter(|client_id| !observed_ids.contains(client_id))
                    .map(|_| lease.exec_id.clone())
            })
            .collect::<Vec<_>>();
        if status.state == TerminalState::Running && !phantom_ids.is_empty() {
            let mut attachments = self.inner.attachments.lock().await;
            let removed = attachments
                .extract_if(.., |_attachment_id, lease| {
                    lease.session_id == session_id && phantom_ids.contains(&lease.exec_id)
                })
                .map(|(_attachment_id, lease)| lease)
                .collect::<Vec<_>>();
            drop(attachments);
            for lease in removed {
                let _ = lease.lifecycle.send(Some(Disconnect::conflict(
                    "tmux client disappeared during attachment reconciliation",
                )));
            }
        }

        status.attachment_count = status.clients.len();
        Ok(status)
    }
}

fn spawn_io_tasks(
    mut input: TerminalExecInput,
    mut output: TerminalExecOutput,
    mut input_rx: mpsc::Receiver<Bytes>,
    output_tx: mpsc::Sender<Bytes>,
    lifecycle: watch::Sender<Option<Disconnect>>,
) -> Vec<JoinHandle<()>> {
    let input_lifecycle = lifecycle.clone();
    let input_task = tokio::spawn(async move {
        while let Some(bytes) = input_rx.recv().await {
            if let Err(error) = input.write_all(&bytes).await {
                let _ = input_lifecycle.send(Some(Disconnect::upstream_unavailable(format!(
                    "failed to write terminal input: {error}"
                ))));
                return;
            }
            if let Err(error) = input.flush().await {
                let _ = input_lifecycle.send(Some(Disconnect::upstream_unavailable(format!(
                    "failed to flush terminal input: {error}"
                ))));
                return;
            }
        }
        let _ = input.shutdown().await;
    });

    let output_task = tokio::spawn(async move {
        while let Some(item) = output.next().await {
            match item {
                Ok(bytes) => match output_tx.try_send(bytes) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        let _ = lifecycle.send(Some(Disconnect::backpressure(
                            "terminal output queue overflowed",
                        )));
                        return;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => return,
                },
                Err(error) => {
                    let _ =
                        lifecycle.send(Some(Disconnect::upstream_unavailable(error.to_string())));
                    return;
                }
            }
        }
        let _ = lifecycle.send(Some(Disconnect::exec_ended()));
    });

    vec![input_task, output_task]
}

fn validate_dimensions(cols: u16, rows: u16) -> Result<(), AgentHostError> {
    if !(1..=MAX_DIMENSION).contains(&cols) || !(1..=MAX_DIMENSION).contains(&rows) {
        return Err(AgentHostError::validation(format!(
            "terminal dimensions must be between 1 and {MAX_DIMENSION}"
        )));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
enum ClientFrame {
    Resize { cols: u16, rows: u16 },
}

#[derive(Debug, Deserialize)]
struct CopyModeRequest {
    attachment_id: String,
}

#[derive(Serialize)]
struct ReadyFrame<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    attachment_id: &'a str,
    cols: u16,
    rows: u16,
    terminal: &'a TerminalStatus,
}

#[derive(Serialize)]
struct ErrorFrame<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    code: u16,
    message: &'a str,
}

#[derive(Serialize)]
struct ExitedFrame<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    state: TerminalState,
    exit_status: Option<i32>,
    terminal: &'a TerminalStatus,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sessions/{session_id}/terminal", get(terminal_status))
        .route(
            "/sessions/{session_id}/terminal/ensure",
            post(terminal_ensure),
        )
        .route("/sessions/{session_id}/terminal/stop", post(terminal_stop))
        .route(
            "/sessions/{session_id}/terminal/resume",
            post(terminal_resume),
        )
        .route(
            "/sessions/{session_id}/terminal/copy-mode",
            post(terminal_copy_mode),
        )
        .route(
            "/sessions/{session_id}/terminal/ws",
            get(terminal_websocket),
        )
}

async fn terminal_status(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<TerminalStatus>, ApiError> {
    state
        .sessions
        .terminal_status(&session_id)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn terminal_ensure(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<TerminalStatus>, ApiError> {
    state
        .sessions
        .terminal_ensure(&session_id)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn terminal_stop(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<TerminalStatus>, ApiError> {
    state
        .sessions
        .terminal_stop(&session_id)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn terminal_resume(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<TerminalStatus>, ApiError> {
    state
        .sessions
        .terminal_resume(&session_id)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn terminal_copy_mode(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<CopyModeRequest>,
) -> Result<Json<TerminalStatus>, ApiError> {
    state
        .sessions
        .terminal_copy_mode(&session_id, &payload.attachment_id)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn terminal_websocket(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    websocket: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let status = state.sessions.terminal_status(&session_id).await?;
    if status.state != TerminalState::Running {
        return Err(ApiError(AgentHostError::conflict(format!(
            "terminal WebSocket requires a running terminal; observed {:?}",
            status.state
        ))));
    }
    Ok(websocket
        .max_message_size(MAX_WEBSOCKET_MESSAGE_SIZE)
        .max_frame_size(MAX_WEBSOCKET_MESSAGE_SIZE)
        .on_upgrade(move |socket| serve_terminal_socket(state, session_id, socket)))
}

async fn serve_terminal_socket(state: AppState, session_id: String, socket: WebSocket) {
    let connection = state.sessions.terminal_attach(&session_id).await;
    let mut connection = match connection {
        Ok(connection) => connection,
        Err(error) => {
            let disconnect = disconnect_for_error(&error);
            close_socket(socket, &disconnect).await;
            return;
        }
    };

    let attachment_id = connection.attachment_id.clone();
    let (mut sender, mut receiver) = socket.split();
    if send_ready(&mut sender, &connection).await {
        run_terminal_socket(
            &state,
            &session_id,
            &attachment_id,
            &mut sender,
            &mut receiver,
            &mut connection,
        )
        .await;
    }

    if let Err(error) = state
        .sessions
        .terminal_detach(&session_id, &attachment_id)
        .await
    {
        tracing::warn!(%session_id, %attachment_id, %error, "failed to clean terminal attachment");
    }
    connection.abort_tasks();
}

type TerminalSocketSender = SplitSink<WebSocket, Message>;
type TerminalSocketReceiver = SplitStream<WebSocket>;

async fn send_ready(sender: &mut TerminalSocketSender, connection: &TerminalConnection) -> bool {
    let ready = ReadyFrame {
        frame_type: "ready",
        attachment_id: &connection.attachment_id,
        cols: connection.cols,
        rows: connection.rows,
        terminal: &connection.status,
    };
    let ready_json = match serde_json::to_string(&ready) {
        Ok(json) => json,
        Err(error) => {
            let disconnect =
                Disconnect::internal(format!("failed to serialize ready frame: {error}"));
            let _ = send_disconnect(sender, &disconnect).await;
            return false;
        }
    };
    sender.send(Message::Text(ready_json.into())).await.is_ok()
}

async fn run_terminal_socket(
    state: &AppState,
    session_id: &str,
    attachment_id: &str,
    sender: &mut TerminalSocketSender,
    receiver: &mut TerminalSocketReceiver,
    connection: &mut TerminalConnection,
) {
    loop {
        let keep_open = tokio::select! {
            incoming = receiver.next() => handle_client_message(
                state,
                session_id,
                attachment_id,
                sender,
                &connection.input,
                incoming,
            ).await,
            output = connection.output.recv() => {
                handle_terminal_output(state, session_id, sender, output).await
            }
            changed = connection.lifecycle.changed() => {
                handle_lifecycle(state, session_id, sender, changed, &connection.lifecycle).await
            }
        };
        if !keep_open {
            break;
        }
    }
}

async fn handle_client_message(
    state: &AppState,
    session_id: &str,
    attachment_id: &str,
    sender: &mut TerminalSocketSender,
    input: &mpsc::Sender<Bytes>,
    incoming: Option<Result<Message, axum::Error>>,
) -> bool {
    match incoming {
        Some(Ok(Message::Binary(bytes))) => match input.try_send(bytes) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                let disconnect = Disconnect::backpressure("terminal input queue overflowed");
                let _ = send_disconnect(sender, &disconnect).await;
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                let disconnect = Disconnect::upstream_unavailable("terminal input stream closed");
                let _ = send_disconnect(sender, &disconnect).await;
                false
            }
        },
        Some(Ok(Message::Text(text))) => {
            handle_client_text(state, session_id, attachment_id, sender, text.as_str()).await
        }
        Some(Ok(Message::Close(_))) | None => {
            let _ = sender
                .send(Message::Close(Some(CloseFrame {
                    code: CLOSE_NORMAL,
                    reason: "terminal detached".into(),
                })))
                .await;
            false
        }
        Some(Ok(Message::Ping(_) | Message::Pong(_))) => true,
        Some(Err(error)) => {
            let disconnect = Disconnect::internal(format!("WebSocket receive failed: {error}"));
            let _ = send_disconnect(sender, &disconnect).await;
            false
        }
    }
}

async fn handle_client_text(
    state: &AppState,
    session_id: &str,
    attachment_id: &str,
    sender: &mut TerminalSocketSender,
    text: &str,
) -> bool {
    let frame = match serde_json::from_str::<ClientFrame>(text) {
        Ok(frame) => frame,
        Err(error) => {
            let disconnect = Disconnect::conflict(format!("invalid terminal text frame: {error}"));
            let _ = send_disconnect(sender, &disconnect).await;
            return false;
        }
    };
    let ClientFrame::Resize { cols, rows } = frame;
    if let Err(error) = validate_dimensions(cols, rows) {
        let disconnect = Disconnect::conflict(error.to_string());
        let _ = send_disconnect(sender, &disconnect).await;
        return false;
    }
    if let Err(error) = state
        .sessions
        .terminal_resize(session_id, attachment_id, cols, rows)
        .await
    {
        let disconnect = disconnect_for_error(&error);
        let _ = send_disconnect(sender, &disconnect).await;
        return false;
    }
    true
}

async fn handle_terminal_output(
    state: &AppState,
    session_id: &str,
    sender: &mut TerminalSocketSender,
    output: Option<Bytes>,
) -> bool {
    let Some(bytes) = output else {
        handle_exec_ended(state, session_id, sender).await;
        return false;
    };
    match time::timeout(Duration::from_secs(5), sender.send(Message::Binary(bytes))).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) => false,
        Err(_) => {
            let disconnect = Disconnect::backpressure("terminal WebSocket output stalled");
            let _ = send_disconnect(sender, &disconnect).await;
            false
        }
    }
}

async fn handle_lifecycle(
    state: &AppState,
    session_id: &str,
    sender: &mut TerminalSocketSender,
    changed: Result<(), watch::error::RecvError>,
    lifecycle: &watch::Receiver<Option<Disconnect>>,
) -> bool {
    if changed.is_err() {
        let disconnect = Disconnect::internal("terminal lifecycle channel closed");
        let _ = send_disconnect(sender, &disconnect).await;
        return false;
    }
    let disconnect = lifecycle.borrow().clone();
    let Some(disconnect) = disconnect else {
        return true;
    };
    if disconnect.kind == DisconnectKind::ExecEnded {
        handle_exec_ended(state, session_id, sender).await;
    } else {
        let _ = send_disconnect(sender, &disconnect).await;
    }
    false
}

async fn handle_exec_ended<S>(state: &AppState, session_id: &str, sender: &mut S)
where
    S: futures_util::Sink<Message> + Unpin,
{
    match state.sessions.terminal_status(session_id).await {
        Ok(status) => {
            let frame = ExitedFrame {
                frame_type: "exited",
                state: status.state,
                exit_status: status.exit_status,
                terminal: &status,
            };
            if let Ok(json) = serde_json::to_string(&frame) {
                let _ = sender.send(Message::Text(json.into())).await;
            }
            let _ = sender
                .send(Message::Close(Some(CloseFrame {
                    code: CLOSE_NORMAL,
                    reason: "terminal attachment ended".into(),
                })))
                .await;
        }
        Err(error) => {
            let disconnect = disconnect_for_error(&error);
            let _ = send_disconnect(sender, &disconnect).await;
        }
    }
}

async fn close_socket(mut socket: WebSocket, disconnect: &Disconnect) {
    let frame = ErrorFrame {
        frame_type: "error",
        code: disconnect.close_code(),
        message: &disconnect.message,
    };
    if let Ok(json) = serde_json::to_string(&frame) {
        let _ = socket.send(Message::Text(json.into())).await;
    }
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: disconnect.close_code(),
            reason: close_reason(&disconnect.message).into(),
        })))
        .await;
}

async fn send_disconnect<S>(sender: &mut S, disconnect: &Disconnect) -> Result<(), S::Error>
where
    S: futures_util::Sink<Message> + Unpin,
{
    let frame = ErrorFrame {
        frame_type: "error",
        code: disconnect.close_code(),
        message: &disconnect.message,
    };
    if let Ok(json) = serde_json::to_string(&frame) {
        sender.send(Message::Text(json.into())).await?;
    }
    sender
        .send(Message::Close(Some(CloseFrame {
            code: disconnect.close_code(),
            reason: close_reason(&disconnect.message).into(),
        })))
        .await
}

fn close_reason(message: &str) -> String {
    message.chars().take(100).collect()
}

fn disconnect_for_error(error: &AgentHostError) -> Disconnect {
    match error {
        AgentHostError::SessionNotFound { .. } => Disconnect::gone(error.to_string()),
        AgentHostError::Conflict { .. }
        | AgentHostError::Validation { .. }
        | AgentHostError::TerminalAttachmentNotFound { .. } => {
            Disconnect::conflict(error.to_string())
        }
        AgentHostError::UpstreamUnavailable { .. }
        | AgentHostError::Docker { .. }
        | AgentHostError::Http { .. } => Disconnect::upstream_unavailable(error.to_string()),
        AgentHostError::Runtime { .. }
        | AgentHostError::Io { .. }
        | AgentHostError::Json { .. } => Disconnect::internal(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        pin::Pin,
        sync::{Arc, Mutex as StdMutex, MutexGuard, PoisonError},
        task::{Context, Poll},
    };

    use async_trait::async_trait;
    use axum::{
        Router,
        body::{Body, Bytes},
        http::{Method, Request, StatusCode},
    };
    use futures_util::stream;
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tokio::{
        io::AsyncWrite,
        sync::{mpsc, watch},
        time::{Duration, sleep, timeout},
    };
    use tower::ServiceExt;

    use super::{
        CLOSE_BACKPRESSURE, CLOSE_CONFLICT, CLOSE_GONE, CLOSE_INTERNAL, CLOSE_UPSTREAM_UNAVAILABLE,
        ClientFrame, Disconnect, DisconnectKind, TerminalAttachKind, TerminalClient,
        TerminalConnection, TerminalExec, TerminalService, TerminalState, TerminalStatus,
        disconnect_for_error, spawn_io_tasks, validate_dimensions,
    };
    use crate::{
        AppConfig, AppState, build_router,
        errors::AgentHostError,
        models::{
            CleanupReport, DockerStatsSummary, HarnessName, InteractionMode, KernelEvent,
            KernelRuntimeSession, KernelStatus, RuntimeSessionSummary,
        },
        sessions::{EventStream, KernelRuntime, RuntimeCreateSession, SessionRegistry},
    };

    const SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";

    #[derive(Clone)]
    struct FakeTerminalRuntime {
        state: Arc<StdMutex<FakeTerminalState>>,
    }

    struct FakeTerminalState {
        terminal: TerminalStatus,
        next_pid: i64,
        inputs: BTreeMap<String, Arc<StdMutex<Vec<u8>>>>,
        outputs: BTreeMap<String, mpsc::Sender<Result<Bytes, AgentHostError>>>,
        exec_pids: BTreeMap<String, i64>,
        resizes: Vec<(String, u16, u16)>,
        detached: Vec<String>,
        copied: Vec<String>,
        controls: Vec<&'static str>,
        created: Vec<RuntimeCreateSession>,
    }

    impl Default for FakeTerminalRuntime {
        fn default() -> Self {
            Self::with_status(running_status(Vec::new()))
        }
    }

    impl FakeTerminalRuntime {
        fn with_status(terminal: TerminalStatus) -> Self {
            Self {
                state: Arc::new(StdMutex::new(FakeTerminalState {
                    terminal,
                    next_pid: 1_000,
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    exec_pids: BTreeMap::new(),
                    resizes: Vec::new(),
                    detached: Vec::new(),
                    copied: Vec::new(),
                    controls: Vec::new(),
                    created: Vec::new(),
                })),
            }
        }

        fn state(&self) -> MutexGuard<'_, FakeTerminalState> {
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }

        async fn send_output(&self, exec_id: &str, bytes: &'static [u8]) {
            let sender = self
                .state()
                .outputs
                .get(exec_id)
                .cloned()
                .unwrap_or_else(|| panic!("missing output sender for {exec_id}"));
            sender
                .send(Ok(Bytes::from_static(bytes)))
                .await
                .unwrap_or_else(|_| panic!("output receiver closed for {exec_id}"));
        }

        fn remove_client(&self, client_id: &str) {
            let mut state = self.state();
            state
                .terminal
                .clients
                .retain(|client| client.id != client_id);
            state.terminal.attachment_count = state.terminal.clients.len();
        }
    }

    #[derive(Clone)]
    struct RecordingWriter {
        bytes: Arc<StdMutex<Vec<u8>>>,
    }

    impl AsyncWrite for RecordingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<Result<usize, std::io::Error>> {
            self.bytes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buffer);
            Poll::Ready(Ok(buffer.len()))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Result<(), std::io::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Result<(), std::io::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    struct PendingWriter;

    impl AsyncWrite for PendingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &[u8],
        ) -> Poll<Result<usize, std::io::Error>> {
            Poll::Pending
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Result<(), std::io::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Result<(), std::io::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    #[async_trait]
    impl KernelRuntime for FakeTerminalRuntime {
        async fn create_session(
            &self,
            request: RuntimeCreateSession,
        ) -> Result<KernelRuntimeSession, AgentHostError> {
            self.state().created.push(request.clone());
            Ok(KernelRuntimeSession::opaque(request.session_id))
        }

        fn stream_message(
            &self,
            _session: KernelRuntimeSession,
            _message: String,
        ) -> Result<EventStream, AgentHostError> {
            Ok(Box::pin(stream::empty()))
        }

        async fn summary(
            &self,
            _session: &KernelRuntimeSession,
        ) -> Result<RuntimeSessionSummary, AgentHostError> {
            Ok(RuntimeSessionSummary {
                status: Some(KernelStatus::Idle),
                resume_token: None,
                vscode_url: None,
                free_port_url: None,
            })
        }

        async fn history(
            &self,
            _session: &KernelRuntimeSession,
        ) -> Result<Vec<Vec<KernelEvent>>, AgentHostError> {
            Ok(Vec::new())
        }

        async fn logs(
            &self,
            _session: &KernelRuntimeSession,
        ) -> Result<Vec<String>, AgentHostError> {
            Ok(Vec::new())
        }

        async fn container_logs(
            &self,
            _session: &KernelRuntimeSession,
            _tail: Option<u32>,
        ) -> Result<Vec<String>, AgentHostError> {
            Ok(Vec::new())
        }

        async fn stats(
            &self,
            _session: &KernelRuntimeSession,
        ) -> Result<Option<DockerStatsSummary>, AgentHostError> {
            Ok(None)
        }

        fn container_name(&self, _session: &KernelRuntimeSession) -> Option<String> {
            Some("fake-kernel".to_owned())
        }

        fn vscode_url(&self, _session: &KernelRuntimeSession) -> Option<String> {
            None
        }

        fn free_port_url(&self, _session: &KernelRuntimeSession) -> Option<String> {
            None
        }

        async fn destroy_session(
            &self,
            _session: KernelRuntimeSession,
        ) -> Result<(), AgentHostError> {
            Ok(())
        }

        async fn destroy_session_by_id(&self, _session_id: &str) -> Result<(), AgentHostError> {
            Ok(())
        }

        async fn cleanup_orphans(
            &self,
            owned_session_ids: &BTreeSet<String>,
            dry_run: bool,
        ) -> Result<CleanupReport, AgentHostError> {
            Ok(CleanupReport {
                dry_run,
                owned_session_count: owned_session_ids.len(),
                resources: Vec::new(),
                deleted_count: 0,
                error_count: 0,
            })
        }

        async fn snapshot_session_workspace(
            &self,
            _session: &KernelRuntimeSession,
            _workspace_id: String,
            _volume_name: String,
            _exclude_paths: Vec<String>,
        ) -> Result<Value, AgentHostError> {
            Ok(json!({}))
        }

        async fn clone_workspace(
            &self,
            _source_volume_name: String,
            _target_workspace_id: String,
            _target_volume_name: String,
        ) -> Result<Value, AgentHostError> {
            Ok(json!({}))
        }

        async fn open_workspace_vscode(
            &self,
            _workspace_id: String,
            _volume_name: String,
        ) -> Result<Value, AgentHostError> {
            Ok(json!({}))
        }

        async fn terminal_status(
            &self,
            _session: &KernelRuntimeSession,
        ) -> Result<TerminalStatus, AgentHostError> {
            let mut status = self.state().terminal.clone();
            status.attachment_count = status.clients.len();
            Ok(status)
        }

        async fn terminal_ensure(
            &self,
            _session: &KernelRuntimeSession,
        ) -> Result<TerminalStatus, AgentHostError> {
            let mut state = self.state();
            state.controls.push("ensure");
            if state.terminal.state == TerminalState::Missing {
                state.terminal.state = TerminalState::Running;
                state.terminal.attach_kind = Some(TerminalAttachKind::Started);
            } else {
                state.terminal.attach_kind = Some(TerminalAttachKind::Attached);
            }
            Ok(state.terminal.clone())
        }

        async fn terminal_stop(
            &self,
            _session: &KernelRuntimeSession,
        ) -> Result<TerminalStatus, AgentHostError> {
            let mut state = self.state();
            state.controls.push("stop");
            state.terminal.state = TerminalState::Missing;
            state.terminal.clients.clear();
            state.terminal.attachment_count = 0;
            Ok(state.terminal.clone())
        }

        async fn terminal_resume(
            &self,
            _session: &KernelRuntimeSession,
        ) -> Result<TerminalStatus, AgentHostError> {
            let mut state = self.state();
            state.controls.push("resume");
            if state.terminal.state != TerminalState::Exited {
                return Err(AgentHostError::conflict(
                    "resume requires an exited terminal",
                ));
            }
            state.terminal.state = TerminalState::Running;
            state.terminal.attach_kind = Some(TerminalAttachKind::Resumed);
            Ok(state.terminal.clone())
        }

        async fn terminal_copy_mode(
            &self,
            _session: &KernelRuntimeSession,
            tmux_client_id: &str,
        ) -> Result<TerminalStatus, AgentHostError> {
            let mut state = self.state();
            state.copied.push(tmux_client_id.to_owned());
            Ok(state.terminal.clone())
        }

        async fn terminal_detach_client(
            &self,
            _session: &KernelRuntimeSession,
            tmux_client_id: &str,
        ) -> Result<TerminalStatus, AgentHostError> {
            let mut state = self.state();
            if !state
                .terminal
                .clients
                .iter()
                .any(|client| client.id == tmux_client_id)
            {
                return Err(AgentHostError::terminal_attachment_not_found(
                    tmux_client_id,
                ));
            }
            state.detached.push(tmux_client_id.to_owned());
            let exec_id = state.exec_pids.iter().find_map(|(exec_id, pid)| {
                state
                    .terminal
                    .clients
                    .iter()
                    .find(|client| client.id == tmux_client_id && client.pid == *pid)
                    .map(|_| exec_id.clone())
            });
            state
                .terminal
                .clients
                .retain(|client| client.id != tmux_client_id);
            state.terminal.attachment_count = state.terminal.clients.len();
            if let Some(exec_id) = exec_id {
                state.outputs.remove(&exec_id);
                state.exec_pids.remove(&exec_id);
            }
            Ok(state.terminal.clone())
        }

        async fn terminal_attach(
            &self,
            _session: &KernelRuntimeSession,
            attachment_id: &str,
            attach_argv: &[String],
        ) -> Result<TerminalExec, AgentHostError> {
            if attach_argv != ["tmux", "attach-session", "-t", "=agentspace-test"] {
                return Err(AgentHostError::conflict("unexpected attach argv"));
            }
            let mut state = self.state();
            state.next_pid += 1;
            let pid = state.next_pid;
            let exec_id = format!("exec-{pid}");
            let client_id = format!("/dev/pts/{pid}");
            state.terminal.clients.push(TerminalClient {
                id: client_id.clone(),
                tty: client_id,
                pid,
                width: 80,
                height: 24,
                session_name: "agentspace-test".to_owned(),
                pane_id: "%0".to_owned(),
                attachment_id: Some(attachment_id.to_owned()),
            });
            state.terminal.attachment_count = state.terminal.clients.len();
            state.exec_pids.insert(exec_id.clone(), pid);
            let bytes = Arc::new(StdMutex::new(Vec::new()));
            state.inputs.insert(exec_id.clone(), bytes.clone());
            let (output_tx, output_rx) = mpsc::channel(16);
            state.outputs.insert(exec_id.clone(), output_tx);
            drop(state);
            let output = stream::unfold(output_rx, |mut receiver| async move {
                receiver.recv().await.map(|item| (item, receiver))
            });
            Ok(TerminalExec {
                exec_id,
                input: Box::pin(RecordingWriter { bytes }),
                output: Box::pin(output),
            })
        }

        async fn terminal_resize(
            &self,
            _session: &KernelRuntimeSession,
            exec_id: &str,
            cols: u16,
            rows: u16,
        ) -> Result<(), AgentHostError> {
            let mut state = self.state();
            let pid = *state
                .exec_pids
                .get(exec_id)
                .unwrap_or_else(|| panic!("missing exec {exec_id}"));
            state.resizes.push((exec_id.to_owned(), cols, rows));
            if let Some(client) = state
                .terminal
                .clients
                .iter_mut()
                .find(|client| client.pid == pid)
            {
                client.width = cols;
                client.height = rows;
            }
            drop(state);
            Ok(())
        }
    }

    fn running_status(clients: Vec<TerminalClient>) -> TerminalStatus {
        TerminalStatus {
            state: TerminalState::Running,
            exit_status: None,
            attach_kind: None,
            session_name: "agentspace-test".to_owned(),
            target_session: "=agentspace-test".to_owned(),
            socket_path: "/run/agentspace-tmux.sock".to_owned(),
            attach_argv: vec![
                "tmux".to_owned(),
                "attach-session".to_owned(),
                "-t".to_owned(),
                "=agentspace-test".to_owned(),
            ],
            pane_id: Some("%0".to_owned()),
            pane_pid: Some(42),
            attachment_count: clients.len(),
            clients,
        }
    }

    fn runtime_session() -> KernelRuntimeSession {
        KernelRuntimeSession::opaque(SESSION_ID)
    }

    async fn detach_connection(service: &TerminalService, connection: &TerminalConnection) {
        service
            .detach(SESSION_ID, &runtime_session(), &connection.attachment_id)
            .await
            .unwrap_or_else(|error| panic!("detach failed: {error}"));
        connection.abort_tasks();
    }

    #[tokio::test]
    async fn attachment_forwards_io_resizes_and_cleans_exact_client() {
        let runtime = FakeTerminalRuntime::default();
        let service = TerminalService::new(Arc::new(runtime.clone()));
        let mut first = service
            .attach(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("first attach failed: {error}"));
        let second = service
            .attach(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("second attach failed: {error}"));

        first
            .input
            .send(Bytes::from_static(b"hello"))
            .await
            .unwrap_or_else(|_| panic!("input queue closed"));
        runtime.send_output("exec-1001", b"world").await;
        assert_eq!(
            first.output.recv().await,
            Some(Bytes::from_static(b"world"))
        );
        service
            .resize(
                SESSION_ID,
                &runtime_session(),
                &first.attachment_id,
                132,
                43,
            )
            .await
            .unwrap_or_else(|error| panic!("resize failed: {error}"));
        sleep(Duration::from_millis(10)).await;

        {
            let state = runtime.state();
            let recorded = state.inputs["exec-1001"]
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            assert_eq!(recorded, b"hello");
            assert!(state.resizes.contains(&("exec-1001".to_owned(), 132, 43)));
            drop(state);
        }

        detach_connection(&service, &first).await;
        let status = service
            .status(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("status failed: {error}"));
        assert_eq!(status.attachment_count, 1);
        assert_eq!(runtime.state().detached, vec!["/dev/pts/1001"]);
        detach_connection(&service, &second).await;
    }

    #[tokio::test]
    async fn simultaneous_clients_keep_independent_mixed_sizes() {
        let runtime = FakeTerminalRuntime::default();
        let service = TerminalService::new(Arc::new(runtime.clone()));
        let first = service
            .attach(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("first attach failed: {error}"));
        let second = service
            .attach(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("second attach failed: {error}"));

        service
            .resize(SESSION_ID, &runtime_session(), &first.attachment_id, 90, 30)
            .await
            .unwrap_or_else(|error| panic!("first resize failed: {error}"));
        service
            .resize(
                SESSION_ID,
                &runtime_session(),
                &second.attachment_id,
                160,
                55,
            )
            .await
            .unwrap_or_else(|error| panic!("second resize failed: {error}"));

        let status = service
            .status(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("status failed: {error}"));
        assert_eq!(status.attachment_count, 2);
        assert_eq!(
            status
                .clients
                .iter()
                .map(|client| (client.width, client.height))
                .collect::<Vec<_>>(),
            vec![(90, 30), (160, 55)]
        );
        detach_connection(&service, &first).await;
        detach_connection(&service, &second).await;
    }

    #[tokio::test]
    async fn adoption_and_phantom_reconciliation_use_observed_tmux_clients() {
        let stale_clients = vec![
            TerminalClient {
                id: "/dev/pts/7".to_owned(),
                tty: "/dev/pts/7".to_owned(),
                pid: 7,
                width: 80,
                height: 24,
                session_name: "agentspace-test".to_owned(),
                pane_id: "%0".to_owned(),
                attachment_id: None,
            },
            TerminalClient {
                id: "/dev/pts/8".to_owned(),
                tty: "/dev/pts/8".to_owned(),
                pid: 8,
                width: 80,
                height: 24,
                session_name: "agentspace-test".to_owned(),
                pane_id: "%0".to_owned(),
                attachment_id: None,
            },
        ];
        let runtime = FakeTerminalRuntime::with_status(running_status(stale_clients));
        let service = TerminalService::new(Arc::new(runtime.clone()));

        let adopted = service
            .reconcile_adoption(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("adoption reconcile failed: {error}"));
        assert_eq!(adopted.attachment_count, 0);
        assert_eq!(runtime.state().detached, vec!["/dev/pts/7", "/dev/pts/8"]);

        let mut connection = service
            .attach(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("attach failed: {error}"));
        runtime.remove_client("/dev/pts/1001");
        service
            .status(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("phantom reconcile failed: {error}"));
        connection
            .lifecycle
            .changed()
            .await
            .unwrap_or_else(|_| panic!("lifecycle channel closed"));
        let disconnect = connection
            .lifecycle
            .borrow()
            .clone()
            .unwrap_or_else(|| panic!("missing phantom disconnect"));
        assert_eq!(disconnect.kind, DisconnectKind::Conflict);
        assert_eq!(disconnect.close_code(), CLOSE_CONFLICT);
        connection.abort_tasks();
    }

    #[tokio::test]
    async fn bounded_output_overflow_detaches_with_backpressure_code() {
        let runtime = FakeTerminalRuntime::default();
        let service = TerminalService::with_queue_capacity(Arc::new(runtime.clone()), 1);
        let mut connection = service
            .attach(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("attach failed: {error}"));

        runtime.send_output("exec-1001", b"one").await;
        runtime.send_output("exec-1001", b"two").await;
        timeout(Duration::from_secs(1), connection.lifecycle.changed())
            .await
            .unwrap_or_else(|_| panic!("overflow was not reported"))
            .unwrap_or_else(|_| panic!("lifecycle channel closed"));
        let disconnect = connection
            .lifecycle
            .borrow()
            .clone()
            .unwrap_or_else(|| panic!("missing overflow disconnect"));
        assert_eq!(disconnect.kind, DisconnectKind::Backpressure);
        assert_eq!(disconnect.close_code(), CLOSE_BACKPRESSURE);
        detach_connection(&service, &connection).await;
    }

    #[tokio::test]
    async fn inbound_exec_queue_is_bounded() {
        let (input_tx, input_rx) = mpsc::channel(1);
        let (output_tx, _output_rx) = mpsc::channel(1);
        let (lifecycle_tx, _lifecycle_rx) = watch::channel(None);
        let tasks = spawn_io_tasks(
            Box::pin(PendingWriter),
            Box::pin(stream::pending()),
            input_rx,
            output_tx,
            lifecycle_tx,
        );

        input_tx
            .send(Bytes::from_static(b"writing"))
            .await
            .unwrap_or_else(|_| panic!("input task closed"));
        sleep(Duration::from_millis(10)).await;
        input_tx
            .try_send(Bytes::from_static(b"queued"))
            .unwrap_or_else(|_| panic!("bounded queue should accept one waiting frame"));
        assert!(matches!(
            input_tx.try_send(Bytes::from_static(b"overflow")),
            Err(mpsc::error::TrySendError::Full(_))
        ));
        for task in tasks {
            task.abort();
        }
    }

    #[tokio::test]
    async fn controls_proxy_status_and_attachment_copy_mode() {
        let runtime = FakeTerminalRuntime::default();
        let service = TerminalService::new(Arc::new(runtime.clone()));
        let connection = service
            .attach(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("attach failed: {error}"));
        service
            .copy_mode(SESSION_ID, &runtime_session(), &connection.attachment_id)
            .await
            .unwrap_or_else(|error| panic!("copy mode failed: {error}"));
        service
            .stop(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("stop failed: {error}"));
        {
            let mut state = runtime.state();
            state.terminal.state = TerminalState::Exited;
        }
        let resumed = service
            .resume(SESSION_ID, &runtime_session())
            .await
            .unwrap_or_else(|error| panic!("resume failed: {error}"));
        assert_eq!(resumed.attach_kind, Some(TerminalAttachKind::Resumed));
        let state = runtime.state();
        assert_eq!(state.copied, vec!["/dev/pts/1001"]);
        assert!(state.controls.contains(&"stop"));
        assert!(state.controls.contains(&"resume"));
        drop(state);
        connection.abort_tasks();
    }

    #[test]
    fn invalid_frames_dimensions_and_close_codes_are_explicit() {
        assert!(serde_json::from_str::<ClientFrame>(r#"{"type":"input"}"#).is_err());
        assert!(
            serde_json::from_str::<ClientFrame>(
                r#"{"type":"resize","cols":80,"rows":24,"extra":true}"#
            )
            .is_err()
        );
        assert!(validate_dimensions(0, 24).is_err());
        assert!(validate_dimensions(80, 1_001).is_err());
        assert_eq!(
            disconnect_for_error(&AgentHostError::session_not_found("gone")).close_code(),
            CLOSE_GONE
        );
        assert_eq!(
            disconnect_for_error(&AgentHostError::conflict("conflict")).close_code(),
            CLOSE_CONFLICT
        );
        assert_eq!(
            disconnect_for_error(&AgentHostError::upstream_unavailable("down")).close_code(),
            CLOSE_UPSTREAM_UNAVAILABLE
        );
        assert_eq!(
            disconnect_for_error(&AgentHostError::runtime("bug")).close_code(),
            CLOSE_INTERNAL
        );
        assert_eq!(
            Disconnect::backpressure("full").close_code(),
            CLOSE_BACKPRESSURE
        );
    }

    #[tokio::test]
    async fn routes_reject_missing_and_chat_sessions_and_proxy_cli_status() {
        let runtime = FakeTerminalRuntime::default();
        let mut state = AppState::new(AppConfig::new("127.0.0.1", 0, BTreeMap::new()));
        state.sessions = SessionRegistry::with_runtime(Arc::new(runtime));
        let app = build_router(state);

        let missing = request_json(&app, Method::GET, "/sessions/missing/terminal", None).await;
        assert_eq!(missing.0, StatusCode::NOT_FOUND);

        let chat = request_json(
            &app,
            Method::POST,
            "/sessions",
            Some(json!({
                "session_id": "chat-session",
                "harness": "echo",
                "interaction_mode": "chat"
            })),
        )
        .await;
        assert_eq!(chat.0, StatusCode::OK);
        let wrong_mode =
            request_json(&app, Method::GET, "/sessions/chat-session/terminal", None).await;
        assert_eq!(wrong_mode.0, StatusCode::CONFLICT);

        let cli = request_json(
            &app,
            Method::POST,
            "/sessions",
            Some(json!({
                "session_id": SESSION_ID,
                "harness": "copilot-cli",
                "interaction_mode": "cli"
            })),
        )
        .await;
        assert_eq!(cli.0, StatusCode::OK);
        let status = request_json(
            &app,
            Method::GET,
            &format!("/sessions/{SESSION_ID}/terminal"),
            None,
        )
        .await;
        assert_eq!(status.0, StatusCode::OK);
        assert_eq!(status.1["state"], "running");
    }

    #[tokio::test]
    async fn recreating_registry_reconciles_restart_orphans() {
        let runtime = FakeTerminalRuntime::default();
        let first_registry = SessionRegistry::with_runtime(Arc::new(runtime.clone()));
        first_registry
            .create_session(crate::sessions::CreateSessionRequest {
                session_id: Some(SESSION_ID.to_owned()),
                harness: HarnessName::CopilotCli,
                interaction_mode: InteractionMode::Cli,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await
            .unwrap_or_else(|error| panic!("initial create failed: {error}"));
        {
            let mut state = runtime.state();
            state.terminal.clients.push(TerminalClient {
                id: "/dev/pts/77".to_owned(),
                tty: "/dev/pts/77".to_owned(),
                pid: 77,
                width: 80,
                height: 24,
                session_name: "agentspace-test".to_owned(),
                pane_id: "%0".to_owned(),
                attachment_id: None,
            });
            state.terminal.attachment_count = 1;
        }

        let restarted_registry = SessionRegistry::with_runtime(Arc::new(runtime.clone()));
        restarted_registry
            .create_session(crate::sessions::CreateSessionRequest {
                session_id: Some(SESSION_ID.to_owned()),
                harness: HarnessName::CopilotCli,
                interaction_mode: InteractionMode::Cli,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await
            .unwrap_or_else(|error| panic!("restart adoption failed: {error}"));

        assert!(runtime.state().detached.contains(&"/dev/pts/77".to_owned()));
    }

    async fn request_json(
        app: &Router,
        method: Method,
        uri: &str,
        payload: Option<Value>,
    ) -> (StatusCode, Value) {
        let has_payload = payload.is_some();
        let body = payload.map_or_else(Body::empty, |payload| Body::from(payload.to_string()));
        let mut request = Request::builder().method(method).uri(uri);
        if has_payload {
            request = request.header("content-type", "application/json");
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(body)
                    .unwrap_or_else(|error| panic!("request build failed: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("request failed: {error}"));
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("body read failed: {error}"))
            .to_bytes();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("JSON parse failed: {error}"))
        };
        (status, value)
    }
}
