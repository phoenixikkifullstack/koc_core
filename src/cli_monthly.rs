use std::path::PathBuf;
use std::time::Duration;

use chrono::{Datelike, Local, Timelike};
use tracing::{info, warn};

use crate::cli_context::{CliContext, restore_context_formation, switch_context_formation};
use koc_core::config::{BatchConfig, FormationContext};

const FISH_TARGET: u32 = 320;
const ARENA_TARGET: u32 = 240;

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonthlyMode {
    Fish,
    Arena,
    All,
}

#[derive(clap::Args, Debug, Clone)]
pub struct MonthlyArgs {
    #[arg(long, value_name = "BIN")]
    pub bin: PathBuf,

    #[arg(long, value_name = "SERVER_ID")]
    pub server_id: u64,

    #[arg(long, value_enum, default_value_t = MonthlyMode::All)]
    pub mode: MonthlyMode,

    #[arg(long, default_value_t = false)]
    pub topup: bool,

    #[arg(long, default_value_t = false)]
    pub complete: bool,

    #[arg(long, default_value_t = 100)]
    pub arena_safety_max: u32,

    #[arg(long, default_value_t = 10)]
    pub fish_batch_size: u32,

    #[arg(long, default_value_t = false)]
    pub no_claim_fish_point: bool,
}

struct MonthlyQueryResult {
    year: i32,
    month: u32,
    day_of_month: u32,
    days_in_month: u32,
    progress_ratio: f64,
    fish_tickets: u32,
    arena_tickets: u32,
    fish: MonthlyProgressLine,
    arena: MonthlyProgressLine,
}

struct MonthlyProgressLine {
    current: u32,
    progress_target: u32,
    full_target: u32,
    need: u32,
    full_need: u32,
}

pub async fn run(ctx: &CliContext, args: &MonthlyArgs) -> Result<(), String> {
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

    let result = run_monthly_flow(ctx, &mut game, args).await;
    game.disconnect().await;
    result
}

async fn run_monthly_flow(
    ctx: &CliContext,
    game: &mut koc_core::GameClient,
    args: &MonthlyArgs,
) -> Result<(), String> {
    let query = query_monthly_progress(game, args.complete).await?;
    if !args.topup {
        print_monthly_query(&query, args.mode);
        return Ok(());
    }

    let mut errors = Vec::new();

    if matches!(args.mode, MonthlyMode::Fish | MonthlyMode::All) {
        if let Err(e) = run_monthly_fish_topup(game, &query, args).await {
            errors.push(format!("fish: {}", e));
        }
    }

    if matches!(args.mode, MonthlyMode::Arena | MonthlyMode::All) {
        let config = ctx.load_config()?;
        if let Err(e) = run_monthly_arena_topup(&config, game, &args.bin, args.server_id, &query, args).await {
            errors.push(format!("arena: {}", e));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

async fn query_monthly_progress(game: &mut koc_core::GameClient, complete: bool) -> Result<MonthlyQueryResult, String> {
    let now = Local::now();
    let act = game.activity_get().await?;
    let activity = act.get("activity").unwrap_or(&act);
    let role_info = game.role_getroleinfo().await?;
    game.role_info = Some(role_info.clone());
    let fish_current = activity
        .pointer("/myMonthInfo/2/num")
        .or_else(|| activity.pointer("/myMonthInfo/"))
        .and_then(|_| activity.get("myMonthInfo"))
        .and_then(|v| v.get("2"))
        .and_then(|v| v.get("num"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let arena_current = activity
        .pointer("/myArenaInfo/num")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let fish_tickets = role_info
        .pointer("/role/items/1011/quantity")
        .or_else(|| role_info.pointer("/items/1011/quantity"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let arena_tickets = role_info
        .pointer("/role/items/1007/quantity")
        .or_else(|| role_info.pointer("/items/1007/quantity"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let (day_of_month, days_in_month, progress_ratio) = calculate_month_progress(now);
    Ok(MonthlyQueryResult {
        year: now.year(),
        month: now.month(),
        day_of_month,
        days_in_month,
        progress_ratio,
        fish_tickets,
        arena_tickets,
        fish: build_progress_line(fish_current, FISH_TARGET, progress_ratio, complete, day_of_month == days_in_month),
        arena: build_progress_line(arena_current, ARENA_TARGET, progress_ratio, complete, day_of_month == days_in_month),
    })
}

fn calculate_month_progress(now: chrono::DateTime<Local>) -> (u32, u32, f64) {
    let day_of_month = now.day();
    let days_in_month = last_day_of_month(now.year(), now.month());
    let ratio = (day_of_month as f64 / days_in_month as f64).clamp(0.0, 1.0);
    (day_of_month, days_in_month, ratio)
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    let first_next = if month == 12 {
        chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1).expect("valid next year date")
    } else {
        chrono::NaiveDate::from_ymd_opt(year, month + 1, 1).expect("valid next month date")
    };
    first_next.pred_opt().expect("valid previous day").day()
}

fn build_progress_line(current: u32, full_target: u32, ratio: f64, complete: bool, is_last_day: bool) -> MonthlyProgressLine {
    let progress_target = if complete || is_last_day {
        full_target
    } else {
        ((ratio * full_target as f64).ceil() as u32).clamp(0, full_target)
    };
    MonthlyProgressLine {
        current,
        progress_target,
        full_target,
        need: progress_target.saturating_sub(current),
        full_need: full_target.saturating_sub(current),
    }
}

fn print_monthly_query(query: &MonthlyQueryResult, mode: MonthlyMode) {
    println!(
        "[Monthly] date={:04}-{:02}-{:02} day={}/{} progress={:.2}",
        query.year,
        query.month,
        query.day_of_month,
        query.day_of_month,
        query.days_in_month,
        query.progress_ratio
    );

    if matches!(mode, MonthlyMode::Fish | MonthlyMode::All) {
        println!(
            "[Fish]  current={} progress_target={} full_target={} need={} full_need={} tickets={}",
            query.fish.current,
            query.fish.progress_target,
            query.fish.full_target,
            query.fish.need,
            query.fish.full_need,
            query.fish_tickets,
        );
    }

    if matches!(mode, MonthlyMode::Arena | MonthlyMode::All) {
        println!(
            "[Arena] current={} progress_target={} full_target={} need={} full_need={} tickets={}",
            query.arena.current,
            query.arena.progress_target,
            query.arena.full_target,
            query.arena.need,
            query.arena.full_need,
            query.arena_tickets,
        );
    }
}

async fn run_monthly_fish_topup(
    game: &mut koc_core::GameClient,
    query: &MonthlyQueryResult,
    args: &MonthlyArgs,
) -> Result<(), String> {
    let target = if args.complete { query.fish.full_target } else { query.fish.progress_target };
    let mut remaining = if args.complete { query.fish.full_need } else { query.fish.need };
    info!(target: "cli", current = query.fish.current, target, remaining, complete = args.complete, "monthly fish target evaluated");
    if remaining == 0 {
        info!(target: "cli", "monthly fish already on target, skip topup");
        return Ok(());
    }

    let role_info = game.role_getroleinfo().await?;
    game.role_info = Some(role_info.clone());
    let last_free_time = role_info
        .pointer("/role/statisticsTime/artifact:normal:lottery:time")
        .or_else(|| role_info.pointer("/statisticsTime/artifact:normal:lottery:time"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let mut free_used = 0u32;
    if is_today_available(last_free_time) {
        info!(target: "cli", "monthly fish free chances available, consuming up to 3");
        for _ in 0..3 {
            if remaining <= free_used { break; }
            match game.artifact_lottery().await {
                Ok(_) => {
                    free_used += 1;
                    info!(target: "cli", used = free_used, "monthly fish free lottery ok");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(e) => {
                    warn!(target: "cli", error = %e, "monthly fish free lottery failed");
                    break;
                }
            }
        }
    }

    let refreshed = query_monthly_progress(game, args.complete).await?;
    remaining = if args.complete { refreshed.fish.full_need } else { refreshed.fish.need };
    info!(target: "cli", current = refreshed.fish.current, target, remaining, free_used, "monthly fish after free chances");
    if remaining == 0 {
        info!(target: "cli", "monthly fish target reached after free chances");
        return Ok(());
    }

    let role_info = game.role_getroleinfo().await?;
    let mut rod_count = role_info
        .pointer("/role/items/1011/quantity")
        .or_else(|| role_info.pointer("/items/1011/quantity"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    info!(target: "cli", rod_count, "monthly fish rods available");
    remaining = remaining.min(rod_count);
    if remaining == 0 {
        info!(target: "cli", "monthly fish no rods available, skip paid topup");
        return Ok(());
    }

    let mut total_paid_fish = 0u32;
    let mut stopped_early = false;
    while remaining > 0 {
        let batch = remaining.min(args.fish_batch_size.max(1));
        match game.ws.send_with_response(
            "artifact_lottery",
            Some(serde_json::json!({"lotteryNumber": batch, "newFree": true, "type": 1})),
            12000,
        ).await {
            Ok(_) => {
                remaining -= batch;
                total_paid_fish += batch;
                info!(target: "cli", batch, remaining, total_paid_fish, "monthly fish paid lottery ok");
            }
            Err(e) => {
                warn!(target: "cli", batch, remaining, error = %e, "monthly fish paid lottery failed, stop topup loop");
                stopped_early = true;
                break;
            }
        }

        if remaining > 0 && total_paid_fish > 0 && total_paid_fish % 50 == 0 {
            match game.role_getroleinfo().await {
                Ok(role_info) => {
                    rod_count = role_info
                        .pointer("/role/items/1011/quantity")
                        .or_else(|| role_info.pointer("/items/1011/quantity"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(rod_count as u64) as u32;
                    if rod_count < remaining {
                        info!(target: "cli", rod_count, remaining_before = remaining, "monthly fish sync rods and adjust remaining");
                        remaining = rod_count;
                    }
                }
                Err(e) => {
                    warn!(target: "cli", error = %e, "monthly fish failed to refresh rod count during topup");
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
    }

    if !args.no_claim_fish_point {
        let role_info = game.role_getroleinfo().await?;
        let points = role_info
            .pointer("/role/statistics/artifact:point")
            .or_else(|| role_info.pointer("/statistics/artifact:point"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let exchange_count = points / 20;
        if exchange_count > 0 {
            info!(target: "cli", points, exchange_count, "monthly fish claim cumulative rewards");
            for idx in 0..exchange_count {
                match game.artifact_exchange(serde_json::json!({})).await {
                    Ok(_) => {
                        info!(target: "cli", count = idx + 1, total = exchange_count, "monthly fish artifact_exchange ok");
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    Err(e) => {
                        warn!(target: "cli", error = %e, done = idx, total = exchange_count, "monthly fish artifact_exchange failed");
                        break;
                    }
                }
            }
        }
    }

    let final_query = query_monthly_progress(game, args.complete).await?;
    let final_target = if args.complete { final_query.fish.full_target } else { final_query.fish.progress_target };
    let final_need = if args.complete { final_query.fish.full_need } else { final_query.fish.need };
    if final_need == 0 {
        info!(target: "cli", final_current = final_query.fish.current, target = final_target, "monthly fish topup completed");
    } else if stopped_early {
        warn!(target: "cli", final_current = final_query.fish.current, target = final_target, need = final_need, "monthly fish topup stopped before reaching target");
    } else {
        info!(target: "cli", final_current = final_query.fish.current, target = final_target, need = final_need, "monthly fish topup finished without reaching target");
    }
    info!(
        target: "cli",
        final_current = final_query.fish.current,
        target = final_target,
        need = final_need,
        "monthly fish topup finished"
    );
    Ok(())
}

async fn run_monthly_arena_topup(
    config: &BatchConfig,
    game: &mut koc_core::GameClient,
    bin_path: &std::path::Path,
    server_id: u64,
    query: &MonthlyQueryResult,
    args: &MonthlyArgs,
) -> Result<(), String> {
    let hour = Local::now().hour();
    if !(6..22).contains(&hour) {
        return Err("arena topup be allowed on 06:00-22:00 only".to_string());
    }

    let target = if args.complete { query.arena.full_target } else { query.arena.progress_target };
    let remaining = if args.complete { query.arena.full_need } else { query.arena.need };
    info!(target: "cli", current = query.arena.current, target, remaining, complete = args.complete, "monthly arena target evaluated");
    if remaining == 0 {
        info!(target: "cli", "monthly arena already on target, skip topup");
        return Ok(());
    }

    let original_team = switch_context_formation(config, game, bin_path, server_id, FormationContext::Arena).await?;
    let topup_result = run_monthly_arena_topup_inner(game, target, remaining, args).await;
    let restore_result = restore_context_formation(game, FormationContext::Arena, original_team).await;

    topup_result?;
    restore_result?;
    Ok(())
}

async fn run_monthly_arena_topup_inner(
    game: &mut koc_core::GameClient,
    target: u32,
    mut remaining: u32,
    args: &MonthlyArgs,
) -> Result<(), String> {
    let role_info = game.role_getroleinfo().await?;
    let mut tickets_left = role_info
        .pointer("/role/items/1007/quantity")
        .or_else(|| role_info.pointer("/items/1007/quantity"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    info!(target: "cli", tickets_left, "monthly arena tickets available");
    remaining = remaining.min(tickets_left);
    if remaining == 0 {
        info!(target: "cli", "monthly arena no tickets available, skip topup");
        return Ok(());
    }

    let _ = game.arena_startarea().await;
    let mut safety_counter = 0u32;
    let mut round = 1u32;
    let mut stopped_early = false;
    while remaining > 0 && tickets_left > 0 && safety_counter < args.arena_safety_max {
        let plan_fights = ((remaining as f64) / 2.0).ceil() as u32;
        let plan_fights = plan_fights.min(tickets_left);
        info!(target: "cli", round, plan_fights, tickets_left, remaining, "monthly arena round start");
        let mut round_interrupted = false;

        for _ in 0..plan_fights {
            if safety_counter >= args.arena_safety_max { break; }
            let targets = match game.arena_getareatarget().await {
                Ok(v) => v,
                Err(e) => {
                    warn!(target: "cli", round, error = %e, "monthly arena get target failed, stop current round");
                    round_interrupted = true;
                    stopped_early = true;
                    break;
                }
            };
            let Some(target_id) = pick_arena_target_id(&targets) else {
                warn!(target: "cli", round, "monthly arena no target available, stop current round");
                round_interrupted = true;
                stopped_early = true;
                break;
            };

            match game.fight_startareaarena(target_id).await {
                Ok(_) => {
                    tickets_left = tickets_left.saturating_sub(1);
                    info!(target: "cli", target_id, tickets_left, "monthly arena fight ok");
                }
                Err(e) => {
                    warn!(target: "cli", target_id, error = %e, "monthly arena fight failed");
                }
            }

            safety_counter += 1;
            tokio::time::sleep(Duration::from_millis(1200)).await;
        }

        let query = query_monthly_progress(game, false).await?;
        let role_info = game.role_getroleinfo().await?;
        tickets_left = role_info
            .pointer("/role/items/1007/quantity")
            .or_else(|| role_info.pointer("/items/1007/quantity"))
            .and_then(|v| v.as_u64())
            .unwrap_or(tickets_left as u64) as u32;
        remaining = target.saturating_sub(query.arena.current).min(tickets_left);
        info!(target: "cli", round, current = query.arena.current, target, remaining, tickets_left, "monthly arena round finished");
        if round_interrupted && remaining > 0 {
            break;
        }
        round += 1;
    }

    let final_query = query_monthly_progress(game, false).await?;
    let final_need = target.saturating_sub(final_query.arena.current);
    if final_need == 0 {
        info!(target: "cli", final_current = final_query.arena.current, target, "monthly arena topup completed");
    } else if safety_counter >= args.arena_safety_max {
        warn!(target: "cli", final_current = final_query.arena.current, target, need = final_need, safety_counter, safety_max = args.arena_safety_max, "monthly arena topup stopped by safety max");
    } else if stopped_early {
        warn!(target: "cli", final_current = final_query.arena.current, target, need = final_need, "monthly arena topup stopped before reaching target");
    } else {
        info!(target: "cli", final_current = final_query.arena.current, target, need = final_need, "monthly arena topup finished without reaching target");
    }
    info!(
        target: "cli",
        final_current = final_query.arena.current,
        target,
        need = final_need,
        safety_counter,
        safety_max = args.arena_safety_max,
        "monthly arena topup finished"
    );
    Ok(())
}

fn is_today_available(last_time_sec: i64) -> bool {
    if last_time_sec <= 0 {
        return true;
    }
    let mut start_of_day = Local::now();
    start_of_day = start_of_day
        .with_hour(0).and_then(|dt| dt.with_minute(0)).and_then(|dt| dt.with_second(0)).and_then(|dt| dt.with_nanosecond(0))
        .expect("valid local midnight");
    last_time_sec < start_of_day.timestamp()
}

fn pick_arena_target_id(targets: &serde_json::Value) -> Option<u64> {
    if let Some(arr) = targets.as_array() {
        if let Some(candidate) = arr.first() {
            return candidate.get("roleId").or_else(|| candidate.get("id")).or_else(|| candidate.get("targetId")).and_then(|v| v.as_u64());
        }
    }

    for path in ["/rankList/0", "/roleList/0", "/targets/0", "/targetList/0", "/list/0"] {
        if let Some(candidate) = targets.pointer(path) {
            if let Some(id) = candidate.get("roleId").or_else(|| candidate.get("id")).or_else(|| candidate.get("targetId")).and_then(|v| v.as_u64()) {
                return Some(id);
            }
        }
    }
    targets.get("roleId").or_else(|| targets.get("id")).or_else(|| targets.get("targetId")).and_then(|v| v.as_u64())
}
