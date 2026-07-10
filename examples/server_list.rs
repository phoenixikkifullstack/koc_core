use std::fs;
use serde_json::Value;
use koc_core::KocCore;

#[tokio::main]
async fn main() {
    let bin_path = "liulian.bin";
    let bin_data = fs::read(bin_path).expect("Failed to read liulian.bin");
    println!("Read {} bytes from {}", bin_data.len(), bin_path);

    let core = KocCore::new();

    // 1. get server list - raw response for debugging
    println!("\n--- Fetching server/role list (raw) ---");
    {
        let http = koc_core::HttpClient::new();
        match http.post_serverlist(&bin_data).await {
            Ok(resp) => {
                let msg = koc_core::parse_message(&resp);
                match msg {
                    Ok(proto) => {
                        let data = proto.get_data();
                        println!("Response fields ({}):", data.len());
                        for (k, v) in data {
                            let s = format!("{}", v);
                            if s.len() > 200 {
                                println!("  {} = {}...", k, &s[..200]);
                            } else {
                                println!("  {} = {}", k, s);
                            }
                        }
                        // Try raw_data decode
                        if let Some(Value::String(body_str)) = data.get("body") {
                            println!("\nBody base64 length: {}", body_str.len());
                            println!("Body first 20 chars: {}", &body_str[..20.min(body_str.len())]);

                            // manual base64 decode test
                            fn b64decode(s: &str) -> Option<Vec<u8>> {
                                let s = s.trim_end_matches('=');
                                let table = |c: u8| -> Option<u8> {
                                    match c { b'A'..=b'Z' => Some(c - b'A'), b'a'..=b'z' => Some(c - b'a' + 26), b'0'..=b'9' => Some(c - b'0' + 52), b'+' => Some(62), b'/' => Some(63), _ => None }
                                };
                                let bytes = s.as_bytes();
                                let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
                                for chunk in bytes.chunks(4) {
                                    let a = table(*chunk.first()?)?;
                                    let b = table(*chunk.get(1)?)?;
                                    out.push((a << 2) | (b >> 4));
                                    if let Some(&c) = chunk.get(2) { let c = table(c)?; out.push((b << 4) | (c >> 2)); if let Some(&d) = chunk.get(3) { let d = table(d)?; out.push((c << 6) | d); } }
                                }
                                Some(out)
                            }

                            match b64decode(body_str) {
                                Some(bytes) => {
                                    println!("Base64 decoded: {} bytes, first 20: {:?}", bytes.len(), &bytes[..20.min(bytes.len())]);
                                    match koc_core::bon::decode(&bytes) {
                                        Some(val) => {
                                            if let Some(obj) = val.as_object() {
                                                println!("BON decoded body fields: {:?}", obj.keys().collect::<Vec<_>>());
                                            } else {
                                                println!("BON decoded (not obj): {:?}", val);
                                            }
                                        }
                                        None => println!("BON decode returned None"),
                                    }
                                }
                                None => println!("Base64 decode failed"),
                            }
                        }
                        match proto.raw_data() {
                            Some(body_val) => {
                                if let Some(obj) = body_val.as_object() {
                                    println!("\nDecoded body fields ({}):", obj.len());
                                    for (k, v) in obj {
                                        let s = format!("{}", v);
                                        if s.len() > 200 {
                                            println!("  {} = {}...", k, &s[..200]);
                                        } else {
                                            println!("  {} = {}", k, s);
                                        }
                                    }
                                } else {
                                    println!("\nDecoded body (not object): {}", body_val);
                                }
                            }
                            None => println!("\nraw_data() returned None"),
                        }
                    }
                    Err(e) => println!("Parse error: {}", e),
                }
            }
            Err(e) => println!("HTTP error: {}", e),
        }
    }

    // 2. get server list - parsed roles
    println!("\n--- Fetching server/role list (parsed) ---");
    match core.get_server_list(&bin_data).await {
        Ok(roles) => {
            println!("Found {} roles (sorted by power):\n", roles.len());
            for (i, role) in roles.iter().enumerate() {
                println!("  [{}] name={}, roleId={}, serverId={}, power={}, level={}",
                    i + 1, role.name, role.role_id, role.server_id, role.power, role.level);
            }
        }
        Err(e) => {
            println!("Failed to get server list: {}", e);
        }
    }

    // 2. transform token (authuser)
    println!("\n--- Fetching auth token ---");
    match core.transform_token(&bin_data).await {
        Ok(token) => {
            let parsed: serde_json::Value = serde_json::from_str(&token).unwrap_or_default();
            println!("Auth token fields:");
            if let Some(obj) = parsed.as_object() {
                for (k, v) in obj {
                    let s = format!("{}", v);
                    if s.len() > 80 {
                        println!("  {} = {}...", k, &s[..80]);
                    } else {
                        println!("  {} = {}", k, s);
                    }
                }
            }
        }
        Err(e) => {
            println!("Failed to get auth token: {}", e);
        }
    }
}
