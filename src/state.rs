use chrono::{Datelike, Local};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

/// global app state (Arc<RwLock<>> for concurrency)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    /// round
    #[serde(default)]
    pub round_date: Option<String>,
    #[serde(default)]
    pub round_count: u32,

    /// role's state, key = "bin_filename:serverId"
    #[serde(default)]
    pub roles: HashMap<String, RoleState>,
}

/// round advance
#[derive(Debug, Clone)]
pub struct RoundAdvance {
    pub round: u32,
    pub reset_happened: bool,
    pub previous_date: Option<String>,
    pub current_date: String,
}

/// full state for single role
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleState {
    /// display info
    #[serde(default)]
    pub role_name: String,
    #[serde(default)]
    pub server_display: String,

    /// daily task state
    #[serde(default)]
    pub daily: RoleDailyState,

    /// periodic task state
    #[serde(default)]
    pub periodic: RolePeriodicState,

    /// weekly task state
    #[serde(default)]
    pub weekly: RoleWeeklyState,
}

/// daily sub-task's state
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoleDailyState {
    /// date (YYYY-MM-DD), reset in cross day
    #[serde(default)]
    pub date: Option<String>,

    /// daily task done counts (retry)
    #[serde(default)]
    pub retry_count: u32,

    // --- 10 main tasks ---
    #[serde(default)]
    pub signin: bool, // signin
    #[serde(default)]
    pub buy_gold: bool, // god
    #[serde(default)]
    pub share: bool, // share
    #[serde(default)]
    pub friend_gift: bool, // friend gift
    #[serde(default)]
    pub recruit: bool, // recruit
    #[serde(default)]
    pub hangup_reward: bool, // hangup reward
    #[serde(default)]
    pub open_box: bool, // box
    #[serde(default)]
    pub store_purchase: bool, // store
    #[serde(default)]
    pub arena: bool, // arena
    #[serde(default)]
    pub bottle_task: bool, // bottle

    // --- non-main daily task ---
    #[serde(default)]
    pub boss: bool, // daily Boss
    #[serde(default)]
    pub legion_boss: bool, // legion Boss
    #[serde(default)]
    pub legion_signin: bool, // legion signin
    #[serde(default)]
    pub artifact: bool, // fishing
    #[serde(default)]
    pub genie: bool, // genie
    #[serde(default)]
    pub gacha: bool, // free gacha
    #[serde(default)]
    pub discount: bool, // discount
    #[serde(default)]
    pub card: bool, // card
    #[serde(default)]
    pub collection: bool, // collection
    #[serde(default)]
    pub task_rewards: bool, // claim rewards
    #[serde(default)]
    pub mail: bool, // mail
    #[serde(default)]
    pub nightmare: bool, // nightmare
    #[serde(default)]
    pub dream_shop: bool, // dream shop
    #[serde(default)]
    pub car_send_done: bool,
    #[serde(default)]
    pub next_car_claim_time: f64,
    #[serde(default)]
    pub car_claimed_today: bool,   // today has claimed cars in periodic
}

/// periodic task timestamp(for calculate)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RolePeriodicState {
    /// hangup: role.hangUp.lastTime (second)
    #[serde(default)]
    pub hangup_last_time: f64,
    /// hangup: role.hangUp.hangUpTime (second)
    #[serde(default)]
    pub hangup_time: f64,
    /// bottle: role.bottleHelpers.helperStopTime (second)
    #[serde(default)]
    pub bottle_stop_time: f64,
    /// legacy: last claim time (second, unix timestamp)
    #[serde(default)]
    pub legacy_last_claim: f64,
    /// hangup check time (second, 0(default)=immediately)
    #[serde(default)]
    pub hangup_next_check: f64,
    /// tower check time (second, 0=immediately)
    #[serde(default)]
    pub tower_next_check: f64,
    /// tower has reached the permanent clear state
    #[serde(default)]
    pub tower_cleared: bool,
    /// evotower check time (second, 0=immediately)
    #[serde(default)]
    pub evo_next_check: f64,
}

/// weekly task state (reset in cross week)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoleWeeklyState {
    #[serde(default)]
    pub week: Option<String>,  // "2026-W18"
    #[serde(default)]
    pub study: bool,           // study complete
}

impl RoleWeeklyState {
    pub fn ensure_this_week(&mut self) {
        let now = Local::now();
        let week = format!("{}-W{:02}", now.iso_week().year(), now.iso_week().week());
        if self.week.as_deref() != Some(&week) {
            *self = RoleWeeklyState { week: Some(week), ..Default::default() };
        }
    }
}

impl AppState {
    /// get from state.json, otherwise return null state
    pub fn load(path: &Path) -> Self {
        if let Ok(content) = fs::read_to_string(path) {
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    /// save to state.json
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize state: {}", e))?;
        fs::write(path, content).map_err(|e| format!("Failed to write state file: {}", e))?;
        Ok(())
    }

    /// get or create role state
    pub fn get_or_create(&mut self, key: &str) -> &mut RoleState {
        self.roles
            .entry(key.to_string())
            .or_insert_with(|| RoleState {
                role_name: String::new(),
                server_display: String::new(),
                daily: RoleDailyState::default(),
                periodic: RolePeriodicState::default(),
                weekly: RoleWeeklyState::default(),
            })
    }

    /// increase round, reset in cross day
    pub fn next_round(&mut self) -> RoundAdvance {
        let today = Local::now().format("%Y-%m-%d").to_string();
        let previous_date = self.round_date.clone();
        let reset_happened = self.round_date.as_deref() != Some(&today);
        if reset_happened {
            self.round_date = Some(today);
            self.round_count = 1;
        } else {
            self.round_count += 1;
        }
        RoundAdvance {
            round: self.round_count,
            reset_happened,
            previous_date,
            current_date: self.round_date.clone().unwrap_or_default(),
        }
    }

    /// sync role info: diff role lists, add null state, remove orphan state
    pub fn sync_with_roles(&mut self, new_keys: &HashSet<String>) -> (Vec<String>, Vec<String>) {
        let old_keys: HashSet<String> = self.roles.keys().cloned().collect();

        let added: Vec<String> = new_keys.difference(&old_keys).cloned().collect();
        let removed: Vec<String> = old_keys.difference(new_keys).cloned().collect();

        for key in &added {
            self.roles.insert(key.clone(), RoleState::default());
        }
        for key in &removed {
            self.roles.remove(key);
        }

        (added, removed)
    }

    /// - normalize the bin_path: "bin_path:serverId"
    pub fn normalize_role_keys(&mut self) -> usize {
        let old_roles = std::mem::take(&mut self.roles);
        let mut new_roles = HashMap::with_capacity(old_roles.len());
        let mut migrated = 0usize;

        for (key, state) in old_roles {
            let normalized_key = if let Some((path_part, sid_part)) = key.rsplit_once(':') {
                if sid_part.parse::<u64>().is_ok() {
                    let normalized_path = crate::config::normalize_path_for_id(path_part);
                    format!("{}:{}", normalized_path, sid_part)
                } else {
                    key.clone()
                }
            } else {
                key.clone()
            };

            if normalized_key != key {
                migrated += 1;
            }
            new_roles.entry(normalized_key).or_insert(state);
        }

        self.roles = new_roles;
        migrated
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            round_date: None,
            round_count: 0,
            roles: HashMap::new(),
        }
    }
}

impl Default for RoleState {
    fn default() -> Self {
        Self {
            role_name: String::new(),
            server_display: String::new(),
            daily: RoleDailyState::default(),
            periodic: RolePeriodicState::default(),
            weekly: RoleWeeklyState::default(),
        }
    }
}

impl RoleDailyState {
    pub fn ensure_today(&mut self) {
        let today = Local::now().format("%Y-%m-%d").to_string();
        if self.date.as_deref() != Some(&today) {
            *self = RoleDailyState {
                date: Some(today),
                ..Default::default()
            };
        }
    }

    /// is all main task done
    pub fn all_main_done(&self) -> bool {
        self.signin
            && self.buy_gold
            && self.share
            && self.friend_gift
            && self.recruit
            && self.hangup_reward
            && self.open_box
            && self.store_purchase
            && self.arena
            && self.bottle_task
    }

    /// is all task done
    pub fn all_done(&self) -> bool {
        self.all_main_done()
            && self.boss
            && self.legion_boss
            && self.legion_signin
            && self.artifact
            && self.genie
            && self.discount
            && self.card
            && self.collection
            && self.task_rewards
            && self.mail
    }
}

impl RolePeriodicState {
    /// hangup condition check
    pub fn needs_hangup(&self, threshold_hours: f64) -> bool {
        let now = now_secs();
        if self.hangup_next_check > 0.0 {
            return now >= self.hangup_next_check;
        }
        if self.hangup_last_time <= 0.0 {
            return true;
        }
        now - self.hangup_last_time >= threshold_hours * 3600.0
    }

    /// bottle operate check (remaining <= threshold)
    pub fn needs_bottle(&self, threshold_hours: f64) -> bool {
        if self.bottle_stop_time <= 0.0 {
            return true;
        }
        let now = now_secs();
        let remaining = self.bottle_stop_time - now;
        remaining <= threshold_hours * 3600.0
    }

    /// legacy claim check (delta time >= interval)
    pub fn needs_legacy(&self, interval_hours: f64) -> bool {
        if self.legacy_last_claim <= 0.0 {
            return true;
        }
        let now = now_secs();
        let since = now - self.legacy_last_claim;
        since >= interval_hours * 3600.0
    }

    /// tower check condition
    pub fn needs_tower(&self) -> bool {
        !self.tower_cleared && (self.tower_next_check <= 0.0 || now_secs() >= self.tower_next_check)
    }

    /// evotower check condition
    pub fn needs_evotower(&self) -> bool {
        self.evo_next_check <= 0.0 || now_secs() >= self.evo_next_check
    }

    /// update hangup from role
    pub fn update_hangup_from_role(&mut self, role_info: &serde_json::Value) {
        let hangup = role_info
            .pointer("/role/hangUp")
            .or_else(|| role_info.pointer("/hangUp"));
        if let Some(obj) = hangup {
            if let Some(v) = obj.get("lastTime").and_then(|v| v.as_f64()) {
                self.hangup_last_time = v;
            }
            if let Some(v) = obj.get("hangUpTime").and_then(|v| v.as_f64()) {
                self.hangup_time = v;
            }

            let now = now_secs();
            let remaining = self.hangup_time - (now - self.hangup_last_time);
            self.hangup_next_check = if self.hangup_last_time > 0.0 && remaining > 3600.0 {
                now + remaining - 3600.0
            } else {
                0.0
            };
        }
    }

    /// update bottle state from role
    pub fn update_bottle_from_role(&mut self, role_info: &serde_json::Value) {
        let helpers = role_info
            .pointer("/role/bottleHelpers")
            .or_else(|| role_info.pointer("/bottleHelpers"));
        if let Some(obj) = helpers {
            if let Some(v) = obj.get("helperStopTime").and_then(|v| v.as_f64()) {
                self.bottle_stop_time = v;
            }
        }
    }
}

/// current time (second, unix timestamp)
pub fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// shared state
pub type SharedState = Arc<RwLock<AppState>>;

/// new shared state
pub fn new_shared_state(state: AppState) -> SharedState {
    Arc::new(RwLock::new(state))
}

#[cfg(test)]
mod tests {
    use super::{RolePeriodicState, now_secs};
    use serde_json::json;

    #[test]
    fn cleared_tower_never_needs_another_check() {
        let state = RolePeriodicState {
            tower_next_check: 0.0,
            tower_cleared: true,
            ..Default::default()
        };

        assert!(!state.needs_tower());
    }

    #[test]
    fn legacy_periodic_state_defaults_tower_to_not_cleared() {
        let state: RolePeriodicState =
            serde_json::from_str(r#"{"tower_next_check":12345.0}"#).unwrap();

        assert!(!state.tower_cleared);
        assert_eq!(state.tower_next_check, 12345.0);
    }

    #[test]
    fn tower_clear_state_survives_serialization() {
        let state = RolePeriodicState {
            tower_cleared: true,
            ..Default::default()
        };

        let encoded = serde_json::to_string(&state).unwrap();
        let decoded: RolePeriodicState = serde_json::from_str(&encoded).unwrap();

        assert!(decoded.tower_cleared);
    }

    #[test]
    fn refreshed_role_state_defers_bottle_and_hangup_checks() {
        let now = now_secs();
        let mut state = RolePeriodicState {
            bottle_stop_time: now + 1800.0,
            ..Default::default()
        };
        assert!(state.needs_bottle(1.0));

        let refreshed = json!({
            "role": {
                "bottleHelpers": {"helperStopTime": now + 8.0 * 3600.0},
                "hangUp": {"lastTime": now, "hangUpTime": 12.0 * 3600.0}
            }
        });
        state.update_bottle_from_role(&refreshed);
        state.update_hangup_from_role(&refreshed);

        assert!(!state.needs_bottle(1.0));
        assert!(state.hangup_next_check > now + 10.0 * 3600.0);
        assert!(!state.needs_hangup(8.0));
    }
}
