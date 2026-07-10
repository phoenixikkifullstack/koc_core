use std::path::PathBuf;

use crate::cli_context::CliContext;
use tracing::{info, warn};

#[derive(clap::ValueEnum, Debug, Clone)]
pub enum InfoType {
    /// role_getroleinfo response
    Role,
    /// evotower_getinfo response
    Evotower,
}

#[derive(clap::Args, Debug, Clone)]
pub struct InfoArgs {
    #[arg(long, value_name = "BIN")]
    pub bin: PathBuf,

    #[arg(long, value_name = "SERVER_ID")]
    pub server_id: u64,

    #[arg(long, value_name = "TYPE", default_value = "role")]
    pub info_type: InfoType,
}

pub async fn run(ctx: &CliContext, args: &InfoArgs) -> Result<(), String> {
    let bin_data = ctx.read_bin(&args.bin)?;
    info!(target: "cli", bytes = bin_data.len(), bin = %args.bin.display(), "bin loaded");

    match ctx.core.parse_bin(&bin_data) {
        Ok(data) => info!(target: "cli", fields = data.len(), "bin parse ok"),
        Err(e) => warn!(target: "cli", error = %e, "bin parse failed"),
    }

    let roles = ctx
        .core
        .get_server_list(&bin_data)
        .await
        .map_err(|e| format!("Failed to get server list: {}", e))?;

    if !roles.iter().any(|r| r.server_id == args.server_id) {
        return Err(format!(
            "server_id {} not found in this bin (use `koc_cli verify --bin ...` to inspect)",
            args.server_id
        ));
    }

    let token = ctx
        .core
        .select_role_token(&bin_data, args.server_id)
        .await
        .map_err(|e| format!("Failed to get role token: {}", e))?;

    let game = koc_core::GameClient::login(&token)
        .await
        .map_err(|e| format!("Login failed: {}", e))?;

    info!(target: "cli", role = %args.server_id, "connected");

    let data = match args.info_type {
        InfoType::Role => game.role_info.clone().ok_or("role_info not available after login")?,
        InfoType::Evotower => game.evotower_getinfo().await.map_err(|e| format!("evotower_getinfo failed: {}", e))?,
    };

    println!("{}", serde_json::to_string_pretty(&data).map_err(|e| format!("JSON serialize failed: {}", e))?);

    Ok(())
}
