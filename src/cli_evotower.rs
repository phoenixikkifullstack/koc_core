use std::path::PathBuf;
use std::time::Duration;
use std::collections::HashSet;

use chrono::Local;
use serde_json::json;
use tracing::{info, warn};

use crate::cli_context::{CliContext, restore_context_formation, switch_context_formation};
use koc_core::config::FormationContext;

#[derive(clap::Args, Debug, Clone)]
pub struct EvoTowerArgs {
    #[arg(long, value_name = "BIN")]
    pub bin: PathBuf,

    #[arg(long, value_name = "SERVER_ID")]
    pub server_id: u64,

    #[arg(long, default_value_t = 100)]
    pub max_climb: u32,

    #[arg(long, default_value_t = 1000)]
    pub interval_ms: u64,

    #[arg(long, default_value_t = 3)]
    pub refresh_every: u32,

    #[arg(long, default_value_t = 3)]
    pub failure_limit: u32,

    #[arg(long, default_value_t = false)]
    pub no_auto_claim_task: bool,

    #[arg(long, default_value_t = false)]
    pub no_auto_claim_reward: bool,

    #[arg(long, default_value_t = 20)]
    pub merge_max_loops: u32,

    #[arg(long, default_value_t = 500)]
    pub merge_delay_ms: u64,
}

pub async fn run(ctx: &CliContext, args: &EvoTowerArgs) -> Result<(), String> {
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
    let original_team = switch_context_formation(&config, &game, &args.bin, args.server_id, FormationContext::Evotower).await?;

    let result = run_evotower_flow(&mut game, args).await;
    let restore_result = restore_context_formation(&game, FormationContext::Evotower, original_team).await;
    game.disconnect().await;
    result?;
    restore_result?;
    Ok(())
}

async fn run_evotower_flow(game: &mut koc_core::GameClient, args: &EvoTowerArgs) -> Result<(), String> {
    let refresh_every = args.refresh_every.max(1);
    let failure_limit = args.failure_limit.max(1);
    let mut climb_count = 0u32;
    let mut fail_count = 0u32;
    let mut skip_count = 0u32;
    let mut win_count = 0u32;
    let mut lose_count = 0u32;
    let mut unknown_count = 0u32;
    let mut consecutive_failures = 0u32;
    let mut end_reason = String::from("max climb reached");

    let mut evo_info = match game.evotower_getinfo().await {
        Ok(v) => v,
        Err(e) => {
            if is_evotower_not_open_error(&e) {
                info!(target: "cli", error = %e, "[~~] EvoTower is not open now");
                return Ok(());
            }
            return Err(format!("Failed to get evotower info: {}", e));
        }
    };

    let mut energy = get_evo_energy(&evo_info).unwrap_or(0);
    let mut tower_id = get_evo_tower_id(&evo_info);
    let date_key = Local::now().format("%y%m%d").to_string();
    let mut claimed_task_ids = collect_claimed_evotower_tasks(&evo_info, &date_key);
    info!(target: "cli", energy, tower_id = ?tower_id, "[EvoTower] Start");

    let _ = recover_evotower_pending_reward(
        game,
        args,
        &mut evo_info,
        &mut energy,
        &mut tower_id,
        &date_key,
        &mut claimed_task_ids,
        &mut skip_count,
        &mut consecutive_failures,
    ).await;

    if energy <= 0 {
        info!(target: "cli", "[~~] No evotower energy, skip climb and run merge once");
        run_merge_once(game, args).await;
        return Ok(());
    }

    while climb_count < args.max_climb {
        if energy <= 0 {
            end_reason = "energy exhausted".to_string();
            break;
        }

        let ready_res = game.evotower_readyfight(json!({})).await;
        if let Err(e) = ready_res {
            if koc_core::error_codes::extract_code_from_error(&e) == Some(12200020) {
                if recover_evotower_pending_reward(
                    game,
                    args,
                    &mut evo_info,
                    &mut energy,
                    &mut tower_id,
                    &date_key,
                    &mut claimed_task_ids,
                    &mut skip_count,
                    &mut consecutive_failures,
                ).await {
                    continue;
                }
            }
            if is_evotower_terminal_error(&e) {
                skip_count += 1;
                end_reason = terminal_reason_from_error(&e);
                info!(target: "cli", error = %e, "[~~] terminal");
                break;
            }
            fail_count += 1;
            consecutive_failures += 1;
            warn!(target: "cli", consecutive_failures, failure_limit, error = %e, "[X] evotower_readyfight failed");
            if consecutive_failures >= failure_limit {
                end_reason = format!("too many consecutive failures (>= {})", failure_limit);
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }

        match game
            .evotower_fight(json!({"battleNum": 1, "winNum": 1}))
            .await
        {
            Ok(fight_resp) => {
                climb_count += 1;
                consecutive_failures = 0;

                match parse_evotower_fight_result(&fight_resp) {
                    Some(true) => {
                        win_count += 1;
                        info!(target: "cli", climb_count, max = args.max_climb, "[OK][WIN] evotower_fight");
                    }
                    Some(false) => {
                        lose_count += 1;
                        info!(target: "cli", climb_count, max = args.max_climb, "[OK][LOSE] evotower_fight");
                    }
                    None => {
                        unknown_count += 1;
                        info!(target: "cli", climb_count, max = args.max_climb, "[OK] evotower_fight (result unknown)");
                    }
                }

                if climb_count % refresh_every == 0 {
                    evo_info = match game.evotower_getinfo().await {
                        Ok(v) => v,
                        Err(e) => {
                            if is_evotower_terminal_error(&e) {
                                skip_count += 1;
                                end_reason = terminal_reason_from_error(&e);
                                info!(target: "cli", error = %e, "[~~] terminal");
                                break;
                            }
                            fail_count += 1;
                            warn!(target: "cli", error = %e, "[X] evotower_getinfo refresh failed");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                    };

                    energy = get_evo_energy(&evo_info).unwrap_or(energy);
                    tower_id = get_evo_tower_id(&evo_info).or(tower_id);
                    info!(target: "cli", energy, tower_id = ?tower_id, "[EvoTower] Refreshed");

                    if !args.no_auto_claim_task && claimed_task_ids.len() < 3 {
                        claimed_task_ids.extend(collect_claimed_evotower_tasks(&evo_info, &date_key));
                        if let Some(task_id) = claimtask_id_for_climb(climb_count) {
                            if !claimed_task_ids.contains(&task_id)
                                && should_claim_evotower_task(&evo_info, &date_key, task_id)
                            {
                                match game.ws.send_with_response("evotower_claimtask", Some(json!({"taskId": task_id})), 2000).await {
                                    Ok(_) => {
                                        claimed_task_ids.insert(task_id);
                                        info!(target: "cli", task_id, climb_count, "[OK] evotower_claimtask");
                                    }
                                    Err(e) => {
                                        if let Some(code) = koc_core::error_codes::extract_code_from_error(&e) {
                                            if code == 12200050 {
                                                claimed_task_ids.insert(task_id);
                                                info!(target: "cli", task_id, climb_count, error = %e, "[~~] evotower_claimtask");
                                            } else if code == 12200040 || koc_core::error_codes::is_done_error(code) {
                                                info!(target: "cli", task_id, climb_count, error = %e, "[~~] evotower_claimtask");
                                            } else {
                                                warn!(target: "cli", task_id, climb_count, error = %e, "[X] evotower_claimtask");
                                            }
                                        } else {
                                            warn!(target: "cli", task_id, climb_count, error = %e, "[X] evotower_claimtask");
                                        }
                                    }
                                }
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                        }
                    }

                    if !args.no_auto_claim_reward {
                        let floor = tower_id.map(|id| (id % 10) + 1).unwrap_or(0);
                        let just_won = parse_evotower_fight_result(&fight_resp) == Some(true);
                        if just_won && floor == 1 {
                            match game.evotower_claimreward(json!({})).await {
                                Ok(_) => {
                                    let chapter = tower_id.map(|id| id / 10).unwrap_or(0);
                                    info!(target: "cli", chapter, "[OK] evotower_claimreward");
                                }
                                Err(e) => {
                                    if let Some(code) = koc_core::error_codes::extract_code_from_error(&e) {
                                        if koc_core::error_codes::is_done_error(code) {
                                            info!(target: "cli", error = %e, "[~~] evotower_claimreward");
                                        } else {
                                            warn!(target: "cli", error = %e, "[X] evotower_claimreward");
                                        }
                                    } else {
                                        warn!(target: "cli", error = %e, "[X] evotower_claimreward");
                                    }
                                }
                            }
                        }
                    }
                }

                tokio::time::sleep(Duration::from_millis(args.interval_ms)).await;
            }
            Err(e) => {
                if is_evotower_terminal_error(&e) {
                    skip_count += 1;
                    end_reason = terminal_reason_from_error(&e);
                    info!(target: "cli", error = %e, "[~~] terminal");
                    break;
                }
                fail_count += 1;
                consecutive_failures += 1;
                warn!(target: "cli", consecutive_failures, failure_limit, error = %e, "[X] evotower_fight failed");

                if consecutive_failures >= failure_limit {
                    end_reason = format!("too many consecutive failures (>= {})", failure_limit);
                    break;
                }

                if koc_core::error_codes::extract_code_from_error(&e) == Some(200400) {
                    skip_count += 1;
                    tokio::time::sleep(Duration::from_secs(3)).await;
                } else {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    info!(
        target: "cli",
        climbs = climb_count,
        win = win_count,
        lose = lose_count,
        unknown = unknown_count,
        skipped = skip_count,
        failed = fail_count,
        end_reason = %end_reason,
        final_energy = energy,
        final_tower_id = ?tower_id,
        "[EvoTower] Done"
    );

    run_merge_once(game, args).await;
    Ok(())
}

async fn run_merge_once(game: &koc_core::GameClient, args: &EvoTowerArgs) {
    info!(target: "cli", max_loops = args.merge_max_loops, delay_ms = args.merge_delay_ms, "[Merge] Start");

    let mut total_claim_rewards = 0u32;
    let mut total_auto_merges = 0u32;
    let mut total_manual_merges = 0u32;
    let mut end_reason = String::from("max loops reached");

    for loop_idx in 1..=args.merge_max_loops.max(1) {
        let info = match game.mergebox_getinfo(1).await {
            Ok(v) => v,
            Err(e) => {
                if is_evotower_not_open_error(&e) {
                    end_reason = "mergebox not open".to_string();
                    info!(target: "cli", error = %e, "[~~] mergebox_getinfo");
                } else {
                    end_reason = format!("mergebox_getinfo failed: {}", e);
                    warn!(target: "cli", error = %e, "[X] mergebox_getinfo");
                }
                break;
            }
        };

        let merge_box = match info.get("mergeBox") {
            Some(v) => v,
            None => {
                end_reason = "mergeBox field missing".to_string();
                warn!(target: "cli", "[X] mergebox_getinfo response missing mergeBox");
                break;
            }
        };

        let claimed = claim_merge_progress_rewards(game, merge_box).await;
        total_claim_rewards += claimed;

        let grouped = collect_merge_groups(merge_box);
        if grouped.is_empty() {
                end_reason = "no mergeable items".to_string();
                if loop_idx == 1 {
                    info!(target: "cli", "[~~] No mergeable items");
                }
                break;
            }

        if is_merge_level8_or_above(merge_box) {
            match game.mergebox_automergeitem(1).await {
                Ok(_) => {
                    total_auto_merges += 1;
                    info!(target: "cli", loop_idx, "[OK] mergebox_automergeitem");
                }
                Err(e) => {
                    warn!(target: "cli", loop_idx, error = %e, "[X] mergebox_automergeitem");
                    end_reason = "automerge failed".to_string();
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(args.merge_delay_ms.max(100))).await;
        } else {
            let mut loop_manual_merges = 0u32;
            for (_, mut items) in grouped {
                while items.len() >= 2 {
                    let source = items.pop().expect("source exists");
                    let target = items.pop().expect("target exists");
                    let params = json!({
                        "actType": 1,
                        "sourcePos": {"gridX": source.0, "gridY": source.1},
                        "targetPos": {"gridX": target.0, "gridY": target.1}
                    });

                    match game.mergebox_mergeitem(params).await {
                        Ok(_) => {
                            loop_manual_merges += 1;
                            total_manual_merges += 1;
                        }
                        Err(e) => {
                            warn!(target: "cli", error = %e, "[X] mergebox_mergeitem");
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }

            if loop_manual_merges == 0 {
                end_reason = "no manual merge executed".to_string();
                break;
            }
            info!(target: "cli", loop_idx, merged = loop_manual_merges, "[OK] mergebox_mergeitem");
        }

        tokio::time::sleep(Duration::from_millis(args.merge_delay_ms)).await;
    }

    match game.mergebox_claimcostprogress(1).await {
        Ok(_) => info!(target: "cli", "[OK] mergebox_claimcostprogress"),
        Err(e) => {
            if let Some(code) = koc_core::error_codes::extract_code_from_error(&e) {
                if koc_core::error_codes::is_done_error(code) {
                    info!(target: "cli", error = %e, "[~~] mergebox_claimcostprogress");
                } else {
                    warn!(target: "cli", error = %e, "[X] mergebox_claimcostprogress");
                }
            } else {
                warn!(target: "cli", error = %e, "[X] mergebox_claimcostprogress");
            }
        }
    }

    info!(target: "cli", claim_rewards = total_claim_rewards, auto_merges = total_auto_merges, manual_merges = total_manual_merges, end_reason = %end_reason, "[Merge] Done");
}

async fn claim_merge_progress_rewards(game: &koc_core::GameClient, merge_box: &serde_json::Value) -> u32 {
    let task_map = match merge_box.get("taskMap").and_then(|v| v.as_object()) {
        Some(v) => v,
        None => return 0,
    };
    let task_claim_map = merge_box
        .get("taskClaimMap")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut claimed = 0u32;
    for (task_id_str, task_val) in task_map {
        let ready = task_val.as_i64().unwrap_or(0) != 0;
        let already_claimed = task_claim_map
            .get(task_id_str)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !ready || already_claimed {
            continue;
        }

        let task_id: u64 = match task_id_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };

        match game.mergebox_claimmergeprogress(1, task_id).await {
            Ok(_) => {
                claimed += 1;
                info!(target: "cli", task_id, "[OK] mergebox_claimmergeprogress");
            }
            Err(e) => {
                if let Some(code) = koc_core::error_codes::extract_code_from_error(&e) {
                    if koc_core::error_codes::is_done_error(code) {
                        info!(target: "cli", task_id, error = %e, "[~~] mergebox_claimmergeprogress");
                    } else {
                        warn!(target: "cli", task_id, error = %e, "[X] mergebox_claimmergeprogress");
                    }
                } else {
                    warn!(target: "cli", task_id, error = %e, "[X] mergebox_claimmergeprogress");
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    claimed
}

fn collect_merge_groups(merge_box: &serde_json::Value) -> std::collections::HashMap<u64, Vec<(u64, u64)>> {
    let mut grouped: std::collections::HashMap<u64, Vec<(u64, u64)>> = std::collections::HashMap::new();
    let grid_map = match merge_box.get("gridMap").and_then(|v| v.as_object()) {
        Some(v) => v,
        None => return grouped,
    };

    for (x_str, col) in grid_map {
        let x: u64 = match x_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let col_obj = match col.as_object() {
            Some(v) => v,
            None => continue,
        };

        for (y_str, cell) in col_obj {
            let y: u64 = match y_str.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let grid_conf_id = cell.get("gridConfId").and_then(|v| v.as_i64()).unwrap_or(-1);
            let grid_item_id = cell.get("gridItemId").and_then(|v| v.as_u64()).unwrap_or(0);
            let is_lock = cell.get("isLock").and_then(|v| v.as_bool()).unwrap_or(false);

            if grid_conf_id == 0 && grid_item_id > 0 && !is_lock {
                grouped.entry(grid_item_id).or_default().push((x, y));
            }
        }
    }

    grouped.retain(|_, v| v.len() >= 2);
    grouped
}

fn is_merge_level8_or_above(merge_box: &serde_json::Value) -> bool {
    merge_box
        .pointer("/taskMap/251212208")
        .and_then(|v| v.as_i64())
        .map(|v| v != 0)
        .unwrap_or(false)
}

fn get_evo_energy(info: &serde_json::Value) -> Option<i64> {
    info.pointer("/evoTower/energy")
        .or_else(|| info.pointer("/energy"))
        .and_then(|v| v.as_i64())
}

fn get_evo_tower_id(info: &serde_json::Value) -> Option<u64> {
    info.pointer("/evoTower/towerId")
        .or_else(|| info.pointer("/towerId"))
        .and_then(|v| v.as_u64())
}

fn parse_evotower_fight_result(resp: &serde_json::Value) -> Option<bool> {
    resp.pointer("/winList/0")
        .or_else(|| resp.pointer("/battleData/winList/0"))
        .and_then(|v| v.as_bool())
}

fn should_claim_evotower_task(info: &serde_json::Value, date_key: &str, task_id: u64) -> bool {
    let map = match info.pointer("/evoTower/taskClaimMap") {
        Some(v) => v,
        None => return false,
    };

    let daily = match map.get(date_key) {
        Some(v) => v,
        None => return true,
    };

    let key = task_id.to_string();
    match daily.get(&key).or_else(|| daily.get(task_id as usize)) {
        Some(v) => !v.as_bool().unwrap_or(false),
        None => true,
    }
}

async fn recover_evotower_pending_reward(
    game: &mut koc_core::GameClient,
    args: &EvoTowerArgs,
    evo_info: &mut serde_json::Value,
    energy: &mut i64,
    tower_id: &mut Option<u64>,
    date_key: &str,
    claimed_task_ids: &mut HashSet<u64>,
    skip_count: &mut u32,
    consecutive_failures: &mut u32,
) -> bool {
    if let Ok(refreshed) = game.evotower_getinfo().await {
        *evo_info = refreshed;
        *energy = get_evo_energy(evo_info).unwrap_or(*energy);
        *tower_id = get_evo_tower_id(evo_info).or(*tower_id);
        claimed_task_ids.extend(collect_claimed_evotower_tasks(evo_info, date_key));
        info!(target: "cli", energy = *energy, tower_id = ?*tower_id, "[EvoTower] Refreshed before pending-reward recovery");
    }

    if args.no_auto_claim_reward {
        return false;
    }

    let Some(current_tower_id) = *tower_id else {
        return false;
    };
    let chapter = current_tower_id / 10;
    if chapter == 0 || current_tower_id % 10 != 0 {
        return false;
    }

    match game.evotower_claimreward(json!({})).await {
        Ok(_) => {
            *skip_count += 1;
            *consecutive_failures = 0;
            info!(target: "cli", chapter, tower_id = current_tower_id, "[OK] evotower_claimreward recovery");
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Ok(refreshed) = game.evotower_getinfo().await {
                *evo_info = refreshed;
                *energy = get_evo_energy(evo_info).unwrap_or(*energy);
                *tower_id = get_evo_tower_id(evo_info).or(*tower_id);
                claimed_task_ids.extend(collect_claimed_evotower_tasks(evo_info, date_key));
                info!(target: "cli", energy = *energy, tower_id = ?*tower_id, "[EvoTower] Refreshed after pending-reward recovery");
            }
            true
        }
        Err(e) => {
            warn!(target: "cli", chapter, tower_id = current_tower_id, error = %e, "[X] evotower_claimreward recovery failed");
            false
        }
    }
}

fn collect_claimed_evotower_tasks(info: &serde_json::Value, date_key: &str) -> HashSet<u64> {
    let mut claimed = HashSet::new();
    let map = match info.pointer("/evoTower/taskClaimMap") {
        Some(v) => v,
        None => return claimed,
    };

    let daily = match map.get(date_key) {
        Some(v) => v,
        None => return claimed,
    };

    for task_id in [1u64, 2, 3] {
        let key = task_id.to_string();
        let done = daily
            .get(&key)
            .or_else(|| daily.get(task_id as usize))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if done {
            claimed.insert(task_id);
        }
    }

    claimed
}

fn claimtask_id_for_climb(climb_count: u32) -> Option<u64> {
    match climb_count {
        3 => Some(1),
        6 => Some(2),
        10 => Some(3),
        _ => None,
    }
}

fn is_evotower_not_open_error(err: &str) -> bool {
    matches!(koc_core::error_codes::extract_code_from_error(err), Some(200160) | Some(2100010))
}

fn is_evotower_terminal_error(err: &str) -> bool {
    if is_evotower_not_open_error(err) {
        return true;
    }

    if let Some(code) = koc_core::error_codes::extract_code_from_error(err) {
        if koc_core::error_codes::is_done_error(code) {
            return true;
        }
    }

    err.contains("能量不足") || err.contains("已全部通关") || err.contains("已经全部通关")
}

fn terminal_reason_from_error(err: &str) -> String {
    if let Some(code) = koc_core::error_codes::extract_code_from_error(err) {
        format!("terminal error code {}", code)
    } else {
        "terminal error".to_string()
    }
}
