use std::path::PathBuf;

use crate::cli_context::CliContext;
use tracing::info;

#[derive(clap::Args, Debug, Clone)]
pub struct GachaArgs {
    #[arg(long, value_name = "BIN")]
    pub bin: PathBuf,

    #[arg(long, value_name = "SERVER_ID")]
    pub server_id: u64,
}

pub async fn run(ctx: &CliContext, args: &GachaArgs) -> Result<(), String> {
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

    // 检查今天是否已扭蛋 (statistics["gacha:free"])
    let gacha_free = game.role_info.as_ref()
        .and_then(|v| v.pointer("/role/statistics").or_else(|| v.pointer("/statistics")))
        .and_then(|s| s.get("gacha:free"));
    if !koc_core::kpi::is_today_available(gacha_free) {
        info!(target: "cli", "[~~] Gacha already done today");
        return Ok(());
    }

    // debug: 打印 statistics 和 statisticsTime 中的 gacha:free
    match game.role_info.as_ref() {
        Some(info) => {
            let st_time = info.pointer("/role/statisticsTime").or_else(|| info.pointer("/statisticsTime"));
            let gacha_free_time = st_time.and_then(|s| s.get("gacha:free"));
            let st = info.pointer("/role/statistics").or_else(|| info.pointer("/statistics"));
            let gacha_free_stat = st.and_then(|s| s.get("gacha:free"));
            info!(target: "cli", gacha_free_time = ?gacha_free_time, gacha_free_stat = ?gacha_free_stat, "gacha:free from statisticsTime & statistics");
        }
        None => {
            info!(target: "cli", "no role_info available");
        }
    }

    match game.gacha_drawreward().await {
        Ok(resp) => {
            info!(target: "cli", "gacha draw success");
            println!("{}", serde_json::to_string_pretty(&resp).unwrap_or_default());
        }
        Err(e) => {
            info!(target: "cli", error = %e, "gacha draw FAILED");
            return Err(format!("gacha draw failed: {}", e));
        }
    }

    Ok(())
}
