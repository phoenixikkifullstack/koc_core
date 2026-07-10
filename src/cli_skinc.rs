use std::path::{Path, PathBuf};

use crate::cli_context::CliContext;
use koc_core::GameClient;
use tracing::info;

#[derive(clap::Args, Debug, Clone)]
pub struct SkinCArgs {
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
}

pub async fn run(ctx: &CliContext, args: &SkinCArgs) -> Result<(), String> {
    let config = ctx.load_config()?;

    if let Some(group) = &args.group {
        let roles = config.group_roles(group)?;
        run_roles(ctx, &roles, args).await
    } else if args.force_all {
        let roles = config.all_roles();
        run_roles(ctx, &roles, args).await
    } else if args.bin.is_some() && args.server_id.is_some() {
        let bin = args.bin.as_ref().unwrap();
        let sid = args.server_id.unwrap();
        let bin_data = ctx.read_bin(bin)?;
        let roles = ctx
            .core
            .get_server_list(&bin_data)
            .await
            .map_err(|e| format!("Failed to get server list: {}", e))?;
        if !roles.iter().any(|role| role.server_id == sid) {
            return Err(format!(
                "server_id {} not found in this bin (use `koc_cli verify --bin ...` to inspect)",
                sid
            ));
        }
        let prefix = role_prefix(bin, sid);
        run_single(ctx, bin, sid, args, &prefix).await
    } else {
        Err("Select a mode:\n  --bin <BIN> --server-id <ID>   single role\n  --group <NAME>                 config group\n  --force-all                    all roles".into())
    }
}

async fn run_roles(
    ctx: &CliContext,
    roles: &[(String, u64)],
    args: &SkinCArgs,
) -> Result<(), String> {
    let total = roles.len();
    for (i, (bin_name, server_id)) in roles.iter().enumerate() {
        let bin_path = PathBuf::from(bin_name);
        let prefix = format!("[{}/{} {}]", i + 1, total, role_prefix(&bin_path, *server_id));
        info!(target: "cli", prefix = %prefix, "skinc: running role");
        match run_single(ctx, &bin_path, *server_id, args, &prefix).await {
            Ok(()) => info!(target: "cli", prefix = %prefix, "skinc: done"),
            Err(e) => tracing::error!(target: "cli", prefix = %prefix, error = %e, "skinc: failed"),
        }
    }
    Ok(())
}

async fn run_single(
    ctx: &CliContext,
    bin_path: &Path,
    server_id: u64,
    _args: &SkinCArgs,
    prefix: &str,
) -> Result<(), String> {
    let bin_data = ctx.read_bin(bin_path)?;
    let token = ctx
        .core
        .select_role_token(&bin_data, server_id)
        .await
        .map_err(|e| format!("Failed to get role token: {}", e))?;

    let mut game = GameClient::login(&token)
        .await
        .map_err(|e| format!("Login failed: {}", e))?;

    let res = game.run_skinc_climb(prefix).await;
    game.disconnect().await;
    res
}

fn role_prefix(bin: &Path, sid: u64) -> String {
    let bin_name = bin.file_name().unwrap_or_default().to_string_lossy();
    format!("{}@{}", bin_name, sid)
}
