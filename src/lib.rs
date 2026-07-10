use serde_json::Value;

pub mod bon;
pub mod crypto;
mod protocol;
mod http_client;
mod websocket;
pub mod kpi;
pub mod error_codes;
pub mod config;
pub mod state;
pub mod scheduler;
pub mod hortor_crypto;
pub mod wx_login;
pub mod logging;
pub mod study;

pub use bon::{DataReader, DataWriter, BonEncoder, BonDecoder};
pub use crypto::{XorCrypto, Lz4Crypto, get_encryptor, Encryptor, auto_decrypt};
pub use protocol::{ProtoMsg, parse_message, create_message};
pub use http_client::HttpClient;
pub use websocket::WebSocketClient;
pub use kpi::GameClient;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoleInfo {
    pub role_id: u64,
    pub name: String,
    pub server_id: u64,
    pub power: u64,
    pub level: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenData {
    pub id: String,
    pub name: String,
    pub token: String,
    pub server: String,
    pub role_id: u64,
    pub ws_url: Option<String>,
    pub import_method: String,
}

pub struct KocCore {
    http_client: HttpClient,
}

impl KocCore {
    pub fn new() -> Self {
        Self { http_client: HttpClient::new() }
    }

    pub fn get_token_id(bin_data: &[u8]) -> String {
        use md5::{Md5, Digest};
        let mut hasher = Md5::new();
        hasher.update(bin_data);
        let result = hasher.finalize();
        result.iter().map(|b| format!("{:02x}", b)).collect()
    }

    pub async fn transform_token(&self, bin_data: &[u8]) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let response = self.http_client.post_authuser(bin_data).await?;
        let msg = protocol::parse_message(&response)?;

        // body is BON-encoded binary, decode it
        let body_data = msg.raw_data().ok_or("Failed to decode body")?;
        let mut data = match body_data {
            Value::Object(obj) => obj,
            _ => msg.get_data().clone(),
        };

        let current_time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as u64;
        let sess_id = current_time * 100 + (rand_u64() % 100);
        let conn_id = current_time + (rand_u64() % 10);

        data.insert("sessId".to_string(), Value::Number(sess_id.into()));
        data.insert("connId".to_string(), Value::Number(conn_id.into()));
        data.insert("isRestore".to_string(), Value::Number(0.into()));

        Ok(serde_json::to_string(&data)?)
    }

    pub async fn get_server_list(&self, bin_data: &[u8]) -> Result<Vec<RoleInfo>, Box<dyn std::error::Error + Send + Sync>> {
        let response = self.http_client.post_serverlist(bin_data).await?;
        let msg = protocol::parse_message(&response)?;

        // body is BON-encoded binary, decode it
        let body_data = msg.raw_data().ok_or("Failed to decode body")?;
        let data = body_data.as_object().ok_or("Body is not an object")?;

        let mut roles = Vec::new();
        if let Some(roles_map) = data.get("roles") {
            if let Some(obj) = roles_map.as_object() {
                for (_, v) in obj {
                    if let Some(role_obj) = v.as_object() {
                        let role_id = role_obj.get("roleId").and_then(|v| v.as_u64()).unwrap_or(0);
                        let name = role_obj.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                        let server_id = role_obj.get("serverId").and_then(|v| v.as_u64()).unwrap_or(0);
                        let power = role_obj.get("power").and_then(|v| v.as_u64()).unwrap_or(0);
                        let level = role_obj.get("level").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        roles.push(RoleInfo { role_id, name, server_id, power, level });
                    }
                }
            }
        }
        roles.sort_by(|a, b| b.power.cmp(&a.power));
        Ok(roles)
    }

    pub fn parse_bin(&self, bin_data: &[u8]) -> Result<serde_json::Map<String, Value>, Box<dyn std::error::Error + Send + Sync>> {
        let msg = protocol::parse_message(bin_data)?;
        Ok(msg.get_data().clone())
    }

    pub fn encode_bin(&self, data: &serde_json::Map<String, Value>) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let msg = protocol::create_message(data, None)?;
        Ok(msg)
    }

    /// Select a role from server list: modify bin's serverId, re-encode, call authuser
    /// Returns the token JSON string ready for WebSocket connection
    pub async fn select_role_token(&self, bin_data: &[u8], server_id: u64) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // 1. Parse original bin
        let mut bin_obj = self.parse_bin(bin_data)?;

        // 2. Patch serverId
        bin_obj.insert("serverId".to_string(), Value::Number(server_id.into()));

        // 3. Re-encode (BON + encrypt)
        let new_bin = self.encode_bin(&bin_obj)?;

        // 4. Call authuser with modified bin to get session token
        let token = self.transform_token(&new_bin).await?;

        Ok(token)
    }
}

fn rand_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos();
    (nanos as u64).wrapping_mul(1103515245).wrapping_add(12345)
}

impl Default for KocCore { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xor_roundtrip() {
        let original = b"hello world test data";
        let enc = XorCrypto::new();
        let encrypted = enc.encrypt(original);
        println!("Encrypted first 4 bytes: {:?}", &encrypted[..4]);
        let decrypted = auto_decrypt(&encrypted);
        assert_eq!(&decrypted, original);
    }

    #[test]
    fn test_bon_roundtrip() {
        let mut data = serde_json::Map::new();
        data.insert("key".to_string(), Value::Number(123.into()));
        data.insert("value".to_string(), Value::String("test".to_string()));
        let value = Value::Object(data.clone());
        let encoded = bon::encode(&value);
        println!("BON encoded len: {}, bytes: {:?}", encoded.len(), &encoded);
        let decoded = bon::decode(&encoded);
        println!("BON decoded: {:?}", decoded);
        assert!(decoded.is_some());
    }

    #[test]
    fn test_bon_encoding() {
        let core = KocCore::new();
        let mut data = serde_json::Map::new();
        data.insert("key".to_string(), Value::Number(123.into()));
        data.insert("value".to_string(), Value::String("test".to_string()));
        let encoded = core.encode_bin(&data).unwrap();
        println!("Encoded len: {}, first bytes: {:?}", encoded.len(), &encoded[..4.min(encoded.len())]);
        let decoded = core.parse_bin(&encoded).unwrap();
        println!("Decoded: {:?}", decoded);
    }
}
