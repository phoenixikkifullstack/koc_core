use crate::proxy_capture::{Direction, ObservedFrame};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};
use tokio_tungstenite::{accept_hdr_async_with_config, connect_async_with_config};
use url::Url;

const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const CLOSE_GRACE_PERIOD: std::time::Duration = std::time::Duration::from_secs(2);
const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;
const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RelayConfig {
    pub listen: SocketAddr,
    pub upstream_origin: Url,
    pub max_connections: usize,
    pub max_capture_queue_bytes: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RelayStats {
    pub accepted_connections: u64,
    pub dropped_queue_full: u64,
    pub dropped_analyzer_closed: u64,
}

#[derive(Default)]
struct CaptureDropCounters {
    queue_full: AtomicU64,
    analyzer_closed: AtomicU64,
}

pub async fn run_relay(
    config: RelayConfig,
    capture_tx: Option<mpsc::Sender<ObservedFrame>>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<RelayStats, String> {
    validate_upstream_origin(&config.upstream_origin)?;
    if !config.listen.ip().is_loopback() {
        return Err("relay must listen on a loopback address".to_string());
    }
    if config.max_connections == 0 {
        return Err("max_connections must be greater than zero".to_string());
    }
    if config.max_capture_queue_bytes == 0 || config.max_capture_queue_bytes > u32::MAX as usize {
        return Err("max_capture_queue_bytes must be between 1 and u32::MAX".to_string());
    }

    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|e| format!("failed to bind relay on {}: {}", config.listen, e))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| format!("failed to read relay address: {}", e))?;
    eprintln!("koc_proxy relay listening on ws://{}", local_addr);
    eprintln!("route the client with KOC_WS_BASE_URL=ws://{}", local_addr);

    let connection_counter = Arc::new(AtomicU64::new(1));
    let session_id = format!(
        "{:x}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        std::process::id()
    );
    let dropped_records = Arc::new(CaptureDropCounters::default());
    let capture_budget = Arc::new(Semaphore::new(config.max_capture_queue_bytes));
    let accepted_connections = Arc::new(AtomicU64::new(0));
    let connection_limit = Arc::new(Semaphore::new(config.max_connections));
    let mut connections = JoinSet::new();

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result.map_err(|e| format!("relay accept failed: {}", e))?;
                let permit = match connection_limit.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        eprintln!("koc_proxy connection limit reached; rejecting client");
                        drop(stream);
                        continue;
                    }
                };
                let connection_number = connection_counter.fetch_add(1, Ordering::Relaxed);
                let connection_id = format!("{}-conn-{}", session_id, connection_number);
                let upstream_origin = config.upstream_origin.clone();
                let capture_tx = capture_tx.clone();
                let dropped_records = dropped_records.clone();
                let capture_budget = capture_budget.clone();
                let connection_shutdown = shutdown.clone();
                accepted_connections.fetch_add(1, Ordering::Relaxed);
                connections.spawn(async move {
                    let _permit = permit;
                    if let Err(error) = relay_connection(
                        stream,
                        connection_id.clone(),
                        upstream_origin,
                        capture_tx,
                        dropped_records,
                        capture_budget,
                        connection_shutdown,
                    ).await {
                        eprintln!("koc_proxy {} closed with error: {}", connection_id, error);
                    }
                });
            }
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = result {
                    eprintln!("koc_proxy connection task failed: {}", error);
                }
            }
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    eprintln!("koc_proxy relay shutting down");
                    break;
                }
            }
        }
    }

    let graceful_shutdown = tokio::time::timeout(CLOSE_GRACE_PERIOD, async {
        while let Some(result) = connections.join_next().await {
            if let Err(error) = result {
                eprintln!(
                    "koc_proxy connection task failed during shutdown: {}",
                    error
                );
            }
        }
    })
    .await;
    if graceful_shutdown.is_err() {
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
    drop(capture_tx);

    Ok(RelayStats {
        accepted_connections: accepted_connections.load(Ordering::Relaxed),
        dropped_queue_full: dropped_records.queue_full.load(Ordering::Relaxed),
        dropped_analyzer_closed: dropped_records.analyzer_closed.load(Ordering::Relaxed),
    })
}

async fn relay_connection(
    stream: TcpStream,
    connection_id: String,
    upstream_origin: Url,
    capture_tx: Option<mpsc::Sender<ObservedFrame>>,
    dropped_records: Arc<CaptureDropCounters>,
    capture_budget: Arc<Semaphore>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let request_target = Arc::new(Mutex::new(None::<String>));
    let request_target_clone = request_target.clone();
    let websocket_config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_SIZE))
        .max_frame_size(Some(MAX_FRAME_SIZE));
    let client_ws = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        accept_hdr_async_with_config(
            stream,
            move |request: &Request, response: Response| {
                if let Ok(mut target) = request_target_clone.lock() {
                    *target = Some(request.uri().to_string());
                }
                Ok(response)
            },
            Some(websocket_config),
        ),
    )
    .await
    .map_err(|_| "client WebSocket handshake timed out".to_string())?
    .map_err(|e| format!("client WebSocket handshake failed: {}", e))?;

    let target = request_target
        .lock()
        .map_err(|_| "client request target lock poisoned".to_string())?
        .take()
        .ok_or_else(|| "client WebSocket request target missing".to_string())?;
    let upstream_url = build_upstream_url(&upstream_origin, &target)?;
    let (upstream_ws, _) = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        connect_async_with_config(upstream_url.as_str(), Some(websocket_config), false),
    )
    .await
    .map_err(|_| "upstream WebSocket connection timed out".to_string())?
    .map_err(|e| format!("upstream WebSocket connection failed: {}", e))?;

    eprintln!("koc_proxy {} connected", connection_id);
    let (client_write, client_read) = client_ws.split();
    let (upstream_write, upstream_read) = upstream_ws.split();
    let frame_index = Arc::new(AtomicU64::new(1));
    let (connection_shutdown_tx, connection_shutdown_rx) = watch::channel(false);

    let client_to_server = relay_direction(
        client_read,
        upstream_write,
        connection_id.clone(),
        Direction::ClientToServer,
        frame_index.clone(),
        capture_tx.clone(),
        dropped_records.clone(),
        capture_budget.clone(),
        shutdown.clone(),
        connection_shutdown_rx.clone(),
    );
    let server_to_client = relay_direction(
        upstream_read,
        client_write,
        connection_id.clone(),
        Direction::ServerToClient,
        frame_index,
        capture_tx,
        dropped_records,
        capture_budget,
        shutdown,
        connection_shutdown_rx,
    );
    tokio::pin!(client_to_server);
    tokio::pin!(server_to_client);

    let (first_result, remaining) = tokio::select! {
        result = &mut client_to_server => (result, RemainingDirection::ServerToClient),
        result = &mut server_to_client => (result, RemainingDirection::ClientToServer),
    };
    let _ = connection_shutdown_tx.send(true);
    let second_result = match remaining {
        RemainingDirection::ClientToServer => {
            tokio::time::timeout(CLOSE_GRACE_PERIOD, &mut client_to_server).await
        }
        RemainingDirection::ServerToClient => {
            tokio::time::timeout(CLOSE_GRACE_PERIOD, &mut server_to_client).await
        }
    };
    first_result?;
    match second_result {
        Ok(result) => result?,
        Err(_) => eprintln!("koc_proxy {} close grace period expired", connection_id),
    }
    eprintln!("koc_proxy {} disconnected", connection_id);
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum RemainingDirection {
    ClientToServer,
    ServerToClient,
}

#[allow(clippy::too_many_arguments)]
async fn relay_direction<R, W>(
    mut reader: R,
    mut writer: W,
    connection_id: String,
    direction: Direction,
    frame_index: Arc<AtomicU64>,
    capture_tx: Option<mpsc::Sender<ObservedFrame>>,
    dropped_records: Arc<CaptureDropCounters>,
    capture_budget: Arc<Semaphore>,
    mut process_shutdown: watch::Receiver<bool>,
    mut connection_shutdown: watch::Receiver<bool>,
) -> Result<(), String>
where
    R: Stream<Item = Result<Message, WebSocketError>> + Unpin,
    W: Sink<Message, Error = WebSocketError> + Unpin,
{
    loop {
        let frame = tokio::select! {
            result = process_shutdown.changed() => {
                if result.is_err() || *process_shutdown.borrow() {
                    send_close(&mut writer).await;
                    return Ok(());
                }
                continue;
            }
            result = connection_shutdown.changed() => {
                if result.is_err() || *connection_shutdown.borrow() {
                    send_close(&mut writer).await;
                    return Ok(());
                }
                continue;
            }
            frame = reader.next() => frame,
        };
        let Some(frame) = frame else { return Ok(()) };
        let message = frame.map_err(|e| format!("{} WebSocket read failed: {}", direction, e))?;
        let index = frame_index.fetch_add(1, Ordering::Relaxed);
        let observed = prepare_record(
            capture_tx.as_ref(),
            &capture_budget,
            &connection_id,
            index,
            direction,
            &message,
            &dropped_records,
        );
        let is_close = matches!(message, Message::Close(_));
        forward_message(&mut writer, message, direction, is_close).await?;
        if let (Some(capture_tx), Some(observed)) = (capture_tx.as_ref(), observed) {
            enqueue_record(capture_tx, observed, &dropped_records);
        }
        if is_close {
            break;
        }
    }
    Ok(())
}

async fn send_close<W>(writer: &mut W)
where
    W: Sink<Message, Error = WebSocketError> + Unpin,
{
    let _ = tokio::time::timeout(WRITE_TIMEOUT, writer.flush()).await;
    let _ = tokio::time::timeout(WRITE_TIMEOUT, writer.send(Message::Close(None))).await;
}

async fn forward_message<W>(
    writer: &mut W,
    message: Message,
    direction: Direction,
    is_close: bool,
) -> Result<(), String>
where
    W: Sink<Message, Error = WebSocketError> + Unpin,
{
    if is_close {
        match tokio::time::timeout(WRITE_TIMEOUT, writer.flush()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if is_closed_error(&error) => return Ok(()),
            Ok(Err(error)) => {
                return Err(format!("{} WebSocket flush failed: {}", direction, error));
            }
            Err(_) => return Err(format!("{} WebSocket flush timed out", direction)),
        }
    }
    match tokio::time::timeout(WRITE_TIMEOUT, writer.send(message)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) if is_close && is_closed_error(&error) => Ok(()),
        Ok(Err(error)) => Err(format!("{} WebSocket write failed: {}", direction, error)),
        Err(_) => Err(format!("{} WebSocket write timed out", direction)),
    }
}

fn is_closed_error(error: &WebSocketError) -> bool {
    matches!(
        error,
        WebSocketError::ConnectionClosed
            | WebSocketError::AlreadyClosed
            | WebSocketError::Protocol(ProtocolError::SendAfterClosing)
    )
}

fn enqueue_record(
    capture_tx: &mpsc::Sender<ObservedFrame>,
    record: ObservedFrame,
    dropped_records: &CaptureDropCounters,
) {
    if let Err(error) = capture_tx.try_send(record) {
        match error {
            mpsc::error::TrySendError::Full(_) => {
                let dropped = dropped_records.queue_full.fetch_add(1, Ordering::Relaxed) + 1;
                if dropped == 1 || dropped % 100 == 0 {
                    eprintln!(
                        "koc_proxy capture channel full; dropped {} records",
                        dropped
                    );
                }
            }
            mpsc::error::TrySendError::Closed(_) => {
                let dropped = dropped_records
                    .analyzer_closed
                    .fetch_add(1, Ordering::Relaxed)
                    + 1;
                if dropped == 1 || dropped % 100 == 0 {
                    eprintln!(
                        "koc_proxy analyzer unavailable; dropped {} records",
                        dropped
                    );
                }
            }
        }
    }
}

fn prepare_record(
    capture_tx: Option<&mpsc::Sender<ObservedFrame>>,
    capture_budget: &Arc<Semaphore>,
    connection_id: &str,
    frame_index: u64,
    direction: Direction,
    message: &Message,
    dropped_records: &CaptureDropCounters,
) -> Option<ObservedFrame> {
    let capture_tx = capture_tx?;
    if capture_tx.is_closed() {
        let dropped = dropped_records
            .analyzer_closed
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        if dropped == 1 || dropped % 100 == 0 {
            eprintln!(
                "koc_proxy analyzer unavailable; dropped {} records",
                dropped
            );
        }
        return None;
    }
    let payload_bytes = message.len().max(1);
    let permits = u32::try_from(payload_bytes).ok()?;
    let permit = match capture_budget.clone().try_acquire_many_owned(permits) {
        Ok(permit) => permit,
        Err(_) => {
            let dropped = dropped_records.queue_full.fetch_add(1, Ordering::Relaxed) + 1;
            if dropped == 1 || dropped % 100 == 0 {
                eprintln!(
                    "koc_proxy capture byte budget exhausted; dropped {} records",
                    dropped
                );
            }
            return None;
        }
    };
    Some(ObservedFrame::from_owned_message(
        connection_id,
        frame_index,
        direction,
        message.clone(),
        permit,
    ))
}

fn validate_upstream_origin(origin: &Url) -> Result<(), String> {
    if !matches!(origin.scheme(), "ws" | "wss") {
        return Err("upstream origin must use ws:// or wss://".to_string());
    }
    if origin.host_str().is_none() {
        return Err("upstream origin must include a host".to_string());
    }
    if !origin.username().is_empty() || origin.password().is_some() {
        return Err("upstream origin must not contain credentials".to_string());
    }
    if origin.query().is_some() || origin.fragment().is_some() {
        return Err("upstream origin must not contain a query or fragment".to_string());
    }
    if origin.scheme() == "ws" {
        let host = origin.host_str().unwrap_or_default();
        let is_loopback = host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .map(|address| address.is_loopback())
                .unwrap_or(false);
        if !is_loopback {
            return Err("remote upstream origins must use wss://".to_string());
        }
    }
    Ok(())
}

fn build_upstream_url(origin: &Url, request_target: &str) -> Result<Url, String> {
    let request_url = if let Ok(url) = Url::parse(request_target) {
        url
    } else {
        Url::parse("ws://relay.invalid")
            .and_then(|base| base.join(request_target))
            .map_err(|e| format!("invalid client request target: {}", e))?
    };
    let mut upstream = origin.clone();
    upstream.set_path(request_url.path());
    upstream.set_query(request_url.query());
    upstream.set_fragment(None);
    Ok(upstream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration, timeout};
    use tokio_tungstenite::{accept_async, connect_async};

    #[test]
    fn preserves_request_path_and_query_without_changing_origin() {
        let origin = Url::parse("wss://example.com").unwrap();
        let url = build_upstream_url(&origin, "/agent?p=secret&e=x").unwrap();
        assert_eq!(url.as_str(), "wss://example.com/agent?p=secret&e=x");
    }

    #[test]
    fn rejects_non_websocket_upstream() {
        let origin = Url::parse("https://example.com").unwrap();
        assert!(validate_upstream_origin(&origin).is_err());
    }

    #[test]
    fn rejects_remote_plaintext_upstream() {
        let origin = Url::parse("ws://example.com").unwrap();
        assert!(validate_upstream_origin(&origin).is_err());
        let loopback = Url::parse("ws://127.0.0.1:9000").unwrap();
        assert!(validate_upstream_origin(&loopback).is_ok());
    }

    #[test]
    fn capture_byte_budget_drops_large_queued_message() {
        let (capture_tx, _capture_rx) = mpsc::channel(8);
        let budget = Arc::new(Semaphore::new(4));
        let counters = CaptureDropCounters::default();
        let message = Message::Binary(vec![0; 5].into());
        let observed = prepare_record(
            Some(&capture_tx),
            &budget,
            "test",
            1,
            Direction::ClientToServer,
            &message,
            &counters,
        );
        assert!(observed.is_none());
        assert_eq!(counters.queue_full.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn relays_binary_payload_and_captures_both_directions() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream_listener.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (stream, _) = upstream_listener.accept().await.unwrap();
            let mut websocket = accept_async(stream).await.unwrap();
            let message = websocket.next().await.unwrap().unwrap();
            websocket.send(message).await.unwrap();
            websocket.close(None).await.unwrap();
        });

        let relay_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_addr = relay_listener.local_addr().unwrap();
        let (capture_tx, mut capture_rx) = mpsc::channel(8);
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let relay_task = tokio::spawn(async move {
            let (stream, _) = relay_listener.accept().await.unwrap();
            relay_connection(
                stream,
                "test-connection".to_string(),
                Url::parse(&format!("ws://{}", upstream_addr)).unwrap(),
                Some(capture_tx),
                Arc::new(CaptureDropCounters::default()),
                Arc::new(Semaphore::new(1024 * 1024)),
                shutdown_rx,
            )
            .await
            .unwrap();
        });

        let (mut client, _) = connect_async(format!("ws://{}/agent?p=secret&e=x", relay_addr))
            .await
            .unwrap();
        let payload = vec![112, 120, 1, 2, 3, 4, 5];
        client
            .send(Message::Binary(payload.clone().into()))
            .await
            .unwrap();
        let echoed = client.next().await.unwrap().unwrap();
        assert_eq!(echoed.into_data().as_ref(), payload.as_slice());
        let close = client.next().await.unwrap().unwrap();
        assert!(close.is_close());

        timeout(Duration::from_secs(2), upstream_task)
            .await
            .unwrap()
            .unwrap();
        timeout(Duration::from_secs(2), relay_task)
            .await
            .unwrap()
            .unwrap();

        let first = capture_rx.recv().await.unwrap().into_record();
        let second = capture_rx.recv().await.unwrap().into_record();
        assert_eq!(first.direction, Direction::ClientToServer);
        assert_eq!(second.direction, Direction::ServerToClient);
        assert_eq!(first.binary_payload().unwrap().unwrap(), payload);
        assert_eq!(second.binary_payload().unwrap().unwrap(), payload);
    }
}
