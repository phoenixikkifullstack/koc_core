use std::fs;

#[tokio::main]
async fn main() {
    let bin_path = "liulian.bin";
    let bin_data = fs::read(bin_path).expect("Failed to read liulian.bin");
    println!("Read {} bytes from {}", bin_data.len(), bin_path);

    // get server list raw json
    println!("\n--- Fetching server/role list ---");
    let http = koc_core::HttpClient::new();
    match http.post_serverlist(&bin_data).await {
        Ok(resp) => {
            match koc_core::parse_message(&resp) {
                Ok(proto) => {
                    match proto.raw_data() {
                        Some(body_val) => {
                            let json = serde_json::to_string_pretty(&body_val).unwrap();
                            let out_path = "server_list.json";
                            std::fs::write(out_path, &json).expect("Failed to write json");
                            println!("Written {} bytes to {}", json.len(), out_path);

                            // print roles summary
                            if let Some(roles) = body_val.get("roles") {
                                if let Some(obj) = roles.as_object() {
                                    println!("\nRoles ({}):", obj.len());
                                    for (sid, role) in obj {
                                        println!("  serverId={}, name={}, roleId={}, power={}, level={}",
                                            sid,
                                            role.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
                                            role.get("roleId").and_then(|v| v.as_u64()).unwrap_or(0),
                                            role.get("power").and_then(|v| v.as_u64()).unwrap_or(0),
                                            role.get("level").and_then(|v| v.as_u64()).unwrap_or(0),
                                        );
                                    }
                                }
                            }
                        }
                        None => println!("raw_data() returned None"),
                    }
                }
                Err(e) => println!("Parse error: {}", e),
            }
        }
        Err(e) => println!("HTTP error: {}", e),
    }
}
