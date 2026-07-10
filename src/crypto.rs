use rand::RngExt;

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
        if data.len() < 4 {
            return vec![];
        }
        let mask = ((data[2] >> 6) & 1) << 7
            | ((data[2] >> 4) & 1) << 6
            | ((data[2] >> 2) & 1) << 5
            | (data[2] & 1) << 4
            | ((data[3] >> 6) & 1) << 3
            | ((data[3] >> 4) & 1) << 2
            | ((data[3] >> 2) & 1) << 1
            | (data[3] & 1);
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
        let compressed = lz4_flex::compress_prepend_size(data);
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
        if data.len() < 4 {
            return vec![];
        }
        let mask = ((data[2] >> 6) & 1) << 7
            | ((data[2] >> 4) & 1) << 6
            | ((data[2] >> 2) & 1) << 5
            | (data[2] & 1) << 4
            | ((data[3] >> 6) & 1) << 3
            | ((data[3] >> 4) & 1) << 2
            | ((data[3] >> 2) & 1) << 1
            | (data[3] & 1);
        let mut decrypted = data.to_vec();
        for (_i, b) in decrypted.iter_mut().enumerate().take(100).skip(2) {
            *b ^= mask;
        }
        decrypted[0] = 4;
        decrypted[1] = 34;
        decrypted[2] = 77;
        decrypted[3] = 24;
        // lz4 frame format (magic: 04 22 4d 18), use frame decoder
        use std::io::Read;
        let mut decoder = lz4_flex::frame::FrameDecoder::new(&decrypted[..]);
        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap_or(0);
        output
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
    if data.len() > 4 && data[0] == 112 && data[1] == 108 {
        get_encryptor("lx").decrypt(data)
    } else if data.len() > 4 && data[0] == 112 && data[1] == 120 {
        get_encryptor("x").decrypt(data)
    } else {
        data.to_vec()
    }
}
