use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use chrono::{Datelike, Local, Timelike, Weekday};
use tokio::sync::{RwLock, Semaphore};
use tracing::{error, info, warn, info_span, Instrument};

use crate::config::{
    BatchConfig,
    ConfigWatcher,
    FormationContext,
    normalize_path_for_id,
    resolve_bin_path,
    role_display,
    role_key,
};
use crate::state::{AppState, SharedState};
use crate::{KocCore, GameClient, RoleInfo};
use crate::study;

/// role struct
#[derive(Debug, Clone)]
pub struct RoleTask {
    pub key: String,            // "bin_filename:serverId"
    pub display: String,        // "bin/{server_num}-{idx}-{role_name}"
    pub bin_path: String,
    pub server_id: u64,
    pub role_name: String,
}

impl RoleTask {
    fn short_display(&self) -> &str {
        self.display.rsplit_once('-').map(|(s, _)| s).unwrap_or(&self.display)
    }
}

/// role plan in a round(base on local state storage)
#[derive(Debug, Clone)]
struct RoleRunPlan {
    role: RoleTask,
    needs_daily: bool,
    needs_periodic: bool,
    needs_weekly: bool,
}

/// scheduler
pub struct Scheduler {
    config_watcher: ConfigWatcher,
    state: SharedState,
    state_path: PathBuf,
    roles: Vec<RoleTask>,
    /// every bin file's origin data
    bin_cache: std::collections::HashMap<String, Vec<u8>>,
    /// maintenance window (for log, avoid round info outputs in log)
    in_maintenance_window: bool,
}

#[derive(Debug, Clone, Copy)]
struct MaintenanceHit {
    weekday: Weekday,
    start_h: u32,
    start_m: u32,
    end_h: u32,
    end_m: u32,
}

struct TimeWindowPolicy {
    maintenance_hit: Option<MaintenanceHit>,
    daily_time_reached: bool,
}

impl TimeWindowPolicy {
    fn is_time_reached(now: &chrono::DateTime<Local>, hour: u32, minute: u32) -> bool {
        now.hour() > hour || (now.hour() == hour && now.minute() >= minute)
    }

    fn is_time_before(now: &chrono::DateTime<Local>, hour: u32, minute: u32) -> bool {
        now.hour() < hour || (now.hour() == hour && now.minute() < minute)
    }

    fn effective_daily_start(now: &chrono::DateTime<Local>, config: &BatchConfig) -> (u32, u32) {
        if now.weekday() == Weekday::Fri {
            config.parse_friday_daily_start_time()
        } else {
            config.parse_schedule_time()
        }
    }

    fn daily_time_reached_at(now: &chrono::DateTime<Local>, config: &BatchConfig) -> bool {
        let (daily_h, daily_m) = Self::effective_daily_start(now, config);
        Self::is_time_reached(now, daily_h, daily_m)
    }

    fn maintenance_hit(now: &chrono::DateTime<Local>, config: &BatchConfig) -> Option<MaintenanceHit> {
        for (weekday, start_h, start_m, end_h, end_m) in config.parse_maintenance_windows() {
            if now.weekday() == weekday
                && Self::is_time_reached(now, start_h, start_m)
                && Self::is_time_before(now, end_h, end_m)
            {
                return Some(MaintenanceHit {
                    weekday,
                    start_h,
                    start_m,
                    end_h,
                    end_m,
                });
            }
        }
        None
    }

    fn new(now: &chrono::DateTime<Local>, config: &BatchConfig) -> Self {
        Self {
            maintenance_hit: Self::maintenance_hit(now, config),
            daily_time_reached: Self::daily_time_reached_at(now, config),
        }
    }
}

struct PendingPlanner;

impl PendingPlanner {
    async fn collect(
        force_all: bool,
        roles: &[RoleTask],
        state: &SharedState,
        config: &BatchConfig,
        daily_time_reached: bool,
        is_weekly_day: bool,
    ) -> Vec<RoleRunPlan> {
        // reset expire state(automatic reset for cross day)
        {
            let mut st = state.write().await;
            for rs in st.roles.values_mut() {
                rs.daily.ensure_today();
                rs.weekly.ensure_this_week();
            }
        }

        let st = state.read().await;
        let mut pending = Vec::new();
        for role in roles {
            if force_all {
                pending.push(RoleRunPlan {
                    role: role.clone(),
                    needs_daily: daily_time_reached,
                    needs_periodic: true,
                    needs_weekly: config.study_enabled && is_weekly_day && daily_time_reached,
                });
                continue;
            }

            let role_state = st.roles.get(&role.key);
            let needs_daily = (daily_time_reached && !role_state
                .map(|rs| rs.daily.all_done())
                .unwrap_or(false))
                || (daily_time_reached
                    && config.car_enabled
                    && GameClient::is_car_send_window()
                    && !role_state.map(|rs| rs.daily.car_send_done).unwrap_or(false));
            let needs_periodic = role_state
                .map(|rs| {
                    rs.periodic.needs_hangup(config.hangup_threshold_hours)
                    || rs.periodic.needs_bottle(config.bottle_threshold_hours)
                    || rs.periodic.needs_legacy(config.legacy_interval_hours)
                    || (config.tower_enabled && rs.periodic.needs_tower())
                    || (config.evotower_enabled && GameClient::is_evotower_active() && rs.periodic.needs_evotower())
                })
                .unwrap_or(true);
            let needs_weekly = config.study_enabled
                && is_weekly_day
                && daily_time_reached
                && !role_state.map(|rs| rs.weekly.study).unwrap_or(false);

            if needs_daily || needs_periodic || needs_weekly {
                pending.push(RoleRunPlan {
                    role: role.clone(),
                    needs_daily,
                    needs_periodic,
                    needs_weekly,
                });
            }
        }

        pending
    }
}

#[derive(Clone)]
struct RoleRunner {
    state: SharedState,
    state_path: PathBuf,
    config: BatchConfig,
    round_prefix: String,
}

impl RoleRunner {
    async fn run_one(&self, plan: RoleRunPlan, bin_data: Vec<u8>) {
        let role_task = plan.role;
        let log_prefix = format!("{} [{}]", self.round_prefix, role_task.short_display());

        info!(target: "scheduler", plan_daily = plan.needs_daily, plan_periodic = plan.needs_periodic, plan_weekly = plan.needs_weekly, "role task starting");

        // obtain token
        let core = KocCore::new();
        let token = match core.select_role_token(&bin_data, role_task.server_id).await {
            Ok(t) => t,
            Err(e) => {
                error!(target: "scheduler", role = %role_task.short_display(), error = %e, "failed to get token");
                return;
            }
        };

        // login
        let mut game = match GameClient::login(&token).await {
            Ok(g) => g,
            Err(e) => {
                error!(target: "scheduler", role = %role_task.short_display(), error = %e, "login failed");
                return;
            }
        };
        info!(target: "scheduler", "login successful");

        // update periodic state from role_info
        {
            let mut st = self.state.write().await;
            let rs = st.get_or_create(&role_task.key);
            game.update_periodic_state(&mut rs.periodic);
        }

        // daily task
        if plan.needs_daily {
            info!(target: "scheduler", phase = "daily", "running daily tasks");
            let mut daily_state;
            {
                let st = self.state.read().await;
                daily_state = st.roles.get(&role_task.key)
                    .map(|rs| rs.daily.clone())
                    .unwrap_or_default();
            }

            let daily_cfg = self.config.daily_task_config(&role_task.bin_path, role_task.server_id);
            info!(target: "formation", role = %role_task.short_display(), "daily config resolved");

            let report = game.run_daily_tasks_stateful(
                &mut daily_state,
                &log_prefix,
                &daily_cfg,
            ).await;
            info!(target: "scheduler", phase = "daily", summary = %report.summary(), "daily tasks finished");

            let periodic_state_refreshed = match game.role_getroleinfo().await {
                Ok(info) => {
                    game.role_info = Some(info);
                    true
                }
                Err(e) => {
                    warn!(target: "scheduler", phase = "daily", error = %e, "failed to refresh periodic state after daily actions");
                    false
                }
            };

            // write back to state
            {
                let mut st = self.state.write().await;
                let rs = st.get_or_create(&role_task.key);
                rs.daily = daily_state;
                if periodic_state_refreshed {
                    game.update_periodic_state(&mut rs.periodic);
                }
                let _ = st.save(&self.state_path);
            }
        }

        // weekly task
        if plan.needs_weekly {
            info!(target: "scheduler", phase = "weekly", "running weekly tasks");
            let already_done = study::is_study_completed_this_week(game.role_info.as_ref());
            if !already_done {
                let mut report = crate::kpi::DailyTaskReport::new();
                let _ = study::run_study(&game.ws, &mut report, &log_prefix).await;
                info!(target: "scheduler", phase = "weekly", summary = %report.summary(), "study finished");
            } else {
                info!(target: "scheduler", phase = "weekly", "study already completed on server");
            }
            {
                let mut st = self.state.write().await;
                let rs = st.get_or_create(&role_task.key);
                rs.weekly.study = true;
                let _ = st.save(&self.state_path);
            }
        }

        // periodic task
        if plan.needs_periodic {
            info!(target: "scheduler", phase = "periodic", "running periodic tasks");
            let mut periodic_state;
            {
                let st = self.state.read().await;
                periodic_state = st.roles.get(&role_task.key)
                    .map(|rs| rs.periodic.clone())
                    .unwrap_or_default();
            }

            let (tower_team, _) = self.config.resolve_formation_with_source(&role_task.bin_path, role_task.server_id, FormationContext::Tower);
            let (evotower_team, _) = self.config.resolve_formation_with_source(&role_task.bin_path, role_task.server_id, FormationContext::Evotower);

            let report = game.run_periodic_tasks(
                &mut periodic_state, &self.config,
                tower_team, evotower_team, &log_prefix
            ).await;
            if report.results.len() > 0 {
                info!(target: "scheduler", phase = "periodic", summary = %report.summary(), "periodic tasks finished");
            } else {
                info!(target: "scheduler", phase = "periodic", "no actionable periodic tasks after login refresh");
            }

            let (dh, dm) = self.config.parse_schedule_time();
            let daily_reached = chrono::Local::now().num_seconds_from_midnight() >= (dh * 3600 + dm * 60) as u32;

            let can_claim = {
                let st = self.state.read().await;
                st.roles.get(&role_task.key).map(|rs| {
                    self.config.car_enabled
                        && daily_reached
                        && !rs.daily.car_claimed_today
                        && rs.daily.next_car_claim_time > 0.0
                        && crate::state::now_secs() >= rs.daily.next_car_claim_time
                }).unwrap_or(false)
            };
            if can_claim {
                info!(target: "scheduler", phase = "car-claim", "claiming cars during periodic");
                let (claim_report, next_claim) = game.claim_all_cars(&log_prefix).await;
                info!(target: "scheduler", phase = "car-claim", summary = %claim_report.summary(), "claim cars finished");

                {
                    let mut st = self.state.write().await;
                    let rs = st.get_or_create(&role_task.key);
                    rs.daily.car_claimed_today = true;
                    rs.daily.next_car_claim_time = next_claim;
                    let _ = st.save(&self.state_path);
                }
            }

            {
                let mut st = self.state.write().await;
                let rs = st.get_or_create(&role_task.key);
                rs.periodic = periodic_state;
                let _ = st.save(&self.state_path);
            }
        }

        // disconnect
        game.disconnect().await;
        info!(target: "scheduler", "role task done");
    }
}

impl Scheduler {
    pub async fn init(config_path: PathBuf, state_path: PathBuf) -> Result<Self, String> {
        let config = BatchConfig::load(&config_path)?;
        let mut state = AppState::load(&state_path);
        let migrated = state.normalize_role_keys();

        info!(target: "scheduler", concurrency = config.concurrency, schedule_time = %config.schedule_time, "config loaded");
        info!(target: "scheduler", existing_roles = state.roles.len(), "state loaded");
        if migrated > 0 {
            info!(target: "scheduler", migrated = migrated, "migrated legacy role keys to normalized paths");
        }

        let config_watcher = ConfigWatcher::new(config_path, config.clone());
        let shared_state = Arc::new(RwLock::new(state));

        let mut scheduler = Self {
            config_watcher,
            state: shared_state,
            state_path,
            roles: Vec::new(),
            bin_cache: std::collections::HashMap::new(),
            in_maintenance_window: false,
        };

        // scan bin files, construct role lists
        scheduler.scan_roles(&config).await?;

        // sync state & role
        {
            let new_keys: HashSet<String> = scheduler.roles.iter().map(|r| r.key.clone()).collect();
            let mut st = scheduler.state.write().await;
            let (added, removed) = st.sync_with_roles(&new_keys);
            if !added.is_empty() {
                info!(target: "scheduler", ?added, "new roles discovered");
            }
            if !removed.is_empty() {
                info!(target: "scheduler", ?removed, "roles removed");
            }
            // update role display infomation
            for role in &scheduler.roles {
                let rs = st.get_or_create(&role.key);
                rs.role_name = role.role_name.clone();
                rs.server_display = role.display.clone();
            }
            st.save(&scheduler.state_path).map_err(|e| e.to_string())?;
        }

        info!(target: "scheduler", total_roles = scheduler.roles.len(), "role scan completed");
        for role in &scheduler.roles {
            info!(target: "scheduler", role = %role.short_display(), "role loaded");
        }

        Ok(scheduler)
    }

    /// scan bin files, construct role lists
    async fn scan_roles(&mut self, config: &BatchConfig) -> Result<(), String> {
        let core = KocCore::new();
        self.roles.clear();

        for bin_cfg in &config.bins {
            let normalized_path = normalize_path_for_id(&bin_cfg.path);
            let resolved_path = resolve_bin_path(self.config_watcher.config_path(), &bin_cfg.path);
            info!(target: "scheduler", bin = %normalized_path, "scanning bin");

            // read bin file
            let bin_data = fs::read(&resolved_path)
                .map_err(|e| format!("Failed to read {} ({}): {}", bin_cfg.path, resolved_path.display(), e))?;
            self.bin_cache.insert(normalized_path.clone(), bin_data.clone());

            // obtain role lists
            let all_roles = match core.get_server_list(&bin_data).await {
                Ok(r) => r,
                Err(e) => {
                    warn!(target: "scheduler", bin = %normalized_path, error = %e, "failed to get server list");
                    continue;
                }
            };

            // filter
            let filtered: Vec<&RoleInfo> = if bin_cfg.server_ids.is_empty() {
                all_roles.iter().collect()
            } else {
                all_roles.iter()
                    .filter(|r| bin_cfg.server_ids.contains(&r.server_id))
                    .collect()
            };

            for role in filtered {
                let key = role_key(&normalized_path, role.server_id);
                let display = role_display(&normalized_path, role.server_id, &role.name);
                self.roles.push(RoleTask {
                    key,
                    display,
                    bin_path: normalized_path.clone(),
                    server_id: role.server_id,
                    role_name: role.name.clone(),
                });
            }
        }

        Ok(())
    }

    /// main scheduler loop
    pub async fn run(&mut self) -> Result<(), String> {
        info!(target: "scheduler", "starting main loop");

        // execute all in the first round
        info!(target: "scheduler", "first round full execution for all roles");
        self.execute_round(true).await;

        loop {
            let check_interval = self.config_watcher.config().check_interval_secs;
            tokio::time::sleep(tokio::time::Duration::from_secs(check_interval)).await;

            // hot reload check
            let (config_changed, changed_bins) = self.config_watcher.check_reload();
            if config_changed || !changed_bins.is_empty() {
                info!(target: "config", changed_bin_count = changed_bins.len(), config_changed, "detected config/bin changes");
                if config_changed {
                    info!(target: "config", "config file changed");
                }
                for bin in &changed_bins {
                    info!(target: "config", bin = %bin, "bin file changed");
                }
                // re scan roles
                let config = self.config_watcher.config().clone();
                if let Err(e) = self.scan_roles(&config).await {
                    warn!(target: "config", error = %e, "rescan failed");
                } else {
                    // sync state
                    let new_keys: HashSet<String> = self.roles.iter().map(|r| r.key.clone()).collect();
                    let mut st = self.state.write().await;
                    let (added, removed) = st.sync_with_roles(&new_keys);
                    if !added.is_empty() { info!(target: "config", ?added, "added roles"); }
                    if !removed.is_empty() { info!(target: "config", ?removed, "removed roles"); }
                    for role in &self.roles {
                        let rs = st.get_or_create(&role.key);
                        rs.role_name = role.role_name.clone();
                        rs.server_display = role.display.clone();
                    }
                    let _ = st.save(&self.state_path);
                }
            }

            self.execute_round(false).await;
        }
    }

    /// execute one round: check role's todo items, in concurrency
    async fn execute_round(&mut self, force_all: bool) {
        let config = self.config_watcher.config().clone();
        let now = Local::now();
        let policy = TimeWindowPolicy::new(&now, &config);

        // upgrade round
        let round_advance;
        {
            let mut st = self.state.write().await;
            round_advance = st.next_round();
            if round_advance.reset_happened {
                let _ = st.save(&self.state_path);
            }
        }
        let round = round_advance.round;

        let round_prefix = format!("[R{}]", round);
        let time_str = now.format("%Y-%m-%d %H:%M:%S").to_string();
        if round_advance.reset_happened {
            info!(
                target: "scheduler",
                previous_round_date = ?round_advance.previous_date,
                new_round_date = %round_advance.current_date,
                round = round,
                "round counter reset for new day"
            );
        }
        info!(
            target: "scheduler",
            round = round,
            force_all,
            roles_total = self.roles.len(),
            now = %time_str,
            check_interval_secs = config.check_interval_secs,
            config_schedule_time = %config.schedule_time,
            "round start"
        );

        if let Some(hit) = policy.maintenance_hit {
            if !self.in_maintenance_window {
                warn!(
                    target: "scheduler",
                    round = round,
                    weekday = ?hit.weekday,
                    start = %format!("{:02}:{:02}", hit.start_h, hit.start_m),
                    end = %format!("{:02}:{:02}", hit.end_h, hit.end_m),
                    "[MAINTENANCE] window entered, skip scheduler rounds"
                );
                self.in_maintenance_window = true;
            }
            return;
        } else if self.in_maintenance_window {
            info!(target: "scheduler", round = round, "maintenance window exited, scheduler resumed");
            self.in_maintenance_window = false;
        }

        // collect role which need working
        let is_weekly_day = now.weekday() == config.weekly_schedule_day;
        let pending = PendingPlanner::collect(
            force_all,
            &self.roles,
            &self.state,
            &config,
            policy.daily_time_reached,
            is_weekly_day,
        ).await;

        if pending.is_empty() {
            info!(target: "scheduler", round = round, now = %time_str, "no roles need work");
            return;
        }

        info!(target: "scheduler", round = round, pending = pending.len(), now = %time_str, "roles need work");

        // concurrency
        let semaphore = Arc::new(Semaphore::new(config.concurrency));
        let delay_between = config.delay_between_ms;
        let mut handles = Vec::new();
        let role_runner = RoleRunner {
            state: self.state.clone(),
            state_path: self.state_path.clone(),
            config: config.clone(),
            round_prefix: round_prefix.clone(),
        };

        for plan in pending {
            let sem = semaphore.clone();
            let runner = role_runner.clone();
            let bin_data = self.bin_cache.get(&plan.role.bin_path).cloned().unwrap_or_default();
            let role_name_for_span = plan.role.short_display().to_string();
            let round_for_span = round;

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                runner.run_one(plan, bin_data).await;
            }.instrument(info_span!("RT", R = round_for_span, role = %role_name_for_span)));

            handles.push(handle);

            // snap for a while
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_between)).await;
        }

        // join
        for h in handles {
            let _ = h.await;
        }

        // Flush state
        {
            let st = self.state.read().await;
            let _ = st.save(&self.state_path);
        }

        let elapsed = Local::now().signed_duration_since(now);
        info!(target: "scheduler", round = round, round_prefix = %round_prefix, elapsed_s = elapsed.num_seconds(), "round complete");
    }
}
