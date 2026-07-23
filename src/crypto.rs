use rand::RngExt;
use std::io::Read;

pub const DEFAULT_MAX_DECOMPRESSED_SIZE: usize = 32 * 1024 * 1024;

pub trait Encryptor: Send + Sync {
    fn encrypt(&self, data: &[u8]) -> Vec<u8>;
    fn decrypt(&self, data: &[u8]) -> Vec<u8>;
}

pub struct XorCrypto;
impl XorCrypto {
    pub fn new() -> Self {
        Self
    }
}
impl Encryptor for XorCrypto {
    fn encrypt(&self, data: &[u8]) -> Vec<u8> {
        let mut rng = rand::rng();
        let mask: u8 = rng.random_range(2..250);
        // construct: [4 random bytes] + data
        let mut n = Vec::with_capacity(data.len() + 4);
        let rnd: u32 = rng.random();
        n.push(rnd as u8);
        n.push((rnd >> 8) as u8);
        n.push((rnd >> 16) as u8);
        n.push((rnd >> 24) as u8);
        n.extend_from_slice(data);
        // XOR
        for b in n.iter_mut() {
            *b ^= mask;
        }
        // set MAGIC BYTES and MASK code
        n[0] = 112;
        n[1] = 120;
        n[2] = (n[2] & 0b10101010)
            | ((mask >> 7) & 1) << 6
            | ((mask >> 6) & 1) << 4
            | ((mask >> 5) & 1) << 2
            | ((mask >> 4) & 1);
        n[3] = (n[3] & 0b10101010)
            | ((mask >> 3) & 1) << 6
            | ((mask >> 2) & 1) << 4
            | ((mask >> 1) & 1) << 2
            | (mask & 1);
        n
    }
    fn decrypt(&self, data: &[u8]) -> Vec<u8> {
        let Some(mask) = extract_mask(data) else {
            return Vec::new();
        };
        let mut decrypted = data[4..].to_vec();
        for b in decrypted.iter_mut() {
            *b ^= mask;
        }
        decrypted
    }
}

pub struct Lz4Crypto;
impl Lz4Crypto {
    pub fn new() -> Self {
        Self
    }
}
impl Encryptor for Lz4Crypto {
    fn encrypt(&self, data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = lz4_flex::frame::FrameEncoder::new(Vec::new());
        if encoder.write_all(data).is_err() {
            return Vec::new();
        }
        let Ok(compressed) = encoder.finish() else {
            return Vec::new();
        };
        let mut rng = rand::rng();
        let mask: u8 = rng.random_range(2..250);
        let mut encrypted = compressed.clone();
        for (i, b) in encrypted.iter_mut().enumerate() {
            if i < 100 {
                *b ^= mask;
            }
        }
        encrypted[0] = 112;
        encrypted[1] = 108;
        encrypted[2] = (encrypted[2] & 0b10101010)
            | ((mask >> 7) & 1) << 6
            | ((mask >> 6) & 1) << 4
            | ((mask >> 5) & 1) << 2
            | ((mask >> 4) & 1);
        encrypted[3] = (encrypted[3] & 0b10101010)
            | ((mask >> 3) & 1) << 6
            | ((mask >> 2) & 1) << 4
            | ((mask >> 1) & 1) << 2
            | (mask & 1);
        encrypted
    }
    fn decrypt(&self, data: &[u8]) -> Vec<u8> {
        decrypt_lz4_checked(data, DEFAULT_MAX_DECOMPRESSED_SIZE).unwrap_or_default()
    }
}

pub fn get_encryptor(name: &str) -> Box<dyn Encryptor> {
    match name {
        "x" => Box::new(XorCrypto::new()),
        "lx" => Box::new(Lz4Crypto::new()),
        _ => Box::new(XorCrypto::new()),
    }
}

pub fn auto_decrypt(data: &[u8]) -> Vec<u8> {
    try_auto_decrypt(data, DEFAULT_MAX_DECOMPRESSED_SIZE).unwrap_or_default()
}

pub fn try_auto_decrypt(data: &[u8], max_decompressed_size: usize) -> Result<Vec<u8>, String> {
    match data.get(..2) {
        Some([112, 108]) => decrypt_lz4_checked(data, max_decompressed_size),
        Some([112, 120]) => {
            if data.len() < 4 {
                return Err("truncated x payload".to_string());
            }
            if data.len() - 4 > max_decompressed_size {
                return Err(format!(
                    "x payload exceeds limit: {} > {} bytes",
                    data.len() - 4,
                    max_decompressed_size
                ));
            }
            Ok(XorCrypto::new().decrypt(data))
        }
        _ => {
            if data.len() > max_decompressed_size {
                return Err(format!(
                    "plain payload exceeds limit: {} > {} bytes",
                    data.len(), max_decompressed_size
                ));
            }
            Ok(data.to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn encode_lx_fixture(data: &[u8], mask: u8) -> Vec<u8> {
        let mut encoder = lz4_flex::frame::FrameEncoder::new(Vec::new());
        encoder.write_all(data).unwrap();
        let mut encoded = encoder.finish().unwrap();
        for byte in encoded.iter_mut().take(100) {
            *byte ^= mask;
        }
        encoded[0] = 112;
        encoded[1] = 108;
        encoded[2] = (encoded[2] & 0b10101010)
            | ((mask >> 7) & 1) << 6
            | ((mask >> 6) & 1) << 4
            | ((mask >> 5) & 1) << 2
            | ((mask >> 4) & 1);
        encoded[3] = (encoded[3] & 0b10101010)
            | ((mask >> 3) & 1) << 6
            | ((mask >> 2) & 1) << 4
            | ((mask >> 1) & 1) << 2
            | (mask & 1);
        encoded
    }

    #[test]
    fn checked_lz4_decode_enforces_output_limit() {
        let original = b"captured websocket payload";
        let encoded = encode_lx_fixture(original, 37);
        assert_eq!(try_auto_decrypt(&encoded, original.len()).unwrap(), original);
        assert!(try_auto_decrypt(&encoded, original.len() - 1).is_err());
    }

    #[test]
    fn checked_decode_rejects_truncated_encrypted_payload() {
        assert!(try_auto_decrypt(b"px", 1024).is_err());
        assert!(try_auto_decrypt(b"pl", 1024).is_err());
    }

    #[test]
    fn lz4_encryptor_roundtrips_frame_format() {
        let original = b"roundtrip compressed payload";
        let encryptor = Lz4Crypto::new();
        let encrypted = encryptor.encrypt(original);
        assert_eq!(encryptor.decrypt(&encrypted), original);
    }
}

fn extract_mask(data: &[u8]) -> Option<u8> {
    if data.len() < 4 {
        return None;
    }
    Some(
        ((data[2] >> 6) & 1) << 7
            | ((data[2] >> 4) & 1) << 6
            | ((data[2] >> 2) & 1) << 5
            | (data[2] & 1) << 4
            | ((data[3] >> 6) & 1) << 3
            | ((data[3] >> 4) & 1) << 2
            | ((data[3] >> 2) & 1) << 1
            | (data[3] & 1),
    )
}

fn decrypt_lz4_checked(data: &[u8], max_decompressed_size: usize) -> Result<Vec<u8>, String> {
    let mask = extract_mask(data).ok_or_else(|| "truncated lx payload".to_string())?;
    let mut decrypted = data.to_vec();
    for b in decrypted.iter_mut().take(100).skip(2) {
        *b ^= mask;
    }
    decrypted[..4].copy_from_slice(&[4, 34, 77, 24]);

    let decoder = lz4_flex::frame::FrameDecoder::new(&decrypted[..]);
    let read_limit = max_decompressed_size.saturating_add(1) as u64;
    let mut output = Vec::new();
    decoder
        .take(read_limit)
        .read_to_end(&mut output)
        .map_err(|e| format!("LZ4 decode failed: {}", e))?;
    if output.len() > max_decompressed_size {
        return Err(format!(
            "LZ4 output exceeds limit: {} > {} bytes",
            output.len(), max_decompressed_size
        ));
    }
    Ok(output)
}
