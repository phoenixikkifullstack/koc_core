use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock, oneshot};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use crate::{bon, protocol};

/// Callback channel for matching request seq -> response
type ResponseSender = oneshot::Sender<Result<Value, String>>;

pub struct WebSocketClient {
    sender: mpsc::Sender<Vec<u8>>,
    seq: Arc<RwLock<u32>>,
    ack: Arc<RwLock<u32>>,
    pending: Arc<RwLock<HashMap<u32, ResponseSender>>>,
    /// Channel to receive all incoming messages (for the caller to process)
    pub messages: mpsc::Receiver<Value>,
    heartbeat_handle: Option<tokio::task::JoinHandle<()>>,
}

impl WebSocketClient {
    /// Build the game WebSocket URL from a token JSON string
    pub fn build_url(token_json: &str) -> String {
        Self::build_url_with_base(token_json, "wss://xxz-xyzw.hortorgames.com")
            .expect("default WebSocket base URL is valid")
    }

    /// Build a game WebSocket URL using an alternate origin, such as a local relay.
    pub fn build_url_with_base(token_json: &str, base_url: &str) -> Result<String, String> {
        let mut url = url::Url::parse(base_url)
            .map_err(|e| format!("invalid WebSocket base URL: {}", e))?;
        if !matches!(url.scheme(), "ws" | "wss") {
            return Err("WebSocket base URL must use ws:// or wss://".to_string());
        }
        if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
            return Err("WebSocket base URL must be an origin without credentials".to_string());
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err("WebSocket base URL must not contain a query or fragment".to_string());
        }
        url.set_path("/agent");
        url.query_pairs_mut()
            .append_pair("p", token_json)
            .append_pair("e", "x")
            .append_pair("lang", "chinese");
        Ok(url.into())
    }

    /// Connect to the game WebSocket server
    pub async fn connect(url: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (ws_stream, _) = connect_async(url).await?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        // Channel: caller -> ws write task
        let (write_tx, mut write_rx) = mpsc::channel::<Vec<u8>>(100);
        // Channel: ws read task -> caller (all messages)
        let (msg_tx, msg_rx) = mpsc::channel::<Value>(200);

        let seq = Arc::new(RwLock::new(1u32));
        let ack = Arc::new(RwLock::new(0u32));
        let pending: Arc<RwLock<HashMap<u32, ResponseSender>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Write task: send binary frames
        tokio::spawn(async move {
            while let Some(data) = write_rx.recv().await {
                if data.is_empty() { break; }
                if ws_write.send(Message::Binary(data.into())).await.is_err() {
                    break;
                }
            }
        });

        // Read task: receive & dispatch
        let ack_clone = ack.clone();
        let pending_clone = pending.clone();
        let msg_tx_clone = msg_tx.clone();
        tokio::spawn(async move {
            while let Some(Ok(frame)) = ws_read.next().await {
                let data = match frame {
                    Message::Binary(b) => b.to_vec(),
                    _ => continue,
                };
                let proto = match protocol::parse_message(&data) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Update ack (track server's latest seq)
                let server_seq = proto.seq();
                if server_seq > 0 {
                    let mut a = ack_clone.write().await;
                    *a = server_seq;
                }

                // Decode body for the response value
                let resp_value = proto.raw_data()
                    .unwrap_or_else(|| Value::Object(proto.get_data().clone()));

                // Check if this is a response to a pending request (match by resp field)
                let resp_seq = proto.resp();
                if resp_seq > 0 {
                    let mut p = pending_clone.write().await;
                    if let Some(tx) = p.remove(&resp_seq) {
                        let code = proto.code();
                        if code == 0 {
                            let _ = tx.send(Ok(resp_value.clone()));
                        } else {
                            let hint = proto.hint();
                            let err_msg = crate::error_codes::format_error(code, hint);
                            let _ = tx.send(Err(err_msg));
                        }
                    }
                }

                // Forward all messages to the caller
                let mut full_msg = serde_json::Map::new();
                full_msg.insert("cmd".to_string(), Value::String(
                    proto.cmd().unwrap_or("").to_string()
                ));
                full_msg.insert("code".to_string(), Value::Number(proto.code().into()));
                full_msg.insert("data".to_string(), resp_value);
                let _ = msg_tx_clone.send(Value::Object(full_msg)).await;
            }
        });

        // Start heartbeat (first after 3s, then every 5s)
        let hb_tx = write_tx.clone();
        let hb_ack = ack.clone();
        let hb_handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
            loop {
                let ack_val = { *hb_ack.read().await };
                let pkt = Self::build_packet("_sys/ack", None, ack_val, 0);
                if hb_tx.send(pkt).await.is_err() { break; }
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        });

        Ok(Self {
            sender: write_tx,
            seq,
            ack,
            pending,
            messages: msg_rx,
            heartbeat_handle: Some(hb_handle),
        })
    }

    /// Send a command (fire-and-forget)
    pub async fn send(&self, cmd: &str, params: Option<Value>) -> Result<u32, String> {
        let assigned_seq = {
            let mut s = self.seq.write().await;
            let v = *s; *s += 1; v
        };
        let ack_val = { *self.ack.read().await };
        let pkt = Self::build_packet(cmd, params.as_ref(), ack_val, assigned_seq);
        self.sender.send(pkt).await.map_err(|e| e.to_string())?;
        Ok(assigned_seq)
    }

    /// Send a command and wait for the matching response
    pub async fn send_with_response(&self, cmd: &str, params: Option<Value>, timeout_ms: u64) -> Result<Value, String> {
        let assigned_seq = {
            let mut s = self.seq.write().await;
            let v = *s; *s += 1; v
        };
        let ack_val = { *self.ack.read().await };

        let (resp_tx, resp_rx) = oneshot::channel();
        {
            let mut p = self.pending.write().await;
            p.insert(assigned_seq, resp_tx);
        }

        let pkt = Self::build_packet(cmd, params.as_ref(), ack_val, assigned_seq);
        self.sender.send(pkt).await.map_err(|e| e.to_string())?;

        tokio::select! {
            result = resp_rx => {
                match result {
                    Ok(Ok(val)) => Ok(val),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err("Response channel closed".to_string()),
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(timeout_ms)) => {
                let mut p = self.pending.write().await;
                p.remove(&assigned_seq);
                Err(format!("Timeout after {}ms for cmd={}", timeout_ms, cmd))
            }
        }
    }

    /// Build a wire-format packet: BON encode {cmd, ack, seq, time, body} then encrypt
    fn build_packet(cmd: &str, params: Option<&Value>, ack: u32, seq: u32) -> Vec<u8> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        // Use the specialized encoder that handles body as binary (tag 7)
        let pkt_bytes = bon::encode_game_packet(cmd, ack, seq, now, params);

        // Encrypt with "x" scheme
        let enc = crate::crypto::get_encryptor("x");
        enc.encrypt(&pkt_bytes)
    }

    pub async fn disconnect(&mut self) {
        if let Some(h) = self.heartbeat_handle.take() {
            h.abort();
        }
        let _ = self.sender.send(Vec::new()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_relay_url_without_losing_token_json() {
        let token = r#"{"sessId":1,"name":"a b"}"#;
        let url = WebSocketClient::build_url_with_base(token, "ws://127.0.0.1:8787").unwrap();
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.scheme(), "ws");
        assert_eq!(parsed.path(), "/agent");
        let query: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(query.get("p").map(String::as_str), Some(token));
        assert_eq!(query.get("e").map(String::as_str), Some("x"));
    }
}
