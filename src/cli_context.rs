use std::fs;
use std::path::{Path, PathBuf};

use koc_core::{KocCore, RoleInfo};
use koc_core::config::{BatchConfig, FormationContext};
use koc_core::GameClient;
use tracing::info;

pub struct CliContext {
    pub core: KocCore,
    pub config_path: PathBuf,
}

impl CliContext {
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            core: KocCore::new(),
            config_path,
        }
    }

    pub fn read_bin(&self, bin_path: &Path) -> Result<Vec<u8>, String> {
        fs::read(bin_path).map_err(|e| format!("Failed to read {}: {}", bin_path.display(), e))
    }

    pub fn load_config(&self) -> Result<BatchConfig, String> {
        BatchConfig::load(&self.config_path)
    }
}

pub async fn switch_context_formation(
    config: &BatchConfig,
    game: &GameClient,
    bin_path: &Path,
    server_id: u64,
    context: FormationContext,
) -> Result<Option<u64>, String> {
    let current_team = get_current_team_id(game).await?;
    let (target_team, source) = config.resolve_formation_with_source(&bin_path.to_string_lossy(), server_id, context);
    let bin_name = bin_path
        .file_name()
        .and_then(|v| v.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| bin_path.to_string_lossy().to_string());

    info!(target: "cli", context = context.as_str(), bin = %bin_name, server_id, current_team, target_team, source, "formation resolved");

    if current_team == target_team {
        info!(target: "cli", context = context.as_str(), current = current_team, "formation unchanged");
        return Ok(None);
    }

    game.presetteam_saveteam(target_team)
        .await
        .map_err(|e| format!("Failed to switch formation for {}: {}", context.as_str(), e))?;
    info!(target: "cli", context = context.as_str(), from = current_team, to = target_team, "formation switch");
    Ok(Some(current_team))
}

pub async fn restore_context_formation(
    game: &GameClient,
    context: FormationContext,
    original_team: Option<u64>,
) -> Result<(), String> {
    let Some(original_team) = original_team else {
        return Ok(());
    };
    let current_team = get_current_team_id(game).await?;
    if current_team == original_team {
        info!(target: "cli", context = context.as_str(), current = current_team, "formation restore skipped, already original");
        return Ok(());
    }
    game.presetteam_saveteam(original_team)
        .await
        .map_err(|e| format!("Failed to restore formation for {}: {}", context.as_str(), e))?;
    info!(target: "cli", context = context.as_str(), from = current_team, to = original_team, "formation restore");
    Ok(())
}

async fn get_current_team_id(game: &GameClient) -> Result<u64, String> {
    game.current_team_id().await
}

pub fn decode_server(server_id: u64) -> (u64, u64) {
    let mut sid = server_id;
    let idx = if sid >= 2_000_000 {
        sid -= 2_000_000;
        2
    } else if sid >= 1_000_000 {
        sid -= 1_000_000;
        1
    } else {
        0
    };
    (sid.saturating_sub(27), idx)
}

pub fn print_roles(roles: &[RoleInfo]) {
    println!("Found {} roles:", roles.len());
    for (i, role) in roles.iter().enumerate() {
        let (server_no, idx) = decode_server(role.server_id);
        println!(
            "  [{}] {}-{} name={} power={} level={} serverId={}",
            i + 1,
            server_no,
            idx,
            role.name,
            role.power,
            role.level,
            role.server_id
        );
    }
}
