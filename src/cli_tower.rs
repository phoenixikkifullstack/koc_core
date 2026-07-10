use std::path::PathBuf;
use std::time::Duration;

use crate::cli_context::{CliContext, restore_context_formation, switch_context_formation};
use koc_core::config::FormationContext;
use tracing::{info, warn};

#[derive(clap::Args, Debug, Clone)]
pub struct TowerArgs {
    #[arg(long, value_name = "BIN")]
    pub bin: PathBuf,

    #[arg(long, value_name = "SERVER_ID")]
    pub server_id: u64,

    #[arg(long, default_value_t = 100)]
    pub max_climb: u32,

    #[arg(long, default_value_t = 1000)]
    pub interval_ms: u64,

    #[arg(long, default_value_t = 5)]
    pub refresh_every: u32,

    #[arg(long, default_value_t = false)]
    pub no_auto_claim: bool,
}

pub async fn run(ctx: &CliContext, args: &TowerArgs) -> Result<(), String> {
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

    let config = ctx.load_config()?;
    let original_team = switch_context_formation(&config, &game, &args.bin, args.server_id, FormationContext::Tower).await?;

    let result = run_tower_flow(&mut game, args).await;
    let restore_result = restore_context_formation(&game, FormationContext::Tower, original_team).await;
    game.disconnect().await;
    result?;
    restore_result?;
    Ok(())
}

async fn run_tower_flow(game: &mut koc_core::GameClient, args: &TowerArgs) -> Result<(), String> {
    let refresh_every = args.refresh_every.max(1);
    let mut climb_count = 0u32;
    let mut fail_count = 0u32;
    let mut skip_count = 0u32;
    let mut battle_win_count = 0u32;
    let mut battle_lose_count = 0u32;
    let mut battle_unknown_count = 0u32;
    let mut consecutive_failures = 0u32;
    let mut end_reason = String::from("max climb reached");

    let _ = game.tower_getinfo().await;
    refresh_role_info(game).await?;

    let mut energy = get_tower_energy(game.role_info.as_ref()).unwrap_or(0);
    let mut tower_id = get_tower_id(game.role_info.as_ref());
    info!(target: "cli", energy, tower_id = ?tower_id, "[Tower] Start");

    if energy <= 0 {
        info!(target: "cli", "[~~] No tower energy, nothing to do");
        return Ok(());
    }

    while climb_count < args.max_climb {
        if energy <= 0 {
            end_reason = "energy exhausted".to_string();
            break;
        }

        match game.fight_starttower().await {
            Ok(resp) => {
                climb_count += 1;
                consecutive_failures = 0;
                if energy > 0 {
                    energy -= 1;
                }

                match parse_tower_battle_result(&resp) {
                    Some((true, tower_id_from_resp)) => {
                        battle_win_count += 1;
                        tower_id = tower_id_from_resp.or(tower_id);
                        info!(target: "cli", climb_count, max = args.max_climb, tower_id = ?tower_id, "[OK][WIN] fight_starttower");
                    }
                    Some((false, tower_id_from_resp)) => {
                        battle_lose_count += 1;
                        tower_id = tower_id_from_resp.or(tower_id);
                        info!(target: "cli", climb_count, max = args.max_climb, tower_id = ?tower_id, "[OK][LOSE] fight_starttower");
                    }
                    None => {
                        battle_unknown_count += 1;
                        info!(target: "cli", climb_count, max = args.max_climb, "[OK] fight_starttower (battle result unknown)");
                    }
                }

                if climb_count % refresh_every == 0 {
                    refresh_role_info(game).await?;
                    energy = get_tower_energy(game.role_info.as_ref()).unwrap_or(energy);
                    tower_id = get_tower_id(game.role_info.as_ref()).or(tower_id);
                    info!(target: "cli", energy, tower_id = ?tower_id, "[Tower] Refreshed");
                }

                tokio::time::sleep(Duration::from_millis(args.interval_ms)).await;
            }
            Err(err) => {
                let code = koc_core::error_codes::extract_code_from_error(&err);
                match code {
                    Some(1500020) => {
                        skip_count += 1;
                        end_reason = "energy exhausted (1500020)".to_string();
                        info!(target: "cli", error = %err, "[~~] tower exhausted");
                        break;
                    }
                    Some(1500010) => {
                        skip_count += 1;
                        end_reason = "tower fully cleared (1500010)".to_string();
                        info!(target: "cli", error = %err, "[~~] tower fully cleared");
                        break;
                    }
                    Some(1500040) if !args.no_auto_claim => {
                        skip_count += 1;
                        info!(target: "cli", error = %err, "[~~] reward pending before climb");

                        refresh_role_info(game).await?;
                        energy = get_tower_energy(game.role_info.as_ref()).unwrap_or(energy);
                        tower_id = get_tower_id(game.role_info.as_ref()).or(tower_id);

                        if let Some(id) = tower_id {
                            let reward_floor = id / 10;
                            if reward_floor > 0 {
                                if should_claim_reward(game.role_info.as_ref(), reward_floor) {
                                    match game.tower_claimreward(reward_floor).await {
                                        Ok(_) => {
                                            info!(target: "cli", reward_floor, "[OK] tower_claimreward");
                                        }
                                        Err(e) => {
                                            if let Some(claim_code) = koc_core::error_codes::extract_code_from_error(&e) {
                                                if koc_core::error_codes::is_done_error(claim_code) {
                                                    info!(target: "cli", reward_floor, error = %e, "[~~] tower_claimreward");
                                                } else {
                                                    warn!(target: "cli", reward_floor, error = %e, "[X] tower_claimreward");
                                                    fail_count += 1;
                                                }
                                            } else {
                                                warn!(target: "cli", reward_floor, error = %e, "[X] tower_claimreward");
                                                fail_count += 1;
                                            }
                                        }
                                    }
                                } else {
                                    info!(target: "cli", reward_floor, "[~~] reward floor already claimed");
                                }
                            }
                        }

                        tokio::time::sleep(Duration::from_millis(1500)).await;
                        refresh_role_info(game).await?;
                        energy = get_tower_energy(game.role_info.as_ref()).unwrap_or(energy);
                        tower_id = get_tower_id(game.role_info.as_ref()).or(tower_id);
                        consecutive_failures = 0;
                    }
                    Some(200400) => {
                        skip_count += 1;
                        info!(target: "cli", error = %err, "[~~] backoff 5s");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                    _ => {
                        fail_count += 1;
                        consecutive_failures += 1;
                        warn!(target: "cli", consecutive_failures, limit = 3, error = %err, "[X] fight_starttower failed");

                        if consecutive_failures >= 3 {
                            end_reason = "too many consecutive failures".to_string();
                            break;
                        }

                        tokio::time::sleep(Duration::from_secs(2)).await;
                        let _ = refresh_role_info(game).await;
                        energy = get_tower_energy(game.role_info.as_ref()).unwrap_or(energy);
                        tower_id = get_tower_id(game.role_info.as_ref()).or(tower_id);
                    }
                }
            }
        }
    }

    info!(
        target: "cli",
        climbs = climb_count,
        win = battle_win_count,
        lose = battle_lose_count,
        unknown = battle_unknown_count,
        skipped = skip_count,
        failed = fail_count,
        end_reason = %end_reason,
        final_energy = energy,
        final_tower_id = ?tower_id,
        "[Tower] Done"
    );
    Ok(())
}

fn parse_tower_battle_result(resp: &serde_json::Value) -> Option<(bool, Option<u64>)> {
    let battle_data = resp.get("battleData")?;
    let cur_hp = battle_data
        .pointer("/result/sponsor/ext/curHP")
        .and_then(|v| v.as_f64())?;
    let tower_id = battle_data
        .pointer("/options/towerId")
        .and_then(|v| v.as_u64());
    Some((cur_hp > 0.0, tower_id))
}

async fn refresh_role_info(game: &mut koc_core::GameClient) -> Result<(), String> {
    let info = game.role_getroleinfo().await?;
    game.role_info = Some(info);
    Ok(())
}

fn get_tower_energy(role_info: Option<&serde_json::Value>) -> Option<i64> {
    role_info
        .and_then(|v| v.pointer("/role/tower/energy").or_else(|| v.pointer("/tower/energy")))
        .and_then(|v| v.as_i64())
}

fn get_tower_id(role_info: Option<&serde_json::Value>) -> Option<u64> {
    role_info
        .and_then(|v| v.pointer("/role/tower/id").or_else(|| v.pointer("/tower/id")))
        .and_then(|v| v.as_u64())
}

fn should_claim_reward(role_info: Option<&serde_json::Value>, reward_floor: u64) -> bool {
    let reward = role_info
        .and_then(|v| v.pointer("/role/tower/reward").or_else(|| v.pointer("/tower/reward")));

    match reward {
        None => true,
        Some(serde_json::Value::Object(map)) => {
            if let Some(v) = map.get(&reward_floor.to_string()) {
                !v.as_bool().unwrap_or(false)
            } else {
                true
            }
        }
        Some(serde_json::Value::Array(arr)) => arr
            .get(reward_floor as usize)
            .and_then(|v| v.as_bool())
            .map(|claimed| !claimed)
            .unwrap_or(true),
        Some(_) => true,
    }
}
