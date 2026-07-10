use std::path::PathBuf;

use serde_json::json;
use tracing::{info, warn};

use crate::cli_context::CliContext;

#[derive(clap::Args, Debug, Clone)]
pub struct StudyArgs {
    #[arg(long, value_name = "BIN")]
    pub bin: PathBuf,

    #[arg(long, value_name = "SERVER_ID")]
    pub server_id: u64,

    #[arg(long, default_value_t = false)]
    pub force: bool,
}

pub async fn run(ctx: &CliContext, args: &StudyArgs) -> Result<(), String> {
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

    let mut game = koc_core::GameClient::login(&token)
        .await
        .map_err(|e| format!("Login failed: {}", e))?;

    let completed_this_week = koc_core::study::is_study_completed_this_week(game.role_info.as_ref());
    if completed_this_week && !args.force {
        info!(target: "cli", "[~~] Study already completed this week");
        game.disconnect().await;
        return Ok(());
    }
    if completed_this_week && args.force {
        info!(target: "cli", "[CLI] force=true, continue study flow despite completed status");
    }

    let mut report = koc_core::kpi::DailyTaskReport::new();
    koc_core::study::run_study(&game.ws, &mut report, "[CLI]").await?;
    info!(target: "cli", summary = %report.summary(), "study flow completed");

    let _ = game.ws.send_with_response(
        "role_getroleinfo",
        Some(json!({
            "clientVersion": "2.21.2-fa918e1997301834-wx",
            "inviteUid": 0,
            "platform": "hortor",
            "platformExt": "mix",
            "scene": ""
        })),
        8000,
    ).await;

    game.disconnect().await;
    Ok(())
}
