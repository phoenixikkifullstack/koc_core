use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use tokio::sync::OwnedSemaphorePermit;
use tokio_tungstenite::tungstenite::Message;

pub const CAPTURE_SCHEMA_VERSION: u8 = 1;
pub const MAX_CAPTURE_LINE_BYTES: usize = 48 * 1024 * 1024;
const MAX_CONNECTION_ID_BYTES: usize = 256;
const MAX_CATALOG_COMMANDS: usize = 4096;
const MAX_CATALOG_SHAPES: usize = 32;
const MAX_SHAPE_DEPTH: usize = 16;
const MAX_SHAPE_FIELDS: usize = 256;
const MAX_ARRAY_VARIANTS: usize = 16;
const MAX_ARRAY_ITEMS_INSPECTED: usize = 256;
const MAX_COMMAND_BYTES: usize = 256;
const MAX_SCHEMA_KEY_CHARS: usize = 256;
const MAX_RESPONSE_CODES: usize = 64;
const MAX_CATALOG_SHAPE_COUNT: usize = 4096;
const MAX_CATALOG_SCHEMA_BYTES: usize = 16 * 1024 * 1024;
const MAX_SINGLE_SHAPE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    ClientToServer,
    ServerToClient,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClientToServer => write!(f, "C->S"),
            Self::ServerToClient => write!(f, "S->C"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameOpcode {
    Binary,
    Text,
    Ping,
    Pong,
    Close,
    Frame,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub version: u8,
    pub connection_id: String,
    pub frame_index: u64,
    pub timestamp_ms: u64,
    pub direction: Direction,
    pub opcode: FrameOpcode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_base64: Option<String>,
}

#[derive(Debug)]
pub struct ObservedFrame {
    connection_id: String,
    frame_index: u64,
    timestamp_ms: u64,
    direction: Direction,
    message: Message,
    _queue_permit: Option<OwnedSemaphorePermit>,
}

impl ObservedFrame {
    pub fn from_message(
        connection_id: &str,
        frame_index: u64,
        direction: Direction,
        message: &Message,
    ) -> Self {
        Self {
            connection_id: connection_id.to_string(),
            frame_index,
            timestamp_ms: unix_time_ms(),
            direction,
            message: message.clone(),
            _queue_permit: None,
        }
    }

    pub fn from_owned_message(
        connection_id: &str,
        frame_index: u64,
        direction: Direction,
        message: Message,
        queue_permit: OwnedSemaphorePermit,
    ) -> Self {
        Self {
            connection_id: connection_id.to_string(),
            frame_index,
            timestamp_ms: unix_time_ms(),
            direction,
            message,
            _queue_permit: Some(queue_permit),
        }
    }

    pub fn into_record(self) -> CaptureRecord {
        let (opcode, payload) = message_payload(&self.message);
        CaptureRecord {
            version: CAPTURE_SCHEMA_VERSION,
            connection_id: self.connection_id,
            frame_index: self.frame_index,
            timestamp_ms: self.timestamp_ms,
            direction: self.direction,
            opcode,
            payload_base64: payload.map(|data| BASE64.encode(data)),
        }
    }
}

impl CaptureRecord {
    pub fn from_message(
        connection_id: &str,
        frame_index: u64,
        direction: Direction,
        message: &Message,
    ) -> Self {
        ObservedFrame::from_message(connection_id, frame_index, direction, message).into_record()
    }

    pub fn binary_payload(&self) -> Result<Option<Vec<u8>>, String> {
        if self.opcode != FrameOpcode::Binary {
            return Ok(None);
        }
        let encoded = self
            .payload_base64
            .as_deref()
            .ok_or_else(|| "binary capture record has no payload".to_string())?;
        BASE64
            .decode(encoded)
            .map(Some)
            .map_err(|e| format!("invalid capture payload base64: {}", e))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadEncoding {
    X,
    Lx,
    Plain,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecodedEvent {
    pub connection_id: String,
    pub frame_index: u64,
    pub timestamp_ms: u64,
    pub direction: Direction,
    pub opcode: FrameOpcode,
    pub wire_size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<PayloadEncoding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    pub seq: u32,
    pub ack: u32,
    pub resp: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_request: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decode_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_decode_error: Option<String>,
}

#[derive(Debug, Clone)]
struct PendingRequest {
    cmd: String,
    timestamp_ms: u64,
}

pub struct Inspector {
    pending: HashMap<(String, u32), PendingRequest>,
    redact_sensitive: bool,
}

impl Inspector {
    pub fn new(redact_sensitive: bool) -> Self {
        Self {
            pending: HashMap::new(),
            redact_sensitive,
        }
    }

    pub fn inspect(&mut self, record: &CaptureRecord) -> DecodedEvent {
        let mut event = DecodedEvent {
            connection_id: record.connection_id.clone(),
            frame_index: record.frame_index,
            timestamp_ms: record.timestamp_ms,
            direction: record.direction,
            opcode: record.opcode,
            wire_size: 0,
            encoding: None,
            cmd: None,
            seq: 0,
            ack: 0,
            resp: 0,
            code: None,
            body: None,
            matched_request: None,
            latency_ms: None,
            decode_error: None,
            body_decode_error: None,
        };

        let payload = match record.binary_payload() {
            Ok(Some(payload)) => payload,
            Ok(None) => return event,
            Err(error) => {
                event.decode_error = Some(error);
                return event;
            }
        };
        event.wire_size = payload.len();
        event.encoding = Some(detect_encoding(&payload));

        let proto = match crate::parse_message_limited(
            &payload,
            crate::protocol::DEFAULT_MAX_WIRE_SIZE,
            crate::crypto::DEFAULT_MAX_DECOMPRESSED_SIZE,
        ) {
            Ok(proto) => proto,
            Err(error) => {
                event.decode_error = Some(error);
                return event;
            }
        };

        event.cmd = match proto.cmd() {
            Some(cmd) if cmd.len() > MAX_COMMAND_BYTES => {
                event.decode_error = Some(format!(
                    "command exceeds limit: {} > {} bytes",
                    cmd.len(),
                    MAX_COMMAND_BYTES
                ));
                None
            }
            Some(cmd) => Some(cmd.to_string()),
            None => None,
        };
        event.seq = proto.seq();
        event.ack = proto.ack();
        event.resp = proto.resp();
        event.code = proto.code_opt();
        event.body = match proto.raw_data_result() {
            Ok(body) => body,
            Err(error) => {
                event.body_decode_error = Some(error);
                None
            }
        };

        let key = (record.connection_id.clone(), event.resp);
        if record.direction == Direction::ServerToClient && event.resp > 0 {
            if let Some(request) = self.pending.remove(&key) {
                event.latency_ms = Some(record.timestamp_ms.saturating_sub(request.timestamp_ms));
                event.matched_request = Some(request.cmd);
            }
        }

        if record.direction == Direction::ClientToServer && event.seq > 0 {
            if self.pending.len() >= 10_000 {
                let cutoff = record.timestamp_ms.saturating_sub(10 * 60 * 1000);
                self.pending
                    .retain(|_, request| request.timestamp_ms >= cutoff);
                if self.pending.len() >= 10_000 {
                    self.pending.clear();
                }
            }
            self.pending.insert(
                (record.connection_id.clone(), event.seq),
                PendingRequest {
                    cmd: event.cmd.clone().unwrap_or_else(|| "<unknown>".to_string()),
                    timestamp_ms: record.timestamp_ms,
                },
            );
        }

        if self.redact_sensitive {
            if let Some(body) = event.body.as_mut() {
                redact_value(body);
            }
        }
        event
    }
}

impl Default for Inspector {
    fn default() -> Self {
        Self::new(true)
    }
}

#[derive(Debug, Default, Serialize)]
pub struct CommandCatalog {
    pub version: u8,
    pub unmatched_responses: u64,
    pub overflow_commands: u64,
    pub overflow_shapes: u64,
    pub shape_count: usize,
    pub schema_bytes_used: usize,
    pub commands: BTreeMap<String, CommandSummary>,
}

#[derive(Debug, Default, Serialize)]
pub struct CommandSummary {
    pub requests: u64,
    pub responses: u64,
    pub pushes: u64,
    pub codes: BTreeMap<String, u64>,
    pub request_shapes: Vec<Value>,
    pub response_shapes: Vec<Value>,
    pub push_shapes: Vec<Value>,
    pub overflow_shapes: u64,
}

impl CommandCatalog {
    pub fn new() -> Self {
        Self {
            version: CAPTURE_SCHEMA_VERSION,
            unmatched_responses: 0,
            overflow_commands: 0,
            overflow_shapes: 0,
            shape_count: 0,
            schema_bytes_used: 0,
            commands: BTreeMap::new(),
        }
    }

    pub fn observe(&mut self, event: &DecodedEvent) {
        if event.decode_error.is_some() || event.opcode != FrameOpcode::Binary {
            return;
        }
        match event.direction {
            Direction::ClientToServer => {
                if event.seq == 0 {
                    return;
                }
                let Some(cmd) = event.cmd.as_deref() else {
                    return;
                };
                if !self.ensure_command(cmd) {
                    return;
                }
                self.commands.get_mut(cmd).unwrap().requests += 1;
                self.record_shape(cmd, ShapeKind::Request, event.body.as_ref());
            }
            Direction::ServerToClient => {
                if event.resp > 0 {
                    let Some(cmd) = event.matched_request.as_deref() else {
                        self.unmatched_responses += 1;
                        return;
                    };
                    if !self.ensure_command(cmd) {
                        return;
                    }
                    {
                        let summary = self.commands.get_mut(cmd).unwrap();
                        summary.responses += 1;
                        if let Some(code) = event.code {
                            add_response_code(summary, code);
                        }
                    }
                    self.record_shape(cmd, ShapeKind::Response, event.body.as_ref());
                } else if let Some(cmd) = event.cmd.as_deref() {
                    if !self.ensure_command(cmd) {
                        return;
                    }
                    {
                        let summary = self.commands.get_mut(cmd).unwrap();
                        summary.pushes += 1;
                        if let Some(code) = event.code {
                            add_response_code(summary, code);
                        }
                    }
                    self.record_shape(cmd, ShapeKind::Push, event.body.as_ref());
                }
            }
        }
    }

    fn ensure_command(&mut self, cmd: &str) -> bool {
        if !self.commands.contains_key(cmd) && self.commands.len() >= MAX_CATALOG_COMMANDS {
            self.overflow_commands += 1;
            return false;
        }
        self.commands.entry(cmd.to_string()).or_default();
        true
    }

    fn record_shape(&mut self, cmd: &str, kind: ShapeKind, body: Option<&Value>) {
        let Some(body) = body else { return };
        let shape = value_shape(body, 0);
        let summary = self.commands.get(cmd).expect("command initialized");
        let shapes = kind.shapes(summary);
        if shapes.contains(&shape) {
            return;
        }
        let shape_bytes = serde_json::to_vec(&shape)
            .map(|value| value.len())
            .unwrap_or(usize::MAX);
        let over_limit = shapes.len() >= MAX_CATALOG_SHAPES
            || shape_bytes > MAX_SINGLE_SHAPE_BYTES
            || self.shape_count >= MAX_CATALOG_SHAPE_COUNT
            || self.schema_bytes_used.saturating_add(shape_bytes) > MAX_CATALOG_SCHEMA_BYTES;
        if over_limit {
            self.overflow_shapes += 1;
            self.commands.get_mut(cmd).unwrap().overflow_shapes += 1;
            return;
        }
        self.shape_count += 1;
        self.schema_bytes_used += shape_bytes;
        kind.shapes_mut(self.commands.get_mut(cmd).unwrap())
            .push(shape);
    }
}

#[derive(Debug, Clone, Copy)]
enum ShapeKind {
    Request,
    Response,
    Push,
}

impl ShapeKind {
    fn shapes(self, summary: &CommandSummary) -> &Vec<Value> {
        match self {
            Self::Request => &summary.request_shapes,
            Self::Response => &summary.response_shapes,
            Self::Push => &summary.push_shapes,
        }
    }

    fn shapes_mut(self, summary: &mut CommandSummary) -> &mut Vec<Value> {
        match self {
            Self::Request => &mut summary.request_shapes,
            Self::Response => &mut summary.response_shapes,
            Self::Push => &mut summary.push_shapes,
        }
    }
}

pub fn read_records<R, F>(reader: R, mut callback: F) -> Result<(), String>
where
    R: BufRead,
    F: FnMut(CaptureRecord) -> Result<(), String>,
{
    let mut reader = reader;
    let mut line_number = 0usize;
    loop {
        let Some(line) = read_bounded_line(&mut reader, MAX_CAPTURE_LINE_BYTES)? else {
            break;
        };
        line_number += 1;
        let line = String::from_utf8(line)
            .map_err(|e| format!("capture line {} is not UTF-8: {}", line_number, e))?;
        if line.trim().is_empty() {
            continue;
        }
        let record: CaptureRecord = serde_json::from_str(&line)
            .map_err(|e| format!("invalid capture record on line {}: {}", line_number, e))?;
        if record.version != CAPTURE_SCHEMA_VERSION {
            return Err(format!(
                "unsupported capture schema version {} on line {}",
                record.version, line_number
            ));
        }
        if record.connection_id.len() > MAX_CONNECTION_ID_BYTES {
            return Err(format!("connection_id is too long on line {}", line_number));
        }
        callback(record)?;
    }
    Ok(())
}

pub fn open_secure_writer(path: &Path) -> Result<BufWriter<File>, String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {}", parent.display(), e))?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!("refusing to overwrite symlink: {}", path.display()));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!("output path is not a file: {}", path.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("failed to inspect {}: {}", path.display(), error)),
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("failed to open {}: {}", path.display(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("failed to secure {}: {}", path.display(), e))?;
    }
    Ok(BufWriter::new(file))
}

pub fn write_json_line<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, value).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())
}

pub fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut writer = open_secure_writer(path)?;
    rewrite_json_pretty(&mut writer, value)
}

pub fn rewrite_json_pretty<T: Serialize>(
    writer: &mut BufWriter<File>,
    value: &T,
) -> Result<(), String> {
    writer.flush().map_err(|e| e.to_string())?;
    writer.get_ref().set_len(0).map_err(|e| e.to_string())?;
    writer.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut *writer, value).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())
}

pub fn format_pretty_event(event: &DecodedEvent) -> String {
    if let Some(error) = &event.decode_error {
        return format!(
            "{} {} #{} DECODE_ERROR size={} {}",
            format_timestamp(event.timestamp_ms),
            event.direction,
            event.frame_index,
            event.wire_size,
            error
        );
    }
    if event.opcode != FrameOpcode::Binary {
        return format!(
            "{} {} #{} {:?}",
            format_timestamp(event.timestamp_ms),
            event.direction,
            event.frame_index,
            event.opcode
        );
    }

    let command = event
        .matched_request
        .as_deref()
        .or(event.cmd.as_deref())
        .unwrap_or("<unknown>");
    let relation = if event.resp > 0 {
        format!(
            "resp={} latency={}ms",
            event.resp,
            event.latency_ms.unwrap_or(0)
        )
    } else if event.seq > 0 {
        format!("seq={}", event.seq)
    } else {
        "unsequenced".to_string()
    };
    let mut output = format!(
        "{} {} #{} {} cmd={} code={}",
        format_timestamp(event.timestamp_ms),
        event.direction,
        event.frame_index,
        relation,
        command,
        event
            .code
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    if let Some(body) = &event.body {
        let formatted = serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string());
        output.push('\n');
        output.push_str(&formatted);
    }
    if let Some(error) = &event.body_decode_error {
        output.push_str("\nBODY_DECODE_ERROR: ");
        output.push_str(error);
    }
    output
}

pub fn matches_command(pattern: Option<&str>, event: &DecodedEvent) -> bool {
    let Some(pattern) = pattern else { return true };
    let command = event
        .matched_request
        .as_deref()
        .or(event.cmd.as_deref())
        .unwrap_or("");
    wildcard_match(pattern, command)
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut pattern_index, mut value_index) = (0usize, 0usize);
    let (mut star_index, mut star_value_index) = (None, 0usize);

    while value_index < value.len() {
        if pattern_index < pattern.len() && pattern[pattern_index] == value[value_index] {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn detect_encoding(payload: &[u8]) -> PayloadEncoding {
    match payload.get(..2) {
        Some([112, 120]) => PayloadEncoding::X,
        Some([112, 108]) => PayloadEncoding::Lx,
        _ => PayloadEncoding::Plain,
    }
}

fn redact_value(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let normalized = key.to_ascii_lowercase().replace(['_', '-'], "");
                if is_sensitive_key(&normalized) {
                    *value = Value::String("[REDACTED]".to_string());
                } else {
                    redact_value(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_value(value);
            }
        }
        _ => {}
    }
}

fn is_sensitive_key(key: &str) -> bool {
    matches!(
        key,
        "token"
            | "accesstoken"
            | "refreshtoken"
            | "session"
            | "sessionid"
            | "sessid"
            | "connid"
            | "connectionid"
            | "openid"
            | "unionid"
            | "roleid"
            | "userid"
            | "accountid"
            | "deviceid"
            | "uid"
            | "name"
            | "nickname"
            | "sessionkey"
            | "sessiontoken"
            | "authtoken"
            | "credential"
            | "authorization"
            | "password"
            | "secret"
    )
}

fn value_shape(value: &Value, depth: usize) -> Value {
    if depth >= MAX_SHAPE_DEPTH {
        return Value::String("depth_limit".to_string());
    }
    match value {
        Value::Null => Value::String("null".to_string()),
        Value::Bool(_) => Value::String("boolean".to_string()),
        Value::Number(_) => Value::String("number".to_string()),
        Value::String(_) => Value::String("string".to_string()),
        Value::Array(values) => {
            let mut variants = Vec::new();
            for value in values.iter().take(MAX_ARRAY_ITEMS_INSPECTED) {
                let shape = value_shape(value, depth + 1);
                if !variants.contains(&shape) {
                    if variants.len() >= MAX_ARRAY_VARIANTS {
                        variants.push(Value::String("variant_limit".to_string()));
                        break;
                    }
                    variants.push(shape);
                }
            }
            variants.sort_by_key(Value::to_string);
            Value::Array(variants)
        }
        Value::Object(object) => {
            let mut shape = serde_json::Map::new();
            for (key, value) in object.iter().take(MAX_SHAPE_FIELDS) {
                let key = if key.len() > MAX_SCHEMA_KEY_CHARS {
                    format!(
                        "{}...",
                        key.chars().take(MAX_SCHEMA_KEY_CHARS).collect::<String>()
                    )
                } else {
                    key.clone()
                };
                shape.insert(key, value_shape(value, depth + 1));
            }
            if object.len() > MAX_SHAPE_FIELDS {
                shape.insert("$truncated".to_string(), Value::Bool(true));
            }
            Value::Object(shape)
        }
    }
}

fn add_response_code(summary: &mut CommandSummary, code: i32) {
    let code = code.to_string();
    if summary.codes.contains_key(&code) || summary.codes.len() < MAX_RESPONSE_CODES {
        *summary.codes.entry(code).or_default() += 1;
    }
}

fn message_payload(message: &Message) -> (FrameOpcode, Option<&[u8]>) {
    match message {
        Message::Binary(data) => (FrameOpcode::Binary, Some(data.as_ref())),
        Message::Text(data) => (FrameOpcode::Text, Some(data.as_bytes())),
        Message::Ping(data) => (FrameOpcode::Ping, Some(data.as_ref())),
        Message::Pong(data) => (FrameOpcode::Pong, Some(data.as_ref())),
        Message::Close(_) => (FrameOpcode::Close, None),
        Message::Frame(_) => (FrameOpcode::Frame, None),
    }
}

fn read_bounded_line(
    reader: &mut impl BufRead,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    let mut line = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .map_err(|e| format!("failed to read capture stream: {}", e))?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(line))
            };
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|position| position + 1)
            .unwrap_or(available.len());
        if line.len().saturating_add(take) > max_bytes {
            return Err(format!("capture line exceeds {} bytes", max_bytes));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if line.last() == Some(&b'\n') {
            return Ok(Some(line));
        }
    }
}

fn format_timestamp(timestamp_ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(timestamp_ms as i64)
        .map(|time| time.format("%H:%M:%S%.3f").to_string())
        .unwrap_or_else(|| timestamp_ms.to_string())
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Encryptor, XorCrypto, bon};
    use serde_json::json;

    fn binary_record(direction: Direction, index: u64, payload: &[u8]) -> CaptureRecord {
        CaptureRecord {
            version: CAPTURE_SCHEMA_VERSION,
            connection_id: "test-1".to_string(),
            frame_index: index,
            timestamp_ms: 1_000 + index * 10,
            direction,
            opcode: FrameOpcode::Binary,
            payload_base64: Some(BASE64.encode(payload)),
        }
    }

    #[test]
    fn correlates_response_with_request() {
        let request = bon::encode_game_packet("activity_test", 0, 7, 1000, Some(&json!({"id": 3})));
        let request = XorCrypto::new().encrypt(&request);
        let response = crate::create_message(
            &serde_json::from_value(json!({
                "resp": 7,
                "seq": 9,
                "code": 0,
                "body": {"ok": true}
            }))
            .unwrap(),
            Some("x"),
        )
        .unwrap();

        let mut inspector = Inspector::default();
        let request_event =
            inspector.inspect(&binary_record(Direction::ClientToServer, 1, &request));
        assert_eq!(request_event.cmd.as_deref(), Some("activity_test"));

        let response_event =
            inspector.inspect(&binary_record(Direction::ServerToClient, 2, &response));
        assert_eq!(
            response_event.matched_request.as_deref(),
            Some("activity_test")
        );
        assert_eq!(response_event.latency_ms, Some(10));
        assert_eq!(response_event.code, Some(0));
    }

    #[test]
    fn command_filter_supports_wildcards() {
        assert!(wildcard_match("activity_*", "activity_test"));
        assert!(wildcard_match("*test", "activity_test"));
        assert!(wildcard_match("act*test", "activity_test"));
        assert!(wildcard_match("*activity", "activity_activity"));
        assert!(wildcard_match("a*a", "aaa"));
        assert!(!wildcard_match("fight_*", "activity_test"));
    }

    #[test]
    fn redacts_sensitive_values() {
        let mut value = json!({"token": "secret", "nested": {"sessId": 1, "id": 2}});
        redact_value(&mut value);
        assert_eq!(value["token"], "[REDACTED]");
        assert_eq!(value["nested"]["sessId"], "[REDACTED]");
        assert_eq!(value["nested"]["id"], 2);
    }

    #[test]
    fn malformed_nested_body_is_reported_without_echoing_base64() {
        let payload = crate::create_message(
            &serde_json::from_value(json!({
                "cmd": "activity_bad",
                "seq": 1,
                "body": "not-valid-base64"
            }))
            .unwrap(),
            Some("x"),
        )
        .unwrap();
        let mut inspector = Inspector::default();
        let event = inspector.inspect(&binary_record(Direction::ClientToServer, 1, &payload));
        assert!(
            event
                .body_decode_error
                .as_deref()
                .unwrap()
                .contains("body base64")
        );
        assert!(event.decode_error.is_none());
        assert!(event.body.is_none());
        let mut catalog = CommandCatalog::new();
        catalog.observe(&event);
        assert_eq!(catalog.commands["activity_bad"].requests, 1);
    }

    #[test]
    fn catalog_ignores_heartbeat_and_tracks_unmatched_response() {
        let heartbeat = bon::encode_game_packet("_sys/ack", 4, 0, 1000, None);
        let heartbeat = XorCrypto::new().encrypt(&heartbeat);
        let unmatched = crate::create_message(
            &serde_json::from_value(json!({
                "cmd": "activity_test",
                "resp": 99,
                "seq": 10,
                "code": 0,
                "body": {"ok": true}
            }))
            .unwrap(),
            Some("x"),
        )
        .unwrap();

        let mut inspector = Inspector::default();
        let mut catalog = CommandCatalog::new();
        catalog.observe(&inspector.inspect(&binary_record(
            Direction::ClientToServer,
            1,
            &heartbeat,
        )));
        catalog.observe(&inspector.inspect(&binary_record(
            Direction::ServerToClient,
            2,
            &unmatched,
        )));
        assert!(catalog.commands.is_empty());
        assert_eq!(catalog.unmatched_responses, 1);
    }

    #[test]
    fn bounded_line_reader_rejects_oversized_input() {
        let mut input = std::io::Cursor::new(b"12345\n".to_vec());
        assert!(read_bounded_line(&mut input, 4).is_err());
    }

    #[test]
    fn jsonl_roundtrip_uses_the_same_inspector_path() {
        let packet = bon::encode_game_packet("activity_roundtrip", 0, 3, 1000, Some(&json!({})));
        let packet = XorCrypto::new().encrypt(&packet);
        let record = binary_record(Direction::ClientToServer, 1, &packet);
        let mut jsonl = Vec::new();
        write_json_line(&mut jsonl, &record).unwrap();

        let mut decoded = Vec::new();
        let mut inspector = Inspector::default();
        read_records(std::io::Cursor::new(jsonl), |record| {
            decoded.push(inspector.inspect(&record));
            Ok(())
        })
        .unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].cmd.as_deref(), Some("activity_roundtrip"));
    }

    #[cfg(unix)]
    #[test]
    fn secure_writer_uses_private_permissions_and_overwrites_regular_file() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join(format!(
            "koc-proxy-test-{}-{}.jsonl",
            std::process::id(),
            unix_time_ms()
        ));
        let mut writer = open_secure_writer(&path).unwrap();
        writer.write_all(b"old").unwrap();
        writer.flush().unwrap();
        drop(writer);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let mut writer = open_secure_writer(&path).unwrap();
        writer.write_all(b"new").unwrap();
        writer.flush().unwrap();
        drop(writer);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn catalog_snapshot_is_visible_before_writer_is_dropped() {
        let path = std::env::temp_dir().join(format!(
            "koc-proxy-catalog-test-{}-{}.json",
            std::process::id(),
            unix_time_ms()
        ));
        let mut writer = open_secure_writer(&path).unwrap();
        let catalog = CommandCatalog::new();
        rewrite_json_pretty(&mut writer, &catalog).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"commands\""));
        drop(writer);
        std::fs::remove_file(path).unwrap();
    }
}
