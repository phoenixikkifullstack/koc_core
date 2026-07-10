use serde_json::Value;
use std::collections::HashMap;
use tracing::warn;

pub struct DataReader {
    data: Vec<u8>,
    position: usize,
}

impl DataReader {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data, position: 0 }
    }

    pub fn reset(&mut self, data: Vec<u8>) {
        self.data = data;
        self.position = 0;
    }

    fn validate(&self, n: usize) -> bool {
        self.position + n <= self.data.len()
    }

    pub fn read_u8(&mut self) -> Option<u8> {
        if !self.validate(1) {
            return None;
        }
        let v = self.data[self.position];
        self.position += 1;
        Some(v)
    }

    pub fn read_i16(&mut self) -> Option<i16> {
        if !self.validate(2) {
            return None;
        }
        let lo = self.data[self.position] as u16;
        let hi = self.data[self.position + 1] as u16;
        self.position += 2;
        Some((lo | (hi << 8)) as i16)
    }

    pub fn read_i32(&mut self) -> Option<i32> {
        if !self.validate(4) {
            return None;
        }
        let b0 = self.data[self.position] as u32;
        let b1 = self.data[self.position + 1] as u32;
        let b2 = self.data[self.position + 2] as u32;
        let b3 = self.data[self.position + 3] as u32;
        self.position += 4;
        Some((b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)) as i32)
    }

    pub fn read_i64(&mut self) -> Option<i64> {
        let lo = self.read_i32()? as i64;
        let hi = self.read_i32()?;
        let lo = if lo < 0 { lo + 0x100000000 } else { lo };
        Some(lo + 0x100000000 * hi as i64)
    }

    pub fn read_f32(&mut self) -> Option<f32> {
        if !self.validate(4) {
            return None;
        }
        let mut buf = [0u8; 4];
        buf.copy_from_slice(&self.data[self.position..self.position + 4]);
        self.position += 4;
        Some(f32::from_le_bytes(buf))
    }

    pub fn read_f64(&mut self) -> Option<f64> {
        if !self.validate(8) {
            return None;
        }
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&self.data[self.position..self.position + 8]);
        self.position += 8;
        Some(f64::from_le_bytes(buf))
    }

    pub fn read_7bit_int(&mut self) -> Option<u32> {
        let mut value: u32 = 0;
        let mut shift = 0;
        let mut count = 0;
        loop {
            if count > 35 {
                return None;
            }
            let b = self.read_u8()?;
            value |= ((b & 0x7f) as u32) << shift;
            shift += 7;
            if (b & 0x80) == 0 {
                break;
            }
            count += 1;
        }
        Some(value)
    }

    pub fn read_utf(&mut self) -> Option<String> {
        let len = self.read_7bit_int()? as usize;
        self.read_utf_bytes(len)
    }

    pub fn read_utf_bytes(&mut self, len: usize) -> Option<String> {
        if len == 0 {
            return Some(String::new());
        }
        if !self.validate(len) {
            return None;
        }
        let s = String::from_utf8_lossy(&self.data[self.position..self.position + len]).to_string();
        self.position += len;
        Some(s)
    }

    pub fn read_bytes(&mut self, len: usize) -> Option<Vec<u8>> {
        if !self.validate(len) {
            return None;
        }
        let v = self.data[self.position..self.position + len].to_vec();
        self.position += len;
        Some(v)
    }
}

pub struct DataWriter {
    data: Vec<u8>,
}

impl DataWriter {
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }
    pub fn reset(&mut self) {
        self.data.clear();
    }
    pub fn write_u8(&mut self, v: u8) {
        self.data.push(v);
    }
    pub fn write_i16(&mut self, v: i16) {
        self.data.push(v as u8);
        self.data.push((v >> 8) as u8);
    }
    pub fn write_i32(&mut self, v: i32) {
        self.data.push(v as u8);
        self.data.push((v >> 8) as u8);
        self.data.push((v >> 16) as u8);
        self.data.push((v >> 24) as u8);
    }
    pub fn write_i64(&mut self, v: i64) {
        self.write_i32(v as i32);
        if v < 0 {
            self.write_i32(!((-v / 0x100000000) as i32));
        } else {
            self.write_i32((v / 0x100000000) as i32);
        }
    }
    pub fn write_f32(&mut self, v: f32) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }
    pub fn write_f64(&mut self, v: f64) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    fn write_7bit_int(&mut self, v: usize) {
        let mut n = v as u32;
        while n >= 0x80 {
            self.data.push((n as u8) | 0x80);
            n >>= 7;
        }
        self.data.push(n as u8);
    }

    pub fn write_utf(&mut self, s: &str) {
        if s.is_empty() {
            self.write_7bit_int(0);
            return;
        }
        let encoded = s.as_bytes();
        self.write_7bit_int(encoded.len());
        self.data.extend_from_slice(encoded);
    }

    pub fn write_bytes(&mut self, src: &[u8]) {
        self.data.extend_from_slice(src);
    }
    pub fn get_bytes(&self) -> Vec<u8> {
        self.data.clone()
    }
}

impl Default for DataWriter {
    fn default() -> Self {
        Self::new()
    }
}

pub struct BonEncoder {
    dw: DataWriter,
    str_map: HashMap<String, usize>,
}

impl BonEncoder {
    pub fn new() -> Self {
        Self {
            dw: DataWriter::new(),
            str_map: HashMap::new(),
        }
    }
    pub fn reset(&mut self) {
        self.dw.reset();
        self.str_map.clear();
    }
    pub fn encode(&mut self, value: &Value) {
        match value {
            Value::Null => self.dw.write_u8(0),
            Value::Bool(b) => {
                self.dw.write_u8(6);
                self.dw.write_u8(if *b { 1 } else { 0 });
            }
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    if i as i32 as i64 == i {
                        self.dw.write_u8(1);
                        self.dw.write_i32(i as i32);
                    } else {
                        self.dw.write_u8(2);
                        self.dw.write_i64(i);
                    }
                } else if let Some(f) = n.as_f64() {
                    self.dw.write_u8(4);
                    self.dw.write_f64(f);
                }
            }
            Value::String(s) => {
                if let Some(idx) = self.str_map.get(s) {
                    self.dw.write_u8(99);
                    self.dw.write_7bit_int(*idx);
                } else {
                    self.dw.write_u8(5);
                    self.dw.write_utf(s);
                    self.str_map.insert(s.to_string(), self.str_map.len());
                }
            }
            Value::Array(arr) => {
                self.dw.write_u8(9);
                self.dw.write_7bit_int(arr.len());
                for v in arr {
                    self.encode(v);
                }
            }
            Value::Object(obj) => {
                self.dw.write_u8(8);
                self.dw.write_7bit_int(obj.len());
                for (k, v) in obj {
                    self.encode(&Value::String(k.clone()));
                    self.encode(v);
                }
            }
        }
    }
    /// Encode raw binary data as BON tag 7 (binary)
    pub fn encode_binary(&mut self, data: &[u8]) {
        self.dw.write_u8(7);
        self.dw.write_7bit_int(data.len());
        self.dw.write_bytes(data);
    }
    pub fn get_bytes(&self) -> Vec<u8> {
        self.dw.get_bytes()
    }
}

pub struct BonDecoder {
    dr: DataReader,
    str_arr: Vec<String>,
}

impl BonDecoder {
    pub fn new() -> Self {
        Self {
            dr: DataReader::new(Vec::new()),
            str_arr: Vec::new(),
        }
    }
    pub fn reset(&mut self, data: Vec<u8>) {
        self.dr.reset(data);
        self.str_arr.clear();
    }
    pub fn decode(&mut self) -> Option<Value> {
        let tag = self.dr.read_u8()?;
        match tag {
            0 => Some(Value::Null),
            1 => self
                .dr
                .read_i32()
                .map(|v| Value::Number(serde_json::Number::from(v))),
            2 => self
                .dr
                .read_i64()
                .map(|v| Value::Number(serde_json::Number::from(v))),
            3 => self.dr.read_f32().map(|v| {
                serde_json::Number::from_f64(v as f64)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }),
            4 => self.dr.read_f64().map(|v| {
                serde_json::Number::from_f64(v)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }),
            5 => {
                let s = self.dr.read_utf()?;
                self.str_arr.push(s.clone());
                Some(Value::String(s))
            }
            6 => self.dr.read_u8().map(|v| Value::Bool(v == 1)),
            7 => {
                let len = self.dr.read_7bit_int()? as usize;
                self.dr
                    .read_bytes(len)
                    .map(|v| Value::String(base64::encode(&v)))
            }
            8 => {
                let count = self.dr.read_7bit_int()? as usize;
                let mut obj = serde_json::Map::new();
                for _ in 0..count {
                    let k = match self.decode()? {
                        Value::String(s) => s,
                        other => format!("{}", other),
                    };
                    let v = self.decode()?;
                    obj.insert(k, v);
                }
                Some(Value::Object(obj))
            }
            9 => {
                let len = self.dr.read_7bit_int()? as usize;
                let mut arr = Vec::with_capacity(len);
                for _ in 0..len {
                    arr.push(self.decode()?);
                }
                Some(Value::Array(arr))
            }
            99 => {
                let idx = self.dr.read_7bit_int()? as usize;
                self.str_arr.get(idx).cloned().map(Value::String)
            }
            10 => {
                // DateTime: read i64 timestamp (ms since epoch)
                self.dr
                    .read_i64()
                    .map(|v| Value::Number(serde_json::Number::from(v)))
            }
            _ => {
                warn!(target: "bon", tag = tag, position = self.dr.position.saturating_sub(1), "unknown BON tag");
                None
            }
        }
    }
}

pub fn encode(value: &Value) -> Vec<u8> {
    let mut enc = BonEncoder::new();
    enc.encode(value);
    enc.get_bytes()
}
pub fn decode(data: &[u8]) -> Option<Value> {
    let mut dec = BonDecoder::new();
    dec.reset(data.to_vec());
    dec.decode()
}

/// Encode a game packet: {cmd, ack, seq, time} with body as raw BON binary (tag 7)
pub fn encode_game_packet(
    cmd: &str,
    ack: u32,
    seq: u32,
    time: i64,
    body_params: Option<&Value>,
) -> Vec<u8> {
    let mut enc = BonEncoder::new();
    // Object with 5 fields (or 4 if no body)
    let field_count = if body_params.is_some() { 5 } else { 4 };
    enc.dw.write_u8(8);
    enc.dw.write_7bit_int(field_count);

    // cmd
    enc.encode(&Value::String("cmd".to_string()));
    enc.encode(&Value::String(cmd.to_string()));
    // ack
    enc.encode(&Value::String("ack".to_string()));
    enc.encode(&Value::Number((ack as i64).into()));
    // seq
    enc.encode(&Value::String("seq".to_string()));
    enc.encode(&Value::Number((seq as i64).into()));
    // time
    enc.encode(&Value::String("time".to_string()));
    enc.encode(&Value::Number(time.into()));
    // body: BON-encode params, then write as binary (tag 7)
    if let Some(params) = body_params {
        let body_bytes = encode(params);
        enc.encode(&Value::String("body".to_string()));
        enc.encode_binary(&body_bytes);
    }

    enc.get_bytes()
}

mod base64 {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    pub fn encode(data: &[u8]) -> String {
        let mut result = String::new();
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as usize;
            let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
            let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
            result.push(CHARS[b0 >> 2] as char);
            result.push(CHARS[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
            if chunk.len() > 1 {
                result.push(CHARS[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
            } else {
                result.push('=');
            }
            if chunk.len() > 2 {
                result.push(CHARS[b2 & 0x3f] as char);
            } else {
                result.push('=');
            }
        }
        result
    }
}
