use std::path::PathBuf;

use crate::cli_context::CliContext;
use tracing::info;

#[derive(clap::Args, Debug, Clone)]
pub struct CarArgs {
    #[arg(long, value_name = "BIN")]
    pub bin: PathBuf,

    #[arg(long, value_name = "SERVER_ID")]
    pub server_id: u64,

    #[arg(long, value_name = "ACTION", default_value = "send")]
    pub action: String,
}

pub async fn run(ctx: &CliContext, args: &CarArgs) -> Result<(), String> {
    let bin_data = ctx.read_bin(&args.bin)?;
    info!(target: "cli", bytes = bin_data.len(), bin = %args.bin.display(), "bin loaded");

    match ctx.core.parse_bin(&bin_data) {
        Ok(data) => info!(target: "cli", fields = data.len(), "bin parse ok"),
        Err(e) => return Err(format!("bin parse failed: {}", e)),
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

    let log_prefix = "car";

    match args.action.as_str() {
        "send" => {
            let (claim_report, _) = game.claim_all_cars(log_prefix).await;
            info!(target: "cli", summary = %claim_report.summary(), "claim all cars done");
            let report = game.smart_send_car(log_prefix).await;
            info!(target: "cli", summary = %report.summary(), "smart send car done");
        }
        "claim" => {
            let (report, _) = game.claim_all_cars(log_prefix).await;
            info!(target: "cli", summary = %report.summary(), "claim all cars done");
        }
        other => {
            return Err(format!("Unknown action '{}', use 'send' or 'claim'", other));
        }
    }

    Ok(())
}