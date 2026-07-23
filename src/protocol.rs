use crate::bon;
use serde_json::Value;

pub const DEFAULT_MAX_WIRE_SIZE: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct ProtoMsg {
    raw: serde_json::Map<String, Value>,
    body_decode_limits: bon::BonDecodeLimits,
}

impl ProtoMsg {
    pub fn new(raw: serde_json::Map<String, Value>) -> Self {
        Self::with_limits(raw, bon::BonDecodeLimits::default())
    }
    fn with_limits(
        raw: serde_json::Map<String, Value>,
        body_decode_limits: bon::BonDecodeLimits,
    ) -> Self {
        Self {
            raw,
            body_decode_limits,
        }
    }
    pub fn cmd(&self) -> Option<&str> {
        self.raw.get("cmd").and_then(|v| v.as_str())
    }
    pub fn ack(&self) -> u32 {
        self.raw
            .get("ack")
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0)
    }
    pub fn seq(&self) -> u32 {
        self.raw
            .get("seq")
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0)
    }
    pub fn time(&self) -> i64 {
        self.raw.get("time").and_then(|v| v.as_i64()).unwrap_or(0)
    }
    pub fn code(&self) -> i32 {
        self.code_opt().unwrap_or(0)
    }
    pub fn code_opt(&self) -> Option<i32> {
        self.raw
            .get("code")
            .and_then(|v| v.as_i64())
            .and_then(|v| i32::try_from(v).ok())
    }
    pub fn error(&self) -> Option<&str> {
        self.raw.get("error").and_then(|v| v.as_str())
    }
    pub fn body(&self) -> Option<&Value> {
        self.raw.get("body")
    }
    pub fn resp(&self) -> u32 {
        self.raw
            .get("resp")
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0)
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
        self.raw_data_result().ok().flatten()
    }

    pub fn raw_data_result(&self) -> Result<Option<Value>, String> {
        let Some(body) = self.body() else {
            return Ok(None);
        };
        match body {
            Value::String(s) => {
                // body was encoded as base64 by our BON decoder (tag 7)
                let bytes = base64_decode(s)
                    .ok_or_else(|| "body base64 decode failed".to_string())?;
                bon::decode_with_limits(&bytes, self.body_decode_limits)
                    .map(Some)
                    .ok_or_else(|| "body BON decode failed".to_string())
            }
            Value::Object(_) => Ok(Some(body.clone())),
            Value::Null => Ok(None),
            _ => Err("body is neither BON binary nor an object".to_string()),
        }
    }
    pub fn into_data(self) -> serde_json::Map<String, Value> {
        self.raw
    }
}

pub fn parse_message(data: &[u8]) -> Result<ProtoMsg, String> {
    parse_message_with_limits(
        data,
        32 * 1024 * 1024,
        crate::crypto::DEFAULT_MAX_DECOMPRESSED_SIZE,
        bon::BonDecodeLimits::default(),
    )
}

pub fn parse_message_limited(
    data: &[u8],
    max_wire_size: usize,
    max_decompressed_size: usize,
) -> Result<ProtoMsg, String> {
    parse_message_with_limits(
        data,
        max_wire_size,
        max_decompressed_size,
        bon::BonDecodeLimits::proxy(),
    )
}

fn parse_message_with_limits(
    data: &[u8],
    max_wire_size: usize,
    max_decompressed_size: usize,
    bon_limits: bon::BonDecodeLimits,
) -> Result<ProtoMsg, String> {
    if data.len() > max_wire_size {
        return Err(format!(
            "wire payload exceeds limit: {} > {} bytes",
            data.len(), max_wire_size
        ));
    }
    let decrypted = crate::crypto::try_auto_decrypt(data, max_decompressed_size)?;
    let value = bon::decode_with_limits(&decrypted, bon_limits)
        .ok_or_else(|| "BON decode failed".to_string())?;
    match value {
        Value::Object(obj) => Ok(ProtoMsg::with_limits(obj, bon_limits)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Encryptor, XorCrypto};

    #[test]
    fn rejects_wire_payload_over_configured_limit() {
        let error = parse_message_limited(&[0; 9], 8, 1024).unwrap_err();
        assert!(error.contains("wire payload exceeds limit"));
    }

    #[test]
    fn numeric_accessors_do_not_wrap_out_of_range_values() {
        let message = ProtoMsg::new(
            serde_json::from_value(serde_json::json!({
                "ack": u64::MAX,
                "seq": u64::MAX,
                "resp": u64::MAX,
                "code": i64::MAX
            }))
            .unwrap(),
        );
        assert_eq!(message.ack(), 0);
        assert_eq!(message.seq(), 0);
        assert_eq!(message.resp(), 0);
        assert_eq!(message.code_opt(), None);
    }

    #[test]
    fn normal_client_accepts_body_larger_than_proxy_budget() {
        let body = Value::Array(vec![Value::Null; 100_001]);
        let packet = bon::encode_game_packet("large_body", 0, 1, 1000, Some(&body));
        let packet = XorCrypto::new().encrypt(&packet);

        let normal = parse_message(&packet).unwrap();
        assert!(normal.raw_data_result().is_ok());

        let strict = parse_message_limited(
            &packet,
            DEFAULT_MAX_WIRE_SIZE,
            crate::crypto::DEFAULT_MAX_DECOMPRESSED_SIZE,
        )
        .unwrap();
        assert!(strict.raw_data_result().is_err());
    }
}
