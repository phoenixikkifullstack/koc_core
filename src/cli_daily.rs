use std::path::{Path, PathBuf};

use crate::cli_context::CliContext;
use koc_core::config::parse_server_id;
use tracing::info;

#[derive(clap::Args, Debug, Clone)]
pub struct DailyArgs {
    /// Bin file path (single-role mode)
    #[arg(long, value_name = "BIN")]
    pub bin: Option<PathBuf>,

    /// Target server ID (single-role mode)
    #[arg(long, value_name = "SERVER_ID")]
    pub server_id: Option<u64>,

    /// Run all roles in the specified config group
    #[arg(long, value_name = "NAME")]
    pub group: Option<String>,

    /// Run all roles across all bins in config.yaml
    #[arg(long, default_value_t = false)]
    pub force_all: bool,

    /// Skip car send/claim in daily tasks
    #[arg(long, default_value_t = false)]
    pub skip_car: bool,

    /// Skip free gacha draw in daily tasks
    #[arg(long, default_value_t = false)]
    pub skip_gacha: bool,
}

pub async fn run(ctx: &CliContext, args: &DailyArgs) -> Result<(), String> {
    let config = ctx.load_config()?;

    if let Some(group) = &args.group {
        let roles = config.group_roles(group)?;
        run_roles(ctx, &roles, &config, args).await
    } else if args.force_all {
        let roles = config.all_roles();
        run_roles(ctx, &roles, &config, args).await
    } else if args.bin.is_some() && args.server_id.is_some() {
        let bin = args.bin.as_ref().unwrap();
        let sid = args.server_id.unwrap();
        let prefix = role_prefix(bin, sid);
        run_single(ctx, bin, sid, &config, args, &prefix).await
    } else {
        Err("Select a mode:\n  --bin <BIN> --server-id <ID>   single role\n  --group <NAME>                 config group\n  --force-all                    all roles".into())
    }
}

async fn run_roles(
    ctx: &CliContext,
    roles: &[(String, u64)],
    config: &koc_core::config::BatchConfig,
    args: &DailyArgs,
) -> Result<(), String> {
    let total = roles.len();
    for (i, (bin_name, server_id)) in roles.iter().enumerate() {
        let bin_path = PathBuf::from(bin_name);
        let prefix = format!("[{}/{} {}]", i + 1, total, role_prefix(&bin_path, *server_id));
        info!(target: "cli", prefix = %prefix, "daily: running role");
        match run_single(ctx, &bin_path, *server_id, config, args, &prefix).await {
            Ok(()) => info!(target: "cli", prefix = %prefix, "daily: done"),
            Err(e) => info!(target: "cli", prefix = %prefix, error = %e, "daily: failed"),
        }
    }
    Ok(())
}

async fn run_single(
    ctx: &CliContext,
    bin: &PathBuf,
    server_id: u64,
    config: &koc_core::config::BatchConfig,
    args: &DailyArgs,
    prefix: &str,
) -> Result<(), String> {
    let bin_data = ctx.read_bin(bin)?;
    info!(target: "cli", bytes = bin_data.len(), bin = %bin.display(), "bin loaded");

    match ctx.core.parse_bin(&bin_data) {
        Ok(data) => info!(target: "cli", fields = data.len(), "bin parse ok"),
        Err(e) => return Err(format!("bin parse failed: {}", e)),
    }

    let roles = ctx
        .core
        .get_server_list(&bin_data)
        .await
        .map_err(|e| format!("Failed to get server list: {}", e))?;

    if !roles.iter().any(|r| r.server_id == server_id) {
        return Err(format!(
            "server_id {} not found in this bin (use `koc_cli verify --bin ...` to inspect)",
            server_id
        ));
    }

    let token = ctx
        .core
        .select_role_token(&bin_data, server_id)
        .await
        .map_err(|e| format!("Failed to get role token: {}", e))?;

    let mut game = koc_core::GameClient::login(&token)
        .await
        .map_err(|e| format!("Login failed: {}", e))?;

    info!(target: "cli", role = %server_id, "connected");

    let bin_name = bin.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let mut daily_cfg = config.daily_task_config(bin_name, server_id);
    if args.skip_car { daily_cfg.car_enabled = false; }
    if args.skip_gacha { daily_cfg.gacha_enabled = false; }

    let mut daily = koc_core::state::RoleDailyState::default();
    let report = game.run_daily_tasks_stateful(&mut daily, prefix, &daily_cfg).await;

    info!(target: "cli", summary = %report.summary(), "daily tasks completed");

    game.disconnect().await;
    Ok(())
}

fn role_prefix(bin: &Path, server_id: u64) -> String {
    let stem = bin.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
    let (server_num, idx) = parse_server_id(server_id);
    format!("[{}/{}-{}]", stem, server_num, idx)
}
