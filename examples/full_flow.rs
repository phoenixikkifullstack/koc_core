use std::fs;
use koc_core::{KocCore, GameClient};

#[tokio::main]
async fn main() {
    let bin_path = "liulian.bin";
    let bin_data = fs::read(bin_path).expect("Failed to read liulian.bin");
    println!("=== KOC Full Flow Demo ===\n");

    let core = KocCore::new();

    // Step 1: Get server/role list
    println!("[1] Fetching role list...");
    let roles = core.get_server_list(&bin_data).await.expect("Failed to get roles");
    println!("    Found {} roles:", roles.len());
    for (i, role) in roles.iter().enumerate() {
        let mut sid = role.server_id;
        let idx = if sid >= 2000000 { sid -= 2000000; 2 }
                  else if sid >= 1000000 { sid -= 1000000; 1 }
                  else { 0 };
        println!("    [{}] {}服-{} name={} power={} roleId={} serverId={}",
            i + 1, sid - 27, idx, role.name, role.power, role.role_id, role.server_id);
    }

    if roles.is_empty() {
        println!("No roles found, exiting.");
        return;
    }

    // Step 2: Select first (strongest) role, get new token
    let selected = &roles[0];
    println!("\n[2] Selecting role: {} (serverId={})", selected.name, selected.server_id);
    let token = core.select_role_token(&bin_data, selected.server_id).await
        .expect("Failed to get role token");
    println!("    Got token ({} bytes)", token.len());

    // Step 3: Login via WebSocket
    println!("\n[3] Logging in via WebSocket...");
    let mut game = match GameClient::login(&token).await {
        Ok(g) => g,
        Err(e) => {
            println!("    Login failed: {}", e);
            return;
        }
    };
    println!("    Login successful!");

    if let Some(ref info) = game.role_info {
        if let Some(name) = info.pointer("/role/name").and_then(|v| v.as_str()) {
            println!("    Role: {}", name);
        }
        if let Some(power) = info.pointer("/role/power").and_then(|v| v.as_u64()) {
            println!("    Power: {}", power);
        }
    }
    if let Some(bv) = game.battle_version {
        println!("    Battle version: {}", bv);
    }

    // Step 4: Run daily tasks (smart: skip already completed)
    game.run_daily_tasks().await;

    // Step 5: Disconnect
    println!("\nDisconnecting...");
    game.disconnect().await;
    println!("\n=== Done ===");
}
