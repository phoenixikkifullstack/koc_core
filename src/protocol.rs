use crate::bon;
use serde_json::Value;

#[derive(Debug)]
pub struct ProtoMsg {
    raw: serde_json::Map<String, Value>,
}

impl ProtoMsg {
    pub fn new(raw: serde_json::Map<String, Value>) -> Self {
        Self { raw }
    }
    pub fn cmd(&self) -> Option<&str> {
        self.raw.get("cmd").and_then(|v| v.as_str())
    }
    pub fn ack(&self) -> u32 {
        self.raw.get("ack").and_then(|v| v.as_u64()).unwrap_or(0) as u32
    }
    pub fn seq(&self) -> u32 {
        self.raw.get("seq").and_then(|v| v.as_u64()).unwrap_or(0) as u32
    }
    pub fn time(&self) -> i64 {
        self.raw.get("time").and_then(|v| v.as_i64()).unwrap_or(0)
    }
    pub fn code(&self) -> i32 {
        self.raw.get("code").and_then(|v| v.as_i64()).unwrap_or(0) as i32
    }
    pub fn error(&self) -> Option<&str> {
        self.raw.get("error").and_then(|v| v.as_str())
    }
    pub fn body(&self) -> Option<&Value> {
        self.raw.get("body")
    }
    pub fn resp(&self) -> u32 {
        self.raw.get("resp").and_then(|v| v.as_u64()).unwrap_or(0) as u32
    }
    pub fn hint(&self) -> Option<&str> {
        self.raw.get("hint").and_then(|v| v.as_str())
    }
    pub fn get_data(&self) -> &serde_json::Map<String, Value> {
        if let Some(body) = self.body() {
            if let Some(obj) = body.as_object() {
                return obj;
            }
        }
        &self.raw
    }

    /// Decode the body field (BON binary) into a serde_json::Map.
    /// In BON protocol, body is a binary blob (tag 7) that contains
    /// another BON-encoded object. We stored it as base64 string during
    /// initial decode, so we need to base64-decode then BON-decode it.
    pub fn raw_data(&self) -> Option<Value> {
        let body = self.body()?;
        match body {
            Value::String(s) => {
                // body was encoded as base64 by our BON decoder (tag 7)
                let bytes = base64_decode(s)?;
                bon::decode(&bytes)
            }
            Value::Object(_) => Some(body.clone()),
            _ => None,
        }
    }
    pub fn into_data(self) -> serde_json::Map<String, Value> {
        self.raw
    }
}

pub fn parse_message(data: &[u8]) -> Result<ProtoMsg, String> {
    let decrypted = crate::crypto::auto_decrypt(data);
    let value = bon::decode(&decrypted).ok_or_else(|| "BON decode failed".to_string())?;
    match value {
        Value::Object(obj) => Ok(ProtoMsg::new(obj)),
        _ => Err("Expected object".to_string()),
    }
}

#[allow(dead_code)]
pub fn parse_message_enc(data: &[u8], enc_name: &str) -> Result<ProtoMsg, String> {
    let enc = crate::crypto::get_encryptor(enc_name);
    let decrypted = enc.decrypt(data);
    let value = bon::decode(&decrypted).ok_or_else(|| "BON decode failed".to_string())?;
    match value {
        Value::Object(obj) => Ok(ProtoMsg::new(obj)),
        _ => Err("Expected object".to_string()),
    }
}

pub fn create_message(
    data: &serde_json::Map<String, Value>,
    enc_name: Option<&str>,
) -> Result<Vec<u8>, String> {
    let enc = crate::crypto::get_encryptor(enc_name.unwrap_or("x"));
    let encoded = bon::encode(&Value::Object(data.clone()));
    Ok(enc.encrypt(&encoded))
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let table = |c: u8| -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let chunks = bytes.chunks(4);
    for chunk in chunks {
        let a = table(*chunk.first()?)?;
        let b = table(*chunk.get(1)?)?;
        out.push((a << 2) | (b >> 4));
        if let Some(&c) = chunk.get(2) {
            let c = table(c)?;
            out.push((b << 4) | (c >> 2));
            if let Some(&d) = chunk.get(3) {
                let d = table(d)?;
                out.push((c << 6) | d);
            }
        }
    }
    Some(out)
}
