use chrono::Weekday;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use crate::kpi::{DailyFormationPlan, DailyTaskConfig};
use std::time::SystemTime;
use tracing::warn;

/// batch scheduler configuration(get from config.yaml)
#[derive(Debug, Clone)]
pub struct BatchConfig {
    pub concurrency: usize,
    pub delay_between_ms: u64,
    pub schedule_time: String,
    pub friday_daily_start_time: String,
    pub maintenance_windows: Vec<MaintenanceWindowConfig>,
    pub max_daily_retries: u32,
    pub check_interval_secs: u64,
    pub hangup_threshold_hours: f64,
    pub bottle_threshold_hours: f64,
    pub legacy_interval_hours: f64,
    pub bin_output_dir: String,
    pub default_bin_path: String,
    pub bins: Vec<BinConfig>,
    formation_defaults: FormationDefaults,
    role_formations: HashMap<String, HashMap<u64, FormationOverride>>,
    role_dream_shop: HashMap<String, HashMap<u64, DreamShopConfig>>,
    pub car_enabled: bool,
    pub weekly_schedule_day: Weekday,
    pub study_enabled: bool,
    pub tower_enabled: bool,
    pub evotower_enabled: bool,
    pub gacha_enabled: bool,
    pub groups: Vec<String>,
    pub role_groups: HashMap<String, Vec<(String, u64)>>,
}

/// single bin file configuration(runtime)
#[derive(Debug, Clone)]
pub struct BinConfig {
    pub bin: String,
    pub path: String,
    pub server_ids: Vec<u64>,
}

/// maintenance window
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MaintenanceWindowConfig {
    pub weekday: String,
    pub start_time: String,
    pub end_time: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DreamShopConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub purchase_list: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum FormationContext {
    Arena,
    Tower,
    Evotower,
    BossDaily,
    BossLegion,
}

impl FormationContext {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Arena => "arena",
            Self::Tower => "tower",
            Self::Evotower => "evotower",
            Self::BossDaily => "boss_daily",
            Self::BossLegion => "boss_legion",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RawBatchConfig {
    #[serde(default = "default_concurrency")]
    concurrency: usize,
    #[serde(default = "default_delay_between_ms")]
    delay_between_ms: u64,
    #[serde(default = "default_schedule_time")]
    schedule_time: String,
    #[serde(default = "default_friday_daily_start_time")]
    friday_daily_start_time: String,
    #[serde(default = "default_maintenance_windows")]
    maintenance_windows: Vec<MaintenanceWindowConfig>,
    #[serde(default = "default_max_daily_retries")]
    max_daily_retries: u32,
    #[serde(default = "default_check_interval_secs")]
    check_interval_secs: u64,
    #[serde(default = "default_hangup_threshold_hours")]
    hangup_threshold_hours: f64,
    #[serde(default = "default_bottle_threshold_hours")]
    bottle_threshold_hours: f64,
    #[serde(default = "default_legacy_interval_hours")]
    legacy_interval_hours: f64,
    #[serde(default = "default_bin_output_dir")]
    bin_output_dir: String,
    #[serde(default = "default_default_bin_path")]
    default_bin_path: String,
    #[serde(default = "default_formations_config")]
    formations: FormationsConfig,
    #[serde(default)]
    dream_shop_presets: HashMap<String, DreamShopConfig>,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    bins: Vec<YamlBinConfig>,
    #[serde(default = "default_car_enabled")]
    car_enabled: bool,
    #[serde(default = "default_weekly_schedule_day")]
    weekly_schedule_day: String,
    #[serde(default = "default_study_enabled")]
    study_enabled: bool,
    #[serde(default = "default_tower_enabled")]
    tower_enabled: bool,
    #[serde(default = "default_evotower_enabled")]
    evotower_enabled: bool,
    #[serde(default = "default_gacha_enabled")]
    gacha_enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct FormationsConfig {
    #[serde(default = "default_formation_defaults")]
    defaults: FormationDefaults,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct YamlBinConfig {
    bin: String,
    #[serde(default)]
    roles: Vec<YamlRoleConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct YamlRoleConfig {
    server_id: u64,
    #[serde(default)]
    formations: Option<FormationOverride>,
    #[serde(default)]
    dream_shop: Option<DreamShopRef>,
    #[serde(default)]
    group: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum DreamShopRef {
    Inline(DreamShopConfig),
    Preset(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct FormationDefaults {
    #[serde(default = "default_team_id")]
    arena: u64,
    #[serde(default = "default_team_id")]
    tower: u64,
    #[serde(default = "default_team_id")]
    evotower: u64,
    #[serde(default = "default_team_id")]
    boss_daily: u64,
    #[serde(default = "default_team_id")]
    boss_legion: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct FormationOverride {
    #[serde(default)]
    arena: Option<u64>,
    #[serde(default)]
    tower: Option<u64>,
    #[serde(default)]
    evotower: Option<u64>,
    #[serde(default)]
    boss_daily: Option<u64>,
    #[serde(default)]
    boss_legion: Option<u64>,
}

fn default_concurrency() -> usize {
    5
}
fn default_delay_between_ms() -> u64 {
    2000
}
fn default_schedule_time() -> String {
    "06:00".to_string()
}
fn default_friday_daily_start_time() -> String {
    "12:10".to_string()
}
fn default_maintenance_windows() -> Vec<MaintenanceWindowConfig> {
    vec![
        MaintenanceWindowConfig {
            weekday: "Fri".to_string(),
            start_time: "05:00".to_string(),
            end_time: "07:00".to_string(),
        },
        MaintenanceWindowConfig {
            weekday: "Sat".to_string(),
            start_time: "19:15".to_string(),
            end_time: "21:15".to_string(),
        },
        MaintenanceWindowConfig {
            weekday: "Sun".to_string(),
            start_time: "19:15".to_string(),
            end_time: "20:45".to_string(),
        },
    ]
}
fn default_max_daily_retries() -> u32 {
    3
}
fn default_car_enabled() -> bool {
    true
}
fn default_weekly_schedule_day() -> String {
    "Sat".to_string()
}
fn default_study_enabled() -> bool {
    true
}
fn default_tower_enabled() -> bool {
    true
}
fn default_evotower_enabled() -> bool {
    true
}
fn default_gacha_enabled() -> bool {
    true
}
fn default_check_interval_secs() -> u64 {
    60
}
fn default_hangup_threshold_hours() -> f64 {
    8.0
}
fn default_bottle_threshold_hours() -> f64 {
    1.0
}
fn default_legacy_interval_hours() -> f64 {
    4.0
}
fn default_bin_output_dir() -> String {
    "bins/".to_string()
}
fn default_default_bin_path() -> String {
    "bins".to_string()
}
fn default_team_id() -> u64 {
    1
}
fn default_formation_defaults() -> FormationDefaults {
    FormationDefaults {
        arena: 1,
        tower: 1,
        evotower: 1,
        boss_daily: 1,
        boss_legion: 1,
    }
}
fn default_formations_config() -> FormationsConfig {
    FormationsConfig {
        defaults: default_formation_defaults(),
    }
}

impl BatchConfig {
    /// load configuration from yaml
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file {:?}: {}", path, e))?;
        let raw: RawBatchConfig = serde_yaml::from_str(&content)
            .map_err(|e| format!("Failed to parse config file {:?}: {}", path, e))?;

        let mut config = Self {
            concurrency: raw.concurrency,
            delay_between_ms: raw.delay_between_ms,
            schedule_time: raw.schedule_time,
            friday_daily_start_time: raw.friday_daily_start_time,
            maintenance_windows: raw.maintenance_windows,
            max_daily_retries: raw.max_daily_retries,
            check_interval_secs: raw.check_interval_secs,
            hangup_threshold_hours: raw.hangup_threshold_hours,
            bottle_threshold_hours: raw.bottle_threshold_hours,
            legacy_interval_hours: raw.legacy_interval_hours,
            bin_output_dir: raw.bin_output_dir,
            default_bin_path: normalize_dir(&raw.default_bin_path),
            bins: Vec::new(),
            formation_defaults: raw.formations.defaults,
            role_formations: HashMap::new(),
            role_dream_shop: HashMap::new(),
            car_enabled: raw.car_enabled,
            weekly_schedule_day: parse_weekday(&raw.weekly_schedule_day)
                .ok_or_else(|| format!("Invalid weekly_schedule_day: '{}'", raw.weekly_schedule_day))?,
            study_enabled: raw.study_enabled,
            tower_enabled: raw.tower_enabled,
            evotower_enabled: raw.evotower_enabled,
            gacha_enabled: raw.gacha_enabled,
            groups: raw.groups,
            role_groups: HashMap::new(),
        };

        config.normalize_maintenance_windows();
        config.build_bins(raw.bins, &raw.dream_shop_presets);
        Ok(config)
    }

    fn build_bins(&mut self, bins: Vec<YamlBinConfig>, presets: &HashMap<String, DreamShopConfig>) {
        let mut runtime_bins = Vec::with_capacity(bins.len());
        let mut formations_map: HashMap<String, HashMap<u64, FormationOverride>> = HashMap::new();
        let mut dream_shop_map: HashMap<String, HashMap<u64, DreamShopConfig>> = HashMap::new();
        let valid_groups: std::collections::HashSet<&str> = self.groups.iter().map(|s| s.as_str()).collect();

        for bin in bins {
            let normalized_bin = normalize_path_for_id(&bin.bin);
            let runtime_path = build_runtime_bin_path(&self.default_bin_path, &normalized_bin);
            let server_ids: Vec<u64> = bin.roles.iter().map(|r| r.server_id).collect();
            let mut role_map = HashMap::new();
            let mut role_dream_map = HashMap::new();
            for role in bin.roles {
                if let Some(ref g) = role.group {
                    if valid_groups.contains(g.as_str()) {
                        self.role_groups.entry(g.clone())
                            .or_default()
                            .push((runtime_path.clone(), role.server_id));
                    } else {
                        tracing::warn!(target: "config", group = %g, bin = %bin.bin, server_id = role.server_id, "unknown group, skipping");
                    }
                }
                if let Some(formations) = role.formations {
                    role_map.insert(role.server_id, formations);
                }
                if let Some(dream_shop) = role.dream_shop {
                    if let Some(resolved) = resolve_dream_shop_ref(dream_shop, presets) {
                        role_dream_map.insert(role.server_id, resolved);
                    }
                }
            }
            if !role_map.is_empty() {
                formations_map.insert(normalized_bin.clone(), role_map);
            }
            if !role_dream_map.is_empty() {
                dream_shop_map.insert(normalized_bin.clone(), role_dream_map);
            }
            runtime_bins.push(BinConfig {
                bin: normalized_bin,
                path: runtime_path,
                server_ids,
            });
        }

        self.bins = runtime_bins;
        self.role_formations = formations_map;
        self.role_dream_shop = dream_shop_map;
    }

    pub fn resolve_formation(
        &self,
        bin_path: &str,
        server_id: u64,
        context: FormationContext,
    ) -> u64 {
        self.resolve_formation_with_source(bin_path, server_id, context)
            .0
    }

    pub fn resolve_formation_with_source(
        &self,
        bin_path: &str,
        server_id: u64,
        context: FormationContext,
    ) -> (u64, &'static str) {
        let bin_name = bin_name_from_path(bin_path);
        if let Some(role_map) = self.role_formations.get(&bin_name) {
            if let Some(override_cfg) = role_map.get(&server_id) {
                if let Some(team_id) = override_cfg.resolve(context) {
                    return (team_id, "role");
                }
            }
        }
        (self.formation_defaults.resolve(context), "default")
    }

    pub fn parse_schedule_time(&self) -> (u32, u32) {
        parse_hhmm_with_fallback(&self.schedule_time, (6, 0))
    }

    pub fn resolve_dream_shop(&self, bin_path: &str, server_id: u64) -> Option<DreamShopConfig> {
        let bin_name = bin_name_from_path(bin_path);
        self.role_dream_shop
            .get(&bin_name)
            .and_then(|role_map| role_map.get(&server_id))
            .cloned()
            .filter(|cfg| cfg.enabled && !cfg.purchase_list.is_empty())
    }

    /// get all users from group
    pub fn group_roles(&self, group: &str) -> Result<Vec<(String, u64)>, String> {
        self.role_groups.get(group)
            .cloned()
            .ok_or_else(|| format!("group '{}' not found", group))
    }

    /// get all roles from single bin (--force-all)
    pub fn all_roles(&self) -> Vec<(String, u64)> {
        self.bins.iter().flat_map(|b| {
            b.server_ids.iter().map(|sid| (b.path.clone(), *sid))
        }).collect()
    }

    pub fn daily_task_config(&self, bin_path: &str, server_id: u64) -> DailyTaskConfig {
        let (arena_team, arena_source) = self.resolve_formation_with_source(bin_path, server_id, FormationContext::Arena);
        let (boss_daily_team, boss_daily_source) = self.resolve_formation_with_source(bin_path, server_id, FormationContext::BossDaily);
        let (boss_legion_team, boss_legion_source) = self.resolve_formation_with_source(bin_path, server_id, FormationContext::BossLegion);
        let has_role_specific = arena_source == "role" || boss_daily_source == "role" || boss_legion_source == "role";
        DailyTaskConfig {
            formations: if has_role_specific {
                Some(DailyFormationPlan { arena: arena_team, boss_daily: boss_daily_team, boss_legion: boss_legion_team })
            } else { None },
            dream_shop: self.resolve_dream_shop(bin_path, server_id),
            car_enabled: self.car_enabled,
            gacha_enabled: self.gacha_enabled,
        }
    }

    pub fn parse_friday_daily_start_time(&self) -> (u32, u32) {
        parse_hhmm_with_fallback(&self.friday_daily_start_time, (12, 10))
    }

    pub fn parse_maintenance_windows(&self) -> Vec<(chrono::Weekday, u32, u32, u32, u32)> {
        let mut out = Vec::with_capacity(self.maintenance_windows.len());
        for w in &self.maintenance_windows {
            if let (Some(weekday), Some((sh, sm)), Some((eh, em))) = (
                parse_weekday(&w.weekday),
                parse_hhmm_strict(&w.start_time),
                parse_hhmm_strict(&w.end_time),
            ) {
                out.push((weekday, sh, sm, eh, em));
            }
        }
        out
    }

    fn normalize_maintenance_windows(&mut self) {
        if self.maintenance_windows.is_empty() {
            warn!(target: "config", "maintenance_windows is empty, fallback to defaults");
            self.maintenance_windows = default_maintenance_windows();
            return;
        }

        let mut grouped: HashMap<chrono::Weekday, Vec<(u32, u32)>> = HashMap::new();
        let mut dropped_invalid = 0usize;

        for w in &self.maintenance_windows {
            let weekday = match parse_weekday(&w.weekday) {
                Some(v) => v,
                None => {
                    dropped_invalid += 1;
                    warn!(target: "config", weekday = %w.weekday, "invalid maintenance window weekday, dropped");
                    continue;
                }
            };
            let (sh, sm) = match parse_hhmm_strict(&w.start_time) {
                Some(v) => v,
                None => {
                    dropped_invalid += 1;
                    warn!(target: "config", start_time = %w.start_time, "invalid maintenance window start_time, dropped");
                    continue;
                }
            };
            let (eh, em) = match parse_hhmm_strict(&w.end_time) {
                Some(v) => v,
                None => {
                    dropped_invalid += 1;
                    warn!(target: "config", end_time = %w.end_time, "invalid maintenance window end_time, dropped");
                    continue;
                }
            };
            let start = sh * 60 + sm;
            let end = eh * 60 + em;
            if start >= end {
                dropped_invalid += 1;
                warn!(target: "config", weekday = ?weekday, start_time = %w.start_time, end_time = %w.end_time, "invalid maintenance window (start >= end), dropped");
                continue;
            }
            grouped.entry(weekday).or_default().push((start, end));
        }

        let mut normalized = Vec::new();
        let mut merged_count = 0usize;
        let mut grouped_vec: Vec<_> = grouped.into_iter().collect();
        grouped_vec.sort_by_key(|(w, _)| w.num_days_from_monday());
        for (weekday, mut intervals) in grouped_vec {
            intervals.sort_by_key(|(s, _)| *s);
            let mut merged = Vec::new();
            for (start, end) in intervals {
                if let Some((_, last_end)) = merged.last_mut() {
                    if start <= *last_end {
                        if end > *last_end {
                            *last_end = end;
                        }
                        merged_count += 1;
                    } else {
                        merged.push((start, end));
                    }
                } else {
                    merged.push((start, end));
                }
            }
            for (start, end) in merged {
                normalized.push(MaintenanceWindowConfig {
                    weekday: weekday_to_short(weekday).to_string(),
                    start_time: minutes_to_hhmm(start),
                    end_time: minutes_to_hhmm(end),
                });
            }
        }

        if normalized.is_empty() {
            warn!(target: "config", "no valid maintenance windows after normalization, fallback to defaults");
            self.maintenance_windows = default_maintenance_windows();
            return;
        }

        if dropped_invalid > 0 || merged_count > 0 {
            warn!(target: "config", dropped_invalid, merged = merged_count, final_windows = normalized.len(), "maintenance windows normalized");
        }
        self.maintenance_windows = normalized;
    }
}

fn resolve_dream_shop_ref(
    dream_shop: DreamShopRef,
    presets: &HashMap<String, DreamShopConfig>,
) -> Option<DreamShopConfig> {
    match dream_shop {
        DreamShopRef::Inline(cfg) => Some(cfg),
        DreamShopRef::Preset(name) => presets.get(&name).cloned().or_else(|| {
            warn!(target: "config", preset = %name, "dream_shop preset not found, ignore role config");
            None
        }),
    }
}

impl FormationDefaults {
    fn resolve(&self, context: FormationContext) -> u64 {
        match context {
            FormationContext::Arena => self.arena,
            FormationContext::Tower => self.tower,
            FormationContext::Evotower => self.evotower,
            FormationContext::BossDaily => self.boss_daily,
            FormationContext::BossLegion => self.boss_legion,
        }
    }
}

impl FormationOverride {
    fn resolve(&self, context: FormationContext) -> Option<u64> {
        match context {
            FormationContext::Arena => self.arena,
            FormationContext::Tower => self.tower,
            FormationContext::Evotower => self.evotower,
            FormationContext::BossDaily => self.boss_daily,
            FormationContext::BossLegion => self.boss_legion,
        }
    }
}

fn parse_weekday(raw: &str) -> Option<chrono::Weekday> {
    match raw.to_ascii_lowercase().as_str() {
        "mon" | "monday" => Some(chrono::Weekday::Mon),
        "tue" | "tues" | "tuesday" => Some(chrono::Weekday::Tue),
        "wed" | "wednesday" => Some(chrono::Weekday::Wed),
        "thu" | "thur" | "thurs" | "thursday" => Some(chrono::Weekday::Thu),
        "fri" | "friday" => Some(chrono::Weekday::Fri),
        "sat" | "saturday" => Some(chrono::Weekday::Sat),
        "sun" | "sunday" => Some(chrono::Weekday::Sun),
        _ => None,
    }
}

fn weekday_to_short(w: chrono::Weekday) -> &'static str {
    match w {
        chrono::Weekday::Mon => "Mon",
        chrono::Weekday::Tue => "Tue",
        chrono::Weekday::Wed => "Wed",
        chrono::Weekday::Thu => "Thu",
        chrono::Weekday::Fri => "Fri",
        chrono::Weekday::Sat => "Sat",
        chrono::Weekday::Sun => "Sun",
    }
}

fn parse_hhmm_strict(raw: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let h = parts[0].parse::<u32>().ok()?;
    let m = parts[1].parse::<u32>().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some((h, m))
}

fn minutes_to_hhmm(total_mins: u32) -> String {
    let h = total_mins / 60;
    let m = total_mins % 60;
    format!("{:02}:{:02}", h, m)
}

fn parse_hhmm_with_fallback(raw: &str, fallback: (u32, u32)) -> (u32, u32) {
    parse_hhmm_strict(raw).unwrap_or(fallback)
}

fn normalize_dir(raw: &str) -> String {
    let mut s = raw.replace('\\', "/");
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    s
}

fn build_runtime_bin_path(default_bin_path: &str, bin: &str) -> String {
    let bin = normalize_path_for_id(bin);
    if bin.contains('/') {
        bin
    } else {
        format!("{}/{}", default_bin_path, bin)
    }
}

fn bin_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(path)
        .replace('\\', "/")
}

pub fn resolve_bin_path(config_path: &Path, raw_path: &str) -> PathBuf {
    let raw = Path::new(raw_path);
    if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        let base_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
        base_dir.join(raw)
    }
}

pub fn normalize_path_for_id(path: &str) -> String {
    let mut s = path.replace('\\', "/");
    while s.starts_with("./") {
        s = s[2..].to_string();
    }
    s
}

pub struct ConfigWatcher {
    path: PathBuf,
    last_modified: Option<SystemTime>,
    current: BatchConfig,
    bin_modified: HashMap<String, SystemTime>,
}

impl ConfigWatcher {
    pub fn new(path: PathBuf, config: BatchConfig) -> Self {
        let last_modified = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
        let mut bin_modified = HashMap::new();
        for bin in &config.bins {
            let resolved = resolve_bin_path(&path, &bin.path);
            let normalized = normalize_path_for_id(&bin.path);
            if let Ok(meta) = fs::metadata(&resolved) {
                if let Ok(t) = meta.modified() {
                    bin_modified.insert(normalized, t);
                }
            }
        }
        Self {
            path,
            last_modified,
            current: config,
            bin_modified,
        }
    }

    pub fn config(&self) -> &BatchConfig {
        &self.current
    }
    pub fn config_path(&self) -> &Path {
        &self.path
    }

    pub fn check_reload(&mut self) -> (bool, Vec<String>) {
        let mut config_changed = false;
        let mut changed_bins = Vec::new();

        if let Ok(meta) = fs::metadata(&self.path) {
            if let Ok(modified) = meta.modified() {
                if self.last_modified != Some(modified) {
                    match BatchConfig::load(&self.path) {
                        Ok(new_config) => {
                            self.last_modified = Some(modified);
                            self.current = new_config;
                            config_changed = true;
                        }
                        Err(e) => {
                            warn!(target: "config", error = %e, "config reload failed, keeping old config");
                        }
                    }
                }
            }
        }

        for bin in &self.current.bins {
            let resolved = resolve_bin_path(&self.path, &bin.path);
            let normalized = normalize_path_for_id(&bin.path);
            if let Ok(meta) = fs::metadata(&resolved) {
                if let Ok(modified) = meta.modified() {
                    let prev = self.bin_modified.get(&normalized);
                    if prev != Some(&modified) {
                        self.bin_modified.insert(normalized.clone(), modified);
                        changed_bins.push(normalized);
                    }
                }
            }
        }

        (config_changed, changed_bins)
    }
}

pub fn parse_server_id(server_id: u64) -> (u64, u32) {
    let (base, idx) = if server_id >= 2_000_000 {
        (server_id - 2_000_000, 2)
    } else if server_id >= 1_000_000 {
        (server_id - 1_000_000, 1)
    } else {
        (server_id, 0)
    };
    (base.saturating_sub(27), idx)
}

pub fn role_key(bin_path: &str, server_id: u64) -> String {
    format!("{}:{}", bin_path, server_id)
}

pub fn role_display(bin_path: &str, server_id: u64, role_name: &str) -> String {
    let filename = Path::new(bin_path)
        .file_stem()
        .and_then(|f| f.to_str())
        .unwrap_or(bin_path);
    let (server_num, idx) = parse_server_id(server_id);
    format!("{}/{}-{}-{}", filename, server_num, idx, role_name)
}
