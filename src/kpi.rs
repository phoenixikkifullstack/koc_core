use serde_json::{json, Value};
use chrono::{Datelike, Timelike};
use crate::config::DreamShopConfig;
use crate::websocket::WebSocketClient;
use tracing::{debug, info, warn};

// ============================================================
// RandomSeed constant (from randomSeed.ts)
// ============================================================
const XOR_A: u32 = 2118920861;
const XOR_B: u32 = 797788954;
const XOR_C: u32 = 1513922175;

/// daily BOSS ID map [Sun, Mon, Tue, Wed, Thu, Fri, Sat]
pub const DAY_BOSS_MAP: [u64; 7] = [9904, 9905, 9901, 9902, 9903, 9904, 9905];

const DREAM_MERCHANT_ITEMS: &[(u64, &[&str])] = &[
    (1, &["Advancement stone", "Refined iron", "Wooden chest", "Bronze chest", "Normal fishing rod", "Salted-Fish God ticket", "Salted-Fish God torch"]),
    (2, &["Nightmare crystal", "Advancement stone", "Refined iron", "Golden chest", "Golden fishing rod", "Recruitment order", "Orange general fragment", "Purple general fragment"]),
    (3, &["Nightmare crystal", "Platinum chest", "Golden fishing rod", "Recruitment order", "Red general fragment", "Orange general fragment", "Red general fragment", "Normal fishing rod"]),
];

// ============================================================
// constant timeout
// ============================================================
const T_DEFAULT: u64 = 8000;
const T_FIGHT: u64 = 12000;
const T_SKINC: u64 = 5000;
// Tower IDs encode each floor as floor * 10; floor 450 is the permanent cap.
const TOWER_CLEAR_ID: u64 = 4500;

/// encapsulation for WebSocket + game-protocol
pub struct GameClient {
    pub ws: WebSocketClient,
    pub role_info: Option<Value>,
    pub battle_version: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct DailyFormationPlan {
    pub arena: u64,
    pub boss_daily: u64,
    pub boss_legion: u64,
}

pub struct DailyTaskConfig {
    pub formations: Option<DailyFormationPlan>,
    pub dream_shop: Option<DreamShopConfig>,
    pub car_enabled: bool,
    pub gacha_enabled: bool,
}

impl GameClient {
    // ============================================================
    // Login
    // ============================================================

    /// whole login process:
    /// 1. connect WebSocket
    /// 2. send role_getroleinfo
    /// 3. calc&send randomSeed
    pub async fn login(token_json: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let url = match std::env::var("KOC_WS_BASE_URL") {
            Ok(base_url) => WebSocketClient::build_url_with_base(token_json, &base_url)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?,
            Err(_) => WebSocketClient::build_url(token_json),
        };
        info!(target: "game_client", "connecting to websocket");
        let ws = WebSocketClient::connect(&url).await?;
        info!(target: "game_client", "connected, requesting role info");

        let role_resp = ws.send_with_response("role_getroleinfo", Some(json!({
            "clientVersion": "2.21.2-fa918e1997301834-wx",
            "inviteUid": 0,
            "platform": "hortor",
            "platformExt": "mix",
            "scene": ""
        })), 10000).await?;

        info!(target: "game_client", "received role info response");

        // Extract battleVersion by calling fight_startlevel
        let battle_version = match ws.send_with_response("fight_startlevel", Some(json!({})), 8000).await {
            Ok(resp) => {
                let bv = resp.pointer("/battleData/version")
                    .and_then(|v| v.as_i64());
                if let Some(v) = bv {
                    info!(target: "game_client", battle_version = v, "battle version resolved");
                }
                bv
            }
            Err(_) => {
                // fallback: try from role_getroleinfo response
                role_resp.pointer("/battleVersion").and_then(|v| v.as_i64())
            }
        };

        let last_login = extract_last_login(&role_resp);
        if let Some(ts) = last_login {
            let seed = generate_random_seed(ts);
            info!(target: "game_client", random_seed = seed, login_time = ts, "sending randomSeed");
            ws.send("system_custom", Some(json!({
                "key": "randomSeed",
                "value": seed
            }))).await?;
        }

        Ok(Self { ws, role_info: Some(role_resp), battle_version })
    }

    /// sync cmd, wait for response
    pub async fn cmd(&self, cmd: &str, params: Value) -> Result<Value, String> {
        self.cmd_with_timeout(cmd, params, T_DEFAULT).await
    }

    async fn cmd_with_timeout(&self, cmd: &str, params: Value, timeout_ms: u64) -> Result<Value, String> {
        self.ws.send_with_response(cmd, Some(params), timeout_ms).await
    }

    /// async cmd, no-wait for response
    pub async fn cmd_fire(&self, cmd: &str, params: Value) -> Result<u32, String> {
        self.ws.send(cmd, Some(params)).await
    }

    /// fight (with battleVersion)
    async fn cmd_fight(&self, cmd: &str, mut params: Value) -> Result<Value, String> {
        if let Some(bv) = self.battle_version {
            if let Some(obj) = params.as_object_mut() {
                obj.insert("battleVersion".to_string(), json!(bv));
            }
        }
        self.ws.send_with_response(cmd, Some(params), T_FIGHT).await
    }

    pub async fn disconnect(&mut self) {
        self.ws.disconnect().await;
    }

    pub async fn current_team_id(&self) -> Result<u64, String> {
        let info = self.presetteam_getinfo().await?;
        let root = info.get("presetTeamInfo").unwrap_or(&info);
        Ok(root.get("useTeamId")
            .or_else(|| root.pointer("/presetTeamInfo/useTeamId"))
            .and_then(|v| v.as_u64())
            .or_else(|| find_use_team_id_rec(root))
            .unwrap_or(1))
    }

    pub async fn switch_team(&self, team_id: u64) -> Result<(), String> {
        self.presetteam_saveteam(team_id).await.map(|_| ())
    }

    // ============================================================
    // Hangup state & Smart hangup
    // ============================================================

    /// acquire hangup state from role_info
    pub fn get_hangup_status(&self) -> HangUpStatus {
        let info = match &self.role_info {
            Some(v) => v,
            None => return HangUpStatus::default(),
        };

        let hangup = info.pointer("/role/hangUp")
            .or_else(|| info.pointer("/hangUp"));

        let mut status = HangUpStatus::default();
        if let Some(obj) = hangup {
            status.last_time = obj.get("lastTime")
                .and_then(|v| v.as_f64()).unwrap_or(0.0);
            status.hangup_time = obj.get("hangUpTime")
                .and_then(|v| v.as_f64()).unwrap_or(0.0);

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64();
            let elapsed = now - status.last_time;
            if elapsed <= status.hangup_time {
                status.remaining = status.hangup_time - elapsed;
                status.elapsed = elapsed;
                status.is_active = true;
            } else {
                status.remaining = 0.0;
                status.elapsed = status.hangup_time;
                status.is_active = false;
            }
        }
        status
    }

    /// Smart Hangup:
    /// - approaching/alread toup → claim + extend 4 times
    pub async fn smart_hangup(&self, report: &mut DailyTaskReport) {
        let delay = || tokio::time::sleep(tokio::time::Duration::from_millis(500));
        let status = self.get_hangup_status();
        let one_hour: f64 = 3600.0;
        let eight_hours: f64 = 8.0 * 3600.0;

        info!(target: "task", hangup_elapsed_h = %format!("{:.1}", status.elapsed / 3600.0), hangup_limit_h = %format!("{:.1}", status.hangup_time / 3600.0), hangup_remaining_h = %format!("{:.1}", status.remaining / 3600.0), active = status.is_active, "hangup status");

        let need_claim = !status.is_active || status.remaining <= one_hour;

        // claim and extend
        if need_claim {
            if !status.is_active {
                info!(target: "task", "hangup expired, claim reward");
            } else {
                info!(target: "task", remaining_h = %format!("{:.1}", status.remaining / 3600.0), "hangup nearly full, claim reward");
            }
            report.run("Claim rewards for Hangup", self.system_claimhangupreward().await);
            for i in 1..=4 {
                report.run(&format!("Extend hangup time #{}/4", i), self.system_mysharecallback().await);
                delay().await;
            }
            return;
        }

        // extend only
        if status.hangup_time <= eight_hours {
            info!(target: "task", limit_h = %format!("{:.1}", status.hangup_time / 3600.0), "hangup limit <= 8h, extend only");
            for i in 1..=4 {
                report.run(&format!("Extend time #{}/4", i), self.system_mysharecallback().await);
                delay().await;
            }
            return;
        }

        info!(target: "task", elapsed_h = %format!("{:.1}", status.elapsed / 3600.0), remaining_h = %format!("{:.1}", status.remaining / 3600.0), "hangup accumulating, skip");
    }

    // ============================================================
    // bottle & smart bottle
    // ============================================================

    /// acquire bottle state from role_info
    pub fn get_bottle_status(&self) -> BottleStatus {
        let info = match &self.role_info {
            Some(v) => v,
            None => return BottleStatus::default(),
        };

        let helpers = info.pointer("/role/bottleHelpers")
            .or_else(|| info.pointer("/bottleHelpers"));

        let mut status = BottleStatus::default();
        if let Some(obj) = helpers {
            status.stop_time = obj.get("helperStopTime")
                .and_then(|v| v.as_f64()).unwrap_or(0.0);

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64();

            if status.stop_time > now {
                status.is_running = true;
                status.remaining = status.stop_time - now;
            } else {
                status.is_running = false;
                status.remaining = 0.0;
            }
        }
        status
    }

    /// Smart bottle: stop & start
    pub async fn smart_bottle(&self, report: &mut DailyTaskReport) {
        let delay = || tokio::time::sleep(tokio::time::Duration::from_millis(500));
        let status = self.get_bottle_status();

        info!(target: "task", running = status.is_running, remaining = %BottleStatus::fmt_time(status.remaining), "bottle status");

        report.run("STOP Bottle", self.bottlehelper_stop().await);
        delay().await;
        report.run("START Bottle", self.bottlehelper_start().await);
        delay().await;
    }

    // ============================================================
    // System/login (system_*)
    // ============================================================

    /// role_getroleinfo
    pub async fn role_getroleinfo(&self) -> Result<Value, String> {
        self.cmd("role_getroleinfo", json!({
            "clientVersion": "2.21.2-fa918e1997301834-wx",
            "inviteUid": 0, "platform": "hortor", "platformExt": "mix", "scene": ""
        })).await
    }

    /// system_getdatabundlever
    pub async fn system_getdatabundlever(&self) -> Result<Value, String> {
        self.cmd("system_getdatabundlever", json!({"isAudit": false})).await
    }

    /// system_custom
    /// - key: self defined
    /// - value: self defined
    pub async fn system_custom(&self, key: &str, value: Value) -> Result<Value, String> {
        self.cmd("system_custom", json!({"key": key, "value": value})).await
    }

    /// system_signinreward
    pub async fn system_signinreward(&self) -> Result<Value, String> {
        self.cmd("system_signinreward", json!({})).await
    }

    /// system_buygold (3 times/day)
    pub async fn system_buygold(&self) -> Result<Value, String> {
        self.cmd("system_buygold", json!({"buyNum": 1})).await
    }

    /// system_claimhangupreward
    /// it's better for call @system_mysharecallback first
    pub async fn system_claimhangupreward(&self) -> Result<Value, String> {
        self.cmd("system_claimhangupreward", json!({})).await
    }

    /// system_mysharecallback
    pub async fn system_mysharecallback(&self) -> Result<Value, String> {
        self.cmd("system_mysharecallback", json!({"isSkipShareCard": true, "type": 2})).await
    }

    /// system_sendchatmessage
    /// - need params (channel, contents...)
    pub async fn system_sendchatmessage(&self, params: Value) -> Result<Value, String> {
        self.cmd("system_sendchatmessage", params).await
    }

    /// role_commitpassword
    /// - password
    /// - password_type: type (default 1)
    pub async fn role_commitpassword(&self, password: &str, password_type: i32) -> Result<Value, String> {
        self.cmd("role_commitpassword", json!({
            "password": password, "passwordType": password_type
        })).await
    }

    // ============================================================
    // friend (friend_*)
    // ============================================================

    /// send gold for friend_batch
    pub async fn friend_batch(&self) -> Result<Value, String> {
        self.cmd("friend_batch", json!({"friendId": 0})).await
    }

    // ============================================================
    // hero (hero_*)
    // ============================================================

    /// Hero Recruit
    /// - recruit_type: 3=free, 1=non-free
    /// - recruit_number: numbers (default 1)
    pub async fn hero_recruit(&self, recruit_type: u32, recruit_number: u32) -> Result<Value, String> {
        self.cmd("hero_recruit", json!({
            "byClub": false, "recruitNumber": recruit_number, "recruitType": recruit_type
        })).await
    }

    /// hero_exchange
    pub async fn hero_exchange(&self, params: Value) -> Result<Value, String> {
        self.cmd("hero_exchange", params).await
    }

    /// hero_gointobattle
    pub async fn hero_gointobattle(&self, params: Value) -> Result<Value, String> {
        self.cmd("hero_gointobattle", params).await
    }

    /// hero_gobackbattle
    pub async fn hero_gobackbattle(&self, params: Value) -> Result<Value, String> {
        self.cmd("hero_gobackbattle", params).await
    }

    /// hero_heroupgradelevel
    pub async fn hero_heroupgradelevel(&self, params: Value) -> Result<Value, String> {
        self.cmd("hero_heroupgradelevel", params).await
    }

    /// hero_heroupgradeorder
    pub async fn hero_heroupgradeorder(&self, params: Value) -> Result<Value, String> {
        self.cmd("hero_heroupgradeorder", params).await
    }

    /// hero_heroupgradestar
    pub async fn hero_heroupgradestar(&self, params: Value) -> Result<Value, String> {
        self.cmd("hero_heroupgradestar", params).await
    }

    /// hero_rebirth
    pub async fn hero_rebirth(&self, params: Value) -> Result<Value, String> {
        self.cmd("hero_rebirth", params).await
    }

    // ============================================================
    // cargo/box (item_*)
    // ============================================================

    /// item_openbox
    /// - item_id
    /// - number
    pub async fn item_openbox(&self, item_id: u64, number: u64) -> Result<Value, String> {
        self.cmd("item_openbox", json!({"itemId": item_id, "number": number})).await
    }

    /// item_batchclaimboxpointreward
    pub async fn item_batchclaimboxpointreward(&self) -> Result<Value, String> {
        self.cmd("item_batchclaimboxpointreward", json!({})).await
    }

    /// item_openpack
    pub async fn item_openpack(&self, params: Value) -> Result<Value, String> {
        self.cmd("item_openpack", params).await
    }

    // ============================================================
    // arena (arena_*)
    // ============================================================

    /// arena_startarea
    pub async fn arena_startarea(&self) -> Result<Value, String> {
        self.cmd("arena_startarea", json!({})).await
    }

    /// arena_getareatarget
    pub async fn arena_getareatarget(&self) -> Result<Value, String> {
        self.cmd("arena_getareatarget", json!({"refresh": false})).await
    }

    /// arena_getarearank
    pub async fn arena_getarearank(&self) -> Result<Value, String> {
        self.cmd("arena_getarearank", json!({})).await
    }

    // ============================================================
    // Fight (fight_*)
    // Be attention: battleVersion is needed in all fight cmd
    // ============================================================

    /// fight_startareaarena
    /// - target_id: roleId
    /// depends: arena_startarea -> arena_getareatarget
    ///        response: roleList[0].roleId is target_id
    pub async fn fight_startareaarena(&self, target_id: u64) -> Result<Value, String> {
        self.cmd_fight("fight_startareaarena", json!({"targetId": target_id})).await
    }

    /// fight_startboss
    /// - boss_id: BOSS ID (see DAY_BOSS_MAP)
    pub async fn fight_startboss(&self, boss_id: u64) -> Result<Value, String> {
        self.cmd_fight("fight_startboss", json!({"bossId": boss_id})).await
    }

    /// fight_startlegionboss
    /// depends: presetteam_saveteam first
    pub async fn fight_startlegionboss(&self) -> Result<Value, String> {
        self.cmd_fight("fight_startlegionboss", json!({})).await
    }

    /// PVP
    pub async fn fight_startpvp(&self, params: Value) -> Result<Value, String> {
        self.cmd_fight("fight_startpvp", params).await
    }

    /// fight_starttower
    pub async fn fight_starttower(&self) -> Result<Value, String> {
        self.cmd_fight("fight_starttower", json!({})).await
    }

    /// fight_startdungeon
    pub async fn fight_startdungeon(&self, params: Value) -> Result<Value, String> {
        self.cmd_fight("fight_startdungeon", params).await
    }

    /// fight_startlevel
    pub async fn fight_startlevel(&self, params: Value) -> Result<Value, String> {
        self.cmd_fight("fight_startlevel", params).await
    }

    // ============================================================
    // TASK (task_*)
    // ============================================================

    /// Claim daily task point rewards.
    /// - task_id: task ID (1-10)
    /// Prerequisite: the corresponding task must be complete before claiming.
    pub async fn task_claimdailypoint(&self, task_id: u64) -> Result<Value, String> {
        self.cmd("task_claimdailypoint", json!({"taskId": task_id})).await
    }

    /// Claim the daily task completion reward.
    /// Prerequisite: sufficient daily task points are required.
    pub async fn task_claimdailyreward(&self) -> Result<Value, String> {
        self.cmd("task_claimdailyreward", json!({"rewardId": 0})).await
    }

    /// Claim the weekly task completion reward.
    pub async fn task_claimweekreward(&self) -> Result<Value, String> {
        self.cmd("task_claimweekreward", json!({"rewardId": 0})).await
    }

    // ============================================================
    // Store (store_*)
    // ============================================================

    /// Get the store goods list.
    /// - store_id: store ID
    pub async fn store_goodslist(&self, store_id: u64) -> Result<Value, String> {
        self.cmd("store_goodslist", json!({"storeId": store_id})).await
    }

    /// Buy from the store.
    /// - goods_id: goods ID
    pub async fn store_buy(&self, goods_id: u64) -> Result<Value, String> {
        self.cmd("store_buy", json!({"goodsId": goods_id})).await
    }

    /// Purchase from the Black Market.
    /// - goods_id: goods ID
    pub async fn store_purchase(&self, goods_id: u64) -> Result<Value, String> {
        self.cmd("store_purchase", json!({"goodsId": goods_id})).await
    }

    /// Refresh the store.
    /// - store_id: store ID
    pub async fn store_refresh(&self, store_id: u64) -> Result<Value, String> {
        self.cmd("store_refresh", json!({"storeId": store_id})).await
    }

    // ============================================================
    // Legion (legion_*)
    // ============================================================

    /// Legion check-in.
    pub async fn legion_signin(&self) -> Result<Value, String> {
        self.cmd("legion_signin", json!({})).await
    }

    /// Get legion information.
    pub async fn legion_getinfo(&self) -> Result<Value, String> {
        self.cmd("legion_getinfo", json!({})).await
    }

    /// Get legion information by ID.
    pub async fn legion_getinfobyid(&self, params: Value) -> Result<Value, String> {
        self.cmd("legion_getinfobyid", params).await
    }

    /// Get legion-war rankings.
    pub async fn legion_getwarrank(&self) -> Result<Value, String> {
        self.cmd("legion_getwarrank", json!({})).await
    }

    /// Get legion-war details.
    pub async fn legionwar_getdetails(&self) -> Result<Value, String> {
        self.cmd("legionwar_getdetails", json!({})).await
    }

    /// Buy from the legion store, such as Four Sacred Beasts fragments.
    pub async fn legion_storebuygoods(&self, params: Value) -> Result<Value, String> {
        self.cmd("legion_storebuygoods", params).await
    }

    /// Remove a legion member.
    pub async fn legion_kickout(&self, params: Value) -> Result<Value, String> {
        self.cmd("legion_kickout", params).await
    }

    /// Get the legion application list.
    pub async fn legion_applylist(&self) -> Result<Value, String> {
        self.cmd("legion_applylist", json!({})).await
    }

    /// Approve a legion application.
    pub async fn legion_approveapply(&self, params: Value) -> Result<Value, String> {
        self.cmd("legion_approveapply", params).await
    }

    /// Reject a legion application.
    pub async fn legion_refuseapply(&self, params: Value) -> Result<Value, String> {
        self.cmd("legion_refuseapply", params).await
    }

    /// Accept a legion application.
    pub async fn legion_agree(&self, params: Value) -> Result<Value, String> {
        self.cmd("legion_agree", params).await
    }

    /// Ignore a legion application.
    pub async fn legion_ignore(&self, params: Value) -> Result<Value, String> {
        self.cmd("legion_ignore", params).await
    }

    /// Research legion technology.
    pub async fn legion_research(&self, params: Value) -> Result<Value, String> {
        self.cmd("legion_research", params).await
    }

    /// Reset legion technology research.
    pub async fn legion_resetresearch(&self, params: Value) -> Result<Value, String> {
        self.cmd("legion_resetresearch", params).await
    }

    /// Get legion regional rankings.
    pub async fn legion_getarearank(&self) -> Result<Value, String> {
        self.cmd("legion_getarearank", json!({})).await
    }

    /// Get legion opponent information.
    pub async fn legion_getopponent(&self) -> Result<Value, String> {
        self.cmd("legion_getopponent", json!({})).await
    }

    /// Get legion battlefield information.
    pub async fn legion_getbattlefield(&self) -> Result<Value, String> {
        self.cmd("legion_getbattlefield", json!({})).await
    }

    /// Register for the Salt Field event.
    pub async fn legion_signup(&self) -> Result<Value, String> {
        self.cmd("legion_signup", json!({})).await
    }

    /// Get Legion Golden Month War rankings.
    pub async fn legionwar_getgoldmonthwarrank(&self) -> Result<Value, String> {
        self.cmd("legionwar_getgoldmonthwarrank", json!({})).await
    }

    /// Register a character for legion matching.
    pub async fn legionmatch_rolesignup(&self) -> Result<Value, String> {
        self.cmd("legionmatch_rolesignup", json!({})).await
    }

    // ============================================================
    // Peach/Payload (legion_payload*)
    // ============================================================

    /// Claim Peach task rewards.
    pub async fn legion_claimpayloadtask(&self, params: Value) -> Result<Value, String> {
        self.cmd("legion_claimpayloadtask", params).await
    }

    /// Claim Peach task progress rewards.
    pub async fn legion_claimpayloadtaskprogress(&self, params: Value) -> Result<Value, String> {
        self.cmd("legion_claimpayloadtaskprogress", params).await
    }

    /// Get Peach task information.
    pub async fn legion_getpayloadtask(&self) -> Result<Value, String> {
        self.cmd("legion_getpayloadtask", json!({})).await
    }

    /// Get Peach kill records.
    pub async fn legion_getpayloadkillrecord(&self) -> Result<Value, String> {
        self.cmd("legion_getpayloadkillrecord", json!({})).await
    }

    /// Get Peach battlefield information.
    pub async fn legion_getpayloadbf(&self) -> Result<Value, String> {
        self.cmd("legion_getpayloadbf", json!({})).await
    }

    /// Get Peach battle records.
    pub async fn legion_getpayloadrecord(&self) -> Result<Value, String> {
        self.cmd("legion_getpayloadrecord", json!({})).await
    }

    /// Register for the Peach event.
    pub async fn legion_payloadsignup(&self) -> Result<Value, String> {
        self.cmd("legion_payloadsignup", json!({})).await
    }

    // ============================================================
    // Salt Road/Salt Field (saltroad_*)
    // ============================================================

    /// Get overall Salt Road War rankings.
    pub async fn saltroad_getsaltroadwartotalrank(&self) -> Result<Value, String> {
        self.cmd("saltroad_getsaltroadwartotalrank", json!({})).await
    }

    /// Get Salt Road War group rankings.
    pub async fn saltroad_getsaltroadwargrouprank(&self) -> Result<Value, String> {
        self.cmd("saltroad_getsaltroadwargrouprank", json!({})).await
    }

    /// Get the Salt Road War type.
    pub async fn saltroad_getwartype(&self) -> Result<Value, String> {
        self.cmd("saltroad_getwartype", json!({})).await
    }

    // ============================================================
    // League (league_*)
    // ============================================================

    /// Get league battlefield information.
    pub async fn league_getbattlefield(&self) -> Result<Value, String> {
        self.cmd("league_getbattlefield", json!({})).await
    }

    /// Get league group opponents.
    pub async fn league_getgroupopponent(&self) -> Result<Value, String> {
        self.cmd("league_getgroupopponent", json!({})).await
    }

    // ============================================================
    // Mail (mail_*)
    // ============================================================

    /// Get the mail list.
    pub async fn mail_getlist(&self) -> Result<Value, String> {
        self.cmd("mail_getlist", json!({"category": [0, 4, 5], "lastId": 0, "size": 60})).await
    }

    /// Claim all mail attachments.
    /// - category: mail category (default: 0 = all)
    pub async fn mail_claimallattachment(&self, category: i32) -> Result<Value, String> {
        self.cmd("mail_claimallattachment", json!({"category": category})).await
    }

    /// Get detailed mail-material information.
    pub async fn mail_getmtlinfo(&self) -> Result<Value, String> {
        self.cmd("mail_getmtlinfo", json!({})).await
    }

    /// Get summarized mail-material information.
    pub async fn mail_getmtlshortinfo(&self) -> Result<Value, String> {
        self.cmd("mail_getmtlshortinfo", json!({})).await
    }

    // ============================================================
    // Quiz/Study (study_*)
    // ============================================================

    /// Start the quiz game.
    pub async fn study_startgame(&self) -> Result<Value, String> {
        self.cmd("study_startgame", json!({})).await
    }

    /// Submit an answer.
    pub async fn study_answer(&self, params: Value) -> Result<Value, String> {
        self.cmd("study_answer", params).await
    }

    /// Claim quiz rewards.
    /// - reward_id: reward ID
    pub async fn study_claimreward(&self, reward_id: u64) -> Result<Value, String> {
        self.cmd("study_claimreward", json!({"rewardId": reward_id})).await
    }

    // ============================================================
    // Artifacts/Fishing (artifact_*)
    // ============================================================

    /// Free fishing/artifact draw (three times daily).
    pub async fn artifact_lottery(&self) -> Result<Value, String> {
        self.cmd("artifact_lottery", json!({"lotteryNumber": 1, "newFree": true, "type": 1})).await
    }

    /// Exchange fishing items.
    pub async fn artifact_exchange(&self, params: Value) -> Result<Value, String> {
        self.cmd("artifact_exchange", params).await
    }

    /// Equip an artifact.
    pub async fn artifact_load(&self, params: Value) -> Result<Value, String> {
        self.cmd("artifact_load", params).await
    }

    /// Unequip an artifact.
    pub async fn artifact_unload(&self, params: Value) -> Result<Value, String> {
        self.cmd("artifact_unload", params).await
    }

    // ============================================================
    // Genie (genie_*)
    // ============================================================

    /// Sweep a Genie stage.
    /// - genie_id: 1 = Wei, 2 = Shu, 3 = Wu, 4 = Qun, 5 = Deep Sea
    pub async fn genie_sweep(&self, genie_id: u64) -> Result<Value, String> {
        self.cmd("genie_sweep", json!({"genieId": genie_id})).await
    }

    /// Claim or buy free sweep vouchers (three times daily).
    pub async fn genie_buysweep(&self) -> Result<Value, String> {
        self.cmd("genie_buysweep", json!({})).await
    }

    /// Free capsule draw.
    pub async fn gacha_drawreward(&self) -> Result<Value, String> {
        self.cmd("gacha_drawreward", json!({"num": 1, "isGroup": false})).await
    }

    // ============================================================
    // Discounts/Gift Packs (discount_*)
    // ============================================================

    /// Claim the daily discount gift pack.
    pub async fn discount_claimreward(&self) -> Result<Value, String> {
        self.cmd("discount_claimreward", json!({"discountId": 1})).await
    }

    /// Get discount information.
    pub async fn discount_getdiscountinfo(&self) -> Result<Value, String> {
        self.cmd("discount_getdiscountinfo", json!({})).await
    }

    // ============================================================
    // Treasure Pavilion (collection_*)
    // ============================================================

    /// Claim the Treasure Pavilion daily free reward.
    pub async fn collection_claimfreereward(&self) -> Result<Value, String> {
        self.cmd("collection_claimfreereward", json!({})).await
    }

    /// Get the Treasure Pavilion goods list.
    pub async fn collection_goodslist(&self) -> Result<Value, String> {
        self.cmd("collection_goodslist", json!({})).await
    }

    // ============================================================
    // Cards/Gift Cards (card_*)
    // ============================================================

    /// Claim card gift-pack rewards.
    /// Free gift pack (the JavaScript default is cardId = 1).
    pub async fn card_claimfree(&self) -> Result<Value, String> {
        self.cmd("card_claimreward", json!({"cardId": 1})).await
    }

    /// Claim card gift-pack rewards.
    /// - card_id: card ID (4003 = permanent card)
    pub async fn card_claimreward(&self, card_id: u64) -> Result<Value, String> {
        self.cmd("card_claimreward", json!({"cardId": card_id})).await
    }

    // ============================================================
    // Salt Jar/Drift Bottle (bottlehelper_*)
    // ============================================================

    /// Claim Salt Jar rewards.
    pub async fn bottlehelper_claim(&self) -> Result<Value, String> {
        self.cmd("bottlehelper_claim", json!({})).await
    }

    /// Start the Salt Jar bot.
    pub async fn bottlehelper_start(&self) -> Result<Value, String> {
        self.cmd("bottlehelper_start", json!({"bottleType": -1})).await
    }

    /// Stop the Salt Jar bot.
    pub async fn bottlehelper_stop(&self) -> Result<Value, String> {
        self.cmd("bottlehelper_stop", json!({"bottleType": -1})).await
    }

    // ============================================================
    // Salted-Fish General Tower (tower_*)
    // ============================================================

    /// Get Salted-Fish General Tower information.
    pub async fn tower_getinfo(&self) -> Result<Value, String> {
        self.cmd("tower_getinfo", json!({})).await
    }

    /// Claim Salted-Fish General Tower clear rewards.
    /// - reward_id: reward floor ID
    pub async fn tower_claimreward(&self, reward_id: u64) -> Result<Value, String> {
        self.cmd("tower_claimreward", json!({"rewardId": reward_id})).await
    }

    // ============================================================
    // Skin Challenge (towers_*)
    // ============================================================

    /// Get the Skin Challenge's dynamic event ID.
    /// Find the most recent Friday, including today if it is Friday, formatted as YYMMDD1.
    fn get_tower_act_id() -> u64 {
        use chrono::Datelike;
        let now = chrono::Local::now().date_naive();
        // num_days_from_monday: Mon=0, Tue=1, Wed=2, Thu=3, Fri=4, Sat=5, Sun=6
        let days_since_friday = (now.weekday().num_days_from_monday() + 7 - chrono::Weekday::Fri.num_days_from_monday()) % 7;
        let cycle_friday = now - chrono::Duration::days(days_since_friday as i64);

        let year = cycle_friday.year() % 100;
        let month = cycle_friday.month();
        let day = cycle_friday.day();

        let id_str = format!("{:02}{:02}{:02}1", year, month, day);
        id_str.parse().unwrap_or(0)
    }

    /// Get Skin Challenge information.
    pub async fn towers_getinfo(&self) -> Result<Value, String> {
        self.cmd_with_timeout("towers_getinfo", json!({"actId": Self::get_tower_act_id()}), T_SKINC).await
    }

    /// Start the Skin Challenge.
    pub async fn towers_start(&self, mut params: Value) -> Result<Value, String> {
        if let Some(obj) = params.as_object_mut() {
            obj.insert("actId".to_string(), json!(Self::get_tower_act_id()));
        }
        self.cmd_with_timeout("towers_start", params, T_SKINC).await
    }

    /// Fight in the Skin Challenge.
    pub async fn towers_fight(&self, mut params: Value) -> Result<Value, String> {
        if let Some(obj) = params.as_object_mut() {
            obj.insert("actId".to_string(), json!(Self::get_tower_act_id()));
        }
        self.cmd_with_timeout("towers_fight", params, T_SKINC).await
    }

    // ============================================================
    // Evo Tower (evotower_*)
    // ============================================================

    /// Get Evo Tower information.
    pub async fn evotower_getinfo(&self) -> Result<Value, String> {
        self.cmd("evotower_getinfo", json!({})).await
    }

    /// Fight in Evo Tower.
    pub async fn evotower_fight(&self, params: Value) -> Result<Value, String> {
        self.cmd("evotower_fight", params).await
    }

    /// Get legion members participating in Evo Tower.
    pub async fn evotower_getlegionjoinmembers(&self) -> Result<Value, String> {
        self.cmd("evotower_getlegionjoinmembers", json!({})).await
    }

    /// Prepare an Evo Tower battle.
    pub async fn evotower_readyfight(&self, params: Value) -> Result<Value, String> {
        self.cmd("evotower_readyfight", params).await
    }

    /// Claim Evo Tower rewards.
    pub async fn evotower_claimreward(&self, params: Value) -> Result<Value, String> {
        self.cmd("evotower_claimreward", params).await
    }

    /// Claim Evo Tower task rewards.
    /// - task_id: task ID
    pub async fn evotower_claimtask(&self, task_id: u64) -> Result<Value, String> {
        self.cmd("evotower_claimtask", json!({"taskId": task_id})).await
    }

    // ============================================================
    // Salted-Fish King Treasury (bosstower_*)
    // ============================================================

    /// Get Salted-Fish King Treasury information.
    pub async fn bosstower_getinfo(&self) -> Result<Value, String> {
        self.cmd("bosstower_getinfo", json!({})).await
    }

    /// Start a Salted-Fish King Treasury boss battle.
    pub async fn bosstower_startboss(&self, params: Value) -> Result<Value, String> {
        self.cmd("bosstower_startboss", params).await
    }

    /// Open a Salted-Fish King Treasury chest.
    pub async fn bosstower_startbox(&self, params: Value) -> Result<Value, String> {
        self.cmd("bosstower_startbox", params).await
    }

    /// Get Salted-Fish King Treasury support-battle rankings.
    pub async fn bosstower_gethelprank(&self) -> Result<Value, String> {
        self.cmd("bosstower_gethelprank", json!({})).await
    }

    // ============================================================
    // Merge Box (mergebox_*)
    // ============================================================

    /// Get Merge Box information.
    /// - act_type: activity type (usually 1 for Evo Tower)
    pub async fn mergebox_getinfo(&self, act_type: u32) -> Result<Value, String> {
        self.cmd("mergebox_getinfo", json!({"actType": act_type})).await
    }

    /// Claim free Merge Box energy.
    /// - act_type: activity type (usually 1 for Evo Tower)
    pub async fn mergebox_claimfreeenergy(&self, act_type: u32) -> Result<Value, String> {
        self.cmd("mergebox_claimfreeenergy", json!({"actType": act_type})).await
    }

    /// Open a Merge Box.
    pub async fn mergebox_openbox(&self, params: Value) -> Result<Value, String> {
        self.cmd("mergebox_openbox", params).await
    }

    /// Automatically merge Merge Box items.
    /// - act_type: activity type (default: 1)
    pub async fn mergebox_automergeitem(&self, act_type: u32) -> Result<Value, String> {
        self.cmd("mergebox_automergeitem", json!({"actType": act_type})).await
    }

    /// Manually merge Merge Box items.
    pub async fn mergebox_mergeitem(&self, params: Value) -> Result<Value, String> {
        self.cmd("mergebox_mergeitem", params).await
    }

    /// Claim Merge Box consumption-progress rewards.
    pub async fn mergebox_claimcostprogress(&self, act_type: u32) -> Result<Value, String> {
        self.cmd("mergebox_claimcostprogress", json!({"actType": act_type})).await
    }

    /// Claim Merge Box merge-progress rewards.
    /// - task_id: task ID
    pub async fn mergebox_claimmergeprogress(&self, act_type: u32, task_id: u64) -> Result<Value, String> {
        self.cmd("mergebox_claimmergeprogress", json!({"actType": act_type, "taskId": task_id})).await
    }

    // ============================================================
    // Preset Formations (presetteam_*)
    // ============================================================

    /// Get preset formation information.
    pub async fn presetteam_getinfo(&self) -> Result<Value, String> {
        self.cmd("presetteam_getinfo", json!({})).await
    }

    /// Save or switch the preset formation.
    /// - team_id: formation ID
    /// Switch before battle and restore the original formation afterward.
    pub async fn presetteam_saveteam(&self, team_id: u64) -> Result<Value, String> {
        self.cmd("presetteam_saveteam", json!({"teamId": team_id})).await
    }

    /// Set a preset formation.
    pub async fn presetteam_setteam(&self, params: Value) -> Result<Value, String> {
        self.cmd("presetteam_setteam", params).await
    }

    // ============================================================
    // Rankings (rank_*)
    // ============================================================

    /// Get server rankings.
    pub async fn rank_getserverrank(&self) -> Result<Value, String> {
        self.cmd("rank_getserverrank", json!({})).await
    }

    /// Get role information from the rankings.
    pub async fn rank_getroleinfo(&self, params: Value) -> Result<Value, String> {
        self.cmd("rank_getroleinfo", params).await
    }

    /// Get target role team information.
    pub async fn role_gettargetteam(&self, params: Value) -> Result<Value, String> {
        self.cmd("role_gettargetteam", params).await
    }

    // ============================================================
    // Nightmare/Dungeon (dungeon_*, nightmare_*)
    // ============================================================

    /// Get Nightmare role information.
    pub async fn nightmare_getroleinfo(&self) -> Result<Value, String> {
        self.cmd("nightmare_getroleinfo", json!({})).await
    }

    /// Select a hero for Salted-Fish King Dreamscape.
    /// - Requires a custom battleTeam parameter, for example {"battleTeam": {0: 107}}.
    pub async fn dungeon_selecthero(&self, params: Value) -> Result<Value, String> {
        self.cmd("dungeon_selecthero", params).await
    }

    /// Buy merchant items in the dungeon.
    pub async fn dungeon_buymerchant(&self, params: Value) -> Result<Value, String> {
        self.cmd("dungeon_buymerchant", params).await
    }

    // ============================================================
    // Vehicles/Dispatch (car_*)
    // ============================================================

    /// Get role vehicle information.
    pub async fn car_getrolecar(&self) -> Result<Value, String> {
        self.cmd("car_getrolecar", json!({})).await
    }

    /// Refresh a vehicle.
    pub async fn car_refresh(&self, car_id: &str) -> Result<Value, String> {
        self.cmd("car_refresh", json!({"carId": car_id})).await
    }

    /// Claim vehicle rewards.
    pub async fn car_claim(&self, car_id: &str) -> Result<Value, String> {
        self.cmd("car_claim", json!({"carId": car_id})).await
    }

    /// Dispatch a vehicle.
    pub async fn car_send(&self, car_id: &str, helper_id: u64, text: &str) -> Result<Value, String> {
        self.cmd("car_send", json!({"carId": car_id, "helperId": helper_id, "text": text, "isUpgrade": false})).await
    }

    /// Get member assistance counts.
    pub async fn car_getmemberhelpingcnt(&self) -> Result<Value, String> {
        self.cmd("car_getmemberhelpingcnt", json!({})).await
    }

    /// Get vehicle member rankings.
    pub async fn car_getmemberrank(&self) -> Result<Value, String> {
        self.cmd("car_getmemberrank", json!({})).await
    }

    /// Research or upgrade vehicles.
    pub async fn car_research(&self, params: Value) -> Result<Value, String> {
        self.cmd("car_research", params).await
    }

    /// Claim vehicle-part consumption rewards.
    pub async fn car_claimpartconsumereward(&self) -> Result<Value, String> {
        self.cmd("car_claimpartconsumereward", json!({})).await
    }

    // ============================================================
    // Vehicle helper functions
    // ============================================================
    const BIG_PRIZES: &[(i32, i32, u32)] = &[
        (3, 3201, 10),   // Red universal fragments >= 10
        (3, 1001, 10),    // Recruit Orders >= 10
        (3, 1022, 2000),  // White Jade >= 2000
        (2, 0, 2000),     // Gold ingots >= 2000
        (3, 1023, 5),     // Colored Jade >= 5
        (3, 1022, 2500),  // White Jade >= 2500
        (3, 1001, 12),    // Recruit Orders >= 12
    ];

    const CAR_RESEARCH_COSTS: &[u32] = &[
        20, 21, 22, 23, 24, 26, 28, 30, 32, 34, 36, 38, 40, 42, 44, 47, 50, 53, 56,
        59, 62, 65, 68, 71, 74, 78, 82, 86, 90, 94, 99, 104, 109, 114, 119, 126, 133,
        140, 147, 154, 163, 172, 181, 190, 199, 210, 221, 232, 243, 369, 393, 422,
        457, 498, 548, 607, 678, 763, 865, 1011,
    ];

    pub fn grade_label(color: u32) -> &'static str {
        match color {
            1 => "Green · Common",
            2 => "Blue · Rare",
            3 => "Purple · Epic",
            4 => "Orange · Legend",
            5 => "Red · Mythic",
            6 => "Gold · Legendary",
            _ => "Unknown",
        }
    }

    pub fn is_big_prize(rewards: &[Value]) -> bool {
        rewards.iter().any(|r| {
            let r_type = r.get("type").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let r_item = r.get("itemId").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let r_value = r.get("value").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            Self::BIG_PRIZES.iter().any(|(p_type, p_item, p_value)| {
                r_type == *p_type && r_item == *p_item && r_value >= *p_value
            })
        })
    }

    pub fn has_refresh_ticket(rewards: &[Value]) -> bool {
        rewards.iter().any(|r| {
            r.get("type").and_then(|v| v.as_i64()) == Some(3)
                && r.get("itemId").and_then(|v| v.as_i64()) == Some(35002)
                && r.get("value").and_then(|v| v.as_u64()).unwrap_or(0) > 0
        })
    }

    pub fn normalize_car_list(resp: &Value) -> Vec<(String, u32, u64, u32, Vec<Value>, u64)> {
        let root = resp.get("body").unwrap_or(resp);
        let role_car = root.get("roleCar").or_else(|| root.get("rolecar")).unwrap_or(&Value::Null);

        let car_map = role_car.get("carDataMap").or_else(|| role_car.get("cardatamap"));
        if let Some(map) = car_map.and_then(|v| v.as_object()) {
            return map.iter().map(|(id, info)| {
                let car_id = id.clone();
                let color = info.get("color").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let send_at = info.get("sendAt").and_then(|v| v.as_u64()).unwrap_or(0);
                let refresh_count = info.get("refreshCount").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let rewards = info.get("rewards").and_then(|v| v.as_array())
                    .map(|a| a.clone()).unwrap_or_default();
                let helper_id = info.get("helperId").and_then(|v| v.as_u64()).unwrap_or(0);
                (car_id, color, send_at, refresh_count, rewards, helper_id)
            }).collect();
        }

        Vec::new()
    }

    pub fn can_claim_car(send_at: u64) -> bool {
        if send_at == 0 { return false; }
        let ts_ms = if send_at < 1_000_000_000_000 { send_at * 1000 } else { send_at };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now_ms.saturating_sub(ts_ms) >= 4 * 3600 * 1000
    }

    pub fn is_car_send_window() -> bool {
        let now = chrono::Local::now();
        let wd = now.weekday().num_days_from_monday(); // 0=Mon, 1=Tue, 2=Wed
        wd <= 2 && now.hour() < 20
    }

    // ============================================================
    // Cultivation Techniques (legacy_*)
    // ============================================================
    pub async fn legacy_getinfo(&self) -> Result<Value, String> {
        self.cmd("legacy_getinfo", json!({})).await
    }

    pub async fn legacy_claimhangup(&self) -> Result<Value, String> {
        self.cmd("legacy_claimhangup", json!({})).await
    }

    pub async fn legacy_gift_getlist(&self) -> Result<Value, String> {
        self.cmd("legacy_gift_getlist", json!({})).await
    }

    pub async fn legacy_gift_send(&self, recipient_id: u64, item_id: u64, quantity: u64) -> Result<Value, String> {
        self.cmd("legacy_gift_send", json!({
            "recipientId": recipient_id, "itemId": item_id, "quantity": quantity
        })).await
    }

    pub async fn legacy_gift_received(&self) -> Result<Value, String> {
        self.cmd("legacy_gift_received", json!({})).await
    }

    pub async fn legacy_sendgift(&self, params: Value) -> Result<Value, String> {
        self.cmd("legacy_sendgift", params).await
    }

    // ============================================================
    // Equipment (equipment_*)
    // ============================================================
    pub async fn equipment_confirm(&self, params: Value) -> Result<Value, String> {
        self.cmd("equipment_confirm", params).await
    }

    pub async fn equipment_quench(&self, params: Value) -> Result<Value, String> {
        self.cmd("equipment_quench", params).await
    }

    pub async fn equipment_updatequenchlock(&self, params: Value) -> Result<Value, String> {
        self.cmd("equipment_updatequenchlock", params).await
    }

    // ============================================================
    // Pearls (pearl_*)
    // ============================================================
    pub async fn pearl_replaceskill(&self, params: Value) -> Result<Value, String> {
        self.cmd("pearl_replaceskill", params).await
    }

    pub async fn pearl_exchangeskill(&self, params: Value) -> Result<Value, String> {
        self.cmd("pearl_exchangeskill", params).await
    }

    pub async fn pearl_unloadskill(&self, params: Value) -> Result<Value, String> {
        self.cmd("pearl_unloadskill", params).await
    }

    // ============================================================
    // Codex (book_*)
    // ============================================================
    pub async fn book_upgrade(&self, params: Value) -> Result<Value, String> {
        self.cmd("book_upgrade", params).await
    }

    pub async fn book_claimpointreward(&self, params: Value) -> Result<Value, String> {
        self.cmd("book_claimpointreward", params).await
    }

    // ============================================================
    // Lord Weapon (lordweapon_*)
    // ============================================================
    pub async fn lordweapon_changedefaultweapon(&self, params: Value) -> Result<Value, String> {
        self.cmd("lordweapon_changedefaultweapon", params).await
    }

    // ============================================================
    // Team Matching (matchteam_*)
    // ============================================================
    pub async fn matchteam_getroleteaminfo(&self) -> Result<Value, String> {
        self.cmd("matchteam_getroleteaminfo", json!({})).await
    }

    // ============================================================
    // Activities (activity_*)
    // ============================================================
    pub async fn activity_get(&self) -> Result<Value, String> {
        self.cmd("activity_get", json!({})).await
    }

    pub async fn activity_recyclewarorderrewardclaim(&self) -> Result<Value, String> {
        self.cmd("activity_recyclewarorderrewardclaim", json!({})).await
    }

    // ============================================================
    // Predictions (warguess_*)
    // ============================================================
    pub async fn warguess_getrank(&self) -> Result<Value, String> {
        self.cmd("warguess_getrank", json!({})).await
    }

    pub async fn warguess_startguess(&self, params: Value) -> Result<Value, String> {
        self.cmd("warguess_startguess", params).await
    }

    pub async fn warguess_getguesscoinreward(&self) -> Result<Value, String> {
        self.cmd("warguess_getguesscoinreward", json!({})).await
    }

    // ============================================================
    // Daily task status queries and smart execution
    // ============================================================
    pub fn get_daily_task_status(&self) -> DailyTaskStatus {
        let info = match &self.role_info {
            Some(v) => v,
            None => return DailyTaskStatus::default(),
        };

        let complete = info.pointer("/role/dailyTask/complete")
            .or_else(|| info.pointer("/dailyTask/complete"));
        let statistics = info.pointer("/role/statistics")
            .or_else(|| info.pointer("/statistics"));
        let statistics_time = info.pointer("/role/statisticsTime")
            .or_else(|| info.pointer("/statisticsTime"));

        let mut status = DailyTaskStatus::default();

        // complete map: key=task ID, value=-1 DONE
        if let Some(obj) = complete.and_then(|v| v.as_object()) {
            for (k, v) in obj {
                let task_id: u32 = k.parse().unwrap_or(0);
                let done = v.as_i64() == Some(-1);
                match task_id {
                    1  => status.login = done,
                    2  => status.share = done,
                    3  => status.friend_gift = done,
                    4  => status.recruit = done,
                    5  => status.hangup_reward = done,
                    6  => status.buy_gold = done,
                    7  => status.open_box = done,
                    12 => status.store_purchase = done,
                    13 => status.arena = done,
                    14 => status.bottle = done,
                    _  => {}
                }
            }
        }

        // Check additional state in statistics.
        if let Some(stats) = statistics {
        // Club Boss attempt count.
            if let Some(v) = stats.get("legion:boss").and_then(|v| v.as_i64()) {
                status.legion_boss_done = v as u32;
            }
        // Arena battles completed.
            if let Some(v) = stats.get("area:arena:last").and_then(|v| v.as_i64()) {
                status.arena_done_today = v as u32;
            }
        }

        // Check whether certain actions have run today in statisticsTime.
        if let Some(st) = statistics_time {
            status.buy_gold_today = !is_today_available(st.get("buy:gold"));
            status.artifact_lottery_today = !is_today_available(st.get("artifact:normal:lottery:time"));
            status.genie_1_today = !is_today_available(st.get("genie:daily:free:1"));
            status.genie_2_today = !is_today_available(st.get("genie:daily:free:2"));
            status.genie_3_today = !is_today_available(st.get("genie:daily:free:3"));
            status.genie_4_today = !is_today_available(st.get("genie:daily:free:4"));
            // Free capsule draw.
            status.gacha_today = !is_today_available(
                st.get("gacha:free").or_else(|| statistics.and_then(|s| s.get("gacha:free")))
            );
            // Club Boss: reset when the timestamp is not today.
            if is_today_available(st.get("legion:boss")) {
                status.legion_boss_done = 0;
            }
            // Arena: reset when the timestamp is not today.
            if is_today_available(st.get("area:arena:last")) {
                status.arena_done_today = 0;
            }
        }

        status
    }

    /// Execute daily tasks smartly: run only incomplete tasks and skip completed ones.
    /// Returns an execution-result summary.
    pub async fn run_daily_tasks(&self) -> DailyTaskReport {
        let status = self.get_daily_task_status();
        let mut report = DailyTaskReport::new();
        let delay = || tokio::time::sleep(tokio::time::Duration::from_millis(500));

        info!(target: "daily", summary = %status.summary(), "daily task status");

        // --- Basic daily tasks (in task-ID order) ---

        // Task ID 2: Share the game.
        if !status.share {
            report.run("分享游戏", self.system_mysharecallback().await);
            delay().await;
        } else { report.skip("分享游戏"); }

        // Task ID 3: Send gold to friends.
        if !status.friend_gift {
            report.run("赠送好友金币", self.friend_batch().await);
            delay().await;
        } else { report.skip("赠送好友金币"); }

        // Task ID 4: Free recruitment.
        if !status.recruit {
            report.run("免费招募", self.hero_recruit(3, 1).await);
            delay().await;
        } else { report.skip("免费招募"); }

        // Task ID 5: Hangup (smart handling: check duration, extend time, claim, then extend again).
        if !status.hangup_reward {
            self.smart_hangup(&mut report).await;
        } else {
            // Run smart hangup even when the task is complete to accumulate time for next time.
            debug!(target: "daily", "hangup task already done, still running smart_hangup");
            self.smart_hangup(&mut report).await;
        }

        // Task ID 6: Free gold conversion x3.
        if !status.buy_gold {
            for i in 1..=3 {
                report.run(&format!("免费点金 #{}/3", i), self.system_buygold().await);
                delay().await;
            }
        } else { report.skip("免费点金"); }

        // Task ID 7: Open chests.
        if !status.open_box {
            report.run("开启宝箱x10", self.item_openbox(2001, 10).await);
            delay().await;
        } else { report.skip("开启宝箱"); }

        // Task ID 12: Black Market purchase.
        if !status.store_purchase {
            report.run("黑市采购", self.store_purchase(1).await);
            delay().await;
        } else { report.skip("黑市采购"); }

        // Task ID 14: Salt Jar (smart handling: stop and start to extend its duration).
        self.smart_bottle(&mut report).await;

        // --- Arena (Task ID 13, three free attempts daily) ---
        if !status.arena {
            let arena_free: u32 = 3;
            let arena_remaining = arena_free.saturating_sub(status.arena_done_today);

            if arena_remaining == 0 {
                report.skip(&format!("竞技场 (已打{}/{})", status.arena_done_today, arena_free));
            } else {
                report.run("进入竞技场", self.arena_startarea().await);
                delay().await;
                for i in 1..=arena_remaining {
                    match self.arena_getareatarget().await {
                        Ok(targets) => {
                            let target_id = targets.pointer("/roleList/0/roleId")
                                .or_else(|| targets.pointer("/0/roleId"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            if target_id > 0 {
                                let r = self.fight_startareaarena(target_id).await;
                                report.run(&format!("竞技场战斗 #{}/{} (vs {})", i, arena_remaining, target_id), r);
                            } else {
                                report.skip(&format!("竞技场战斗 #{}/{} (无对手)", i, arena_remaining));
                            }
                        }
                        Err(e) => { report.fail(&format!("获取对手 #{}/{}", i, arena_remaining), &e); break; },
                    }
                    delay().await;
                }
            }
        } else { report.skip("竞技场"); }

        // --- Daily actions not tied to tasks ---

        // Boss battles (three times daily).
        let day_of_week = chrono::Local::now().weekday().num_days_from_sunday() as usize;
        let boss_id = DAY_BOSS_MAP[day_of_week];
        for i in 1..=3 {
            report.run(&format!("Boss战 #{}/3 (id={})", i, boss_id),
                self.fight_startboss(boss_id).await);
            delay().await;
        }

        // Club Boss (two free attempts daily).
        let legion_boss_max: u32 = 2;
        let legion_boss_remaining = legion_boss_max.saturating_sub(status.legion_boss_done);
        if legion_boss_remaining > 0 {
            for i in 1..=legion_boss_remaining {
                report.run(&format!("俱乐部Boss #{}/{} (已打{}/{})",
                    i, legion_boss_remaining, status.legion_boss_done, legion_boss_max),
                    self.fight_startlegionboss().await);
                delay().await;
            }
        } else {
            report.skip(&format!("俱乐部Boss (已打{}/{})", status.legion_boss_done, legion_boss_max));
        }
        delay().await;

        // Benefits check-in.
        report.run("福利签到", self.system_signinreward().await);
        delay().await;

        // Club check-in.
        report.run("俱乐部签到", self.legion_signin().await);
        delay().await;

        // Daily discount gift pack.
        report.run("每日折扣礼包", self.discount_claimreward().await);
        delay().await;

        // Free gift card.
        report.run("免费礼包卡", self.card_claimfree().await);
        delay().await;

        // Permanent-card gift pack.
        report.run("永久卡礼包", self.card_claimreward(4003).await);
        delay().await;

        // Treasure Pavilion free reward.
        report.run("珍宝阁免费奖励", self.collection_claimfreereward().await);
        delay().await;

        // Free fishing x3.
        if !status.artifact_lottery_today {
            for i in 1..=3 {
                report.run(&format!("免费钓鱼 #{}/3", i), self.artifact_lottery().await);
                delay().await;
            }
        } else { report.skip("免费钓鱼"); }

        // Genie sweeps (Wei, Shu, Wu, and Qun).
        let kingdoms = [
            (1, "魏国", status.genie_1_today),
            (2, "蜀国", status.genie_2_today),
            (3, "吴国", status.genie_3_today),
            (4, "群雄", status.genie_4_today),
        ];
        for (id, name, done) in kingdoms {
            if !done {
                report.run(&format!("{}灯神扫荡", name), self.genie_sweep(id).await);
                delay().await;
            } else { report.skip(&format!("{}灯神扫荡", name)); }
        }

        // Free sweep vouchers x3.
        for i in 1..=3 {
            report.run(&format!("免费扫荡卷 #{}/3", i), self.genie_buysweep().await);
            delay().await;
        }

        // --- Cultivation-technique fragments ---
        if self.level_id() > 8000 {
            report.run("领取功法残卷", self.legacy_claimhangup().await);
        } else {
            report.skip("领取功法残卷 (关卡≤8000)");
        }
        delay().await;

        // --- Claim task point rewards ---
        for task_id in 1..=10 {
            let _ = self.task_claimdailypoint(task_id).await;
        }
        let _ = self.task_claimdailyreward().await;
        let _ = self.task_claimweekreward().await;
        let _ = self.activity_recyclewarorderrewardclaim().await;
        report.run("领取任务奖励", Ok(json!({})));

        // Claim mail.
        report.run("领取邮件", self.mail_claimallattachment(0).await);

        // Salted-Fish King Dreamscape (open Sunday, Monday, Wednesday, and Thursday).
        let nightmare_day = chrono::Local::now().weekday().num_days_from_sunday();
        if matches!(nightmare_day, 0 | 1 | 3 | 4) {
            report.run(
                "咸王梦境",
                self.dungeon_selecthero(json!({"battleTeam": {"0": 107}})).await,
            );
            delay().await;
        } else {
            report.skip("咸王梦境 (当前未开放)");
        }

        info!(target: "daily", summary = %report.summary(), "daily tasks completed");
        report
    }

    // ============================================================
    // Stateful daily tasks for scheduler batch execution
    // ============================================================

    /// Execute daily tasks and update the completion flag for each RoleDailyState item.
    /// - Completed on the server: mark done and skip.
    /// - Already marked done locally: skip.
    /// - Success or an "already complete" error code: mark done.
    /// - Network, timeout, or similar errors: leave unmarked and retry next time.
    pub async fn run_daily_tasks_stateful(
        &self,
        daily: &mut crate::state::RoleDailyState,
        log_prefix: &str,
        config: &DailyTaskConfig,
    ) -> DailyTaskReport {
        use crate::error_codes::is_done_result;
        let status = self.get_daily_task_status();
        let mut report = DailyTaskReport::new();
        let delay = || tokio::time::sleep(tokio::time::Duration::from_millis(500));

        daily.ensure_today();

        let original_team = if config.formations.is_some() {
            match self.current_team_id().await {
                Ok(team_id) => Some(team_id),
                Err(e) => {
                    report.fail_with_prefix(log_prefix, "读取当前阵容", &e);
                    None
                }
            }
        } else {
            None
        };

        // Helper: execute an action and update its state flag from the result.
        macro_rules! do_task {
            ($flag:expr, $name:expr, $action:expr) => {
                if !$flag {
                    let result = $action;
                    let is_done = is_done_result(&result);
                    report.run_with_prefix(log_prefix, $name, &result);
                    if is_done { $flag = true; }
                    delay().await;
                } else {
                    report.skip_with_prefix(log_prefix, $name);
                }
            };
        }

        // --- Main daily tasks ---

        // Task ID 2: Share the game.
        let server_share = status.share;
        if server_share { daily.share = true; }
        do_task!(daily.share, "分享游戏", self.system_mysharecallback().await);

        // Task ID 3: Send gold to friends.
        if status.friend_gift { daily.friend_gift = true; }
        do_task!(daily.friend_gift, "赠送好友金币", self.friend_batch().await);

        // Task ID 4: Free recruitment.
        if status.recruit { daily.recruit = true; }
        do_task!(daily.recruit, "免费招募", self.hero_recruit(3, 1).await);

        // Task ID 5: Hangup rewards (always run smart_hangup).
        if status.hangup_reward { daily.hangup_reward = true; }
        // smart_hangup handles its own state; do not use the do_task macro.
        debug!(target: "daily", prefix = log_prefix, "running smart_hangup flow");
        self.smart_hangup(&mut report).await;
        daily.hangup_reward = true; // The hangup flow always runs.

        // Task ID 6: Free gold conversion x3.
        if status.buy_gold { daily.buy_gold = true; }
        if !daily.buy_gold {
            let mut all_done = true;
            for i in 1..=3 {
                let r = self.system_buygold().await;
                if !is_done_result(&r) { all_done = false; }
                report.run_with_prefix(log_prefix, &format!("免费点金 #{}/3", i), &r);
                delay().await;
            }
            if all_done { daily.buy_gold = true; }
        } else { report.skip_with_prefix(log_prefix, "免费点金"); }

        // Task ID 7: Open chests.
        if status.open_box { daily.open_box = true; }
        do_task!(daily.open_box, "开启宝箱x10", self.item_openbox(2001, 10).await);

        // Task ID 12: Black Market purchase.
        if status.store_purchase { daily.store_purchase = true; }
        do_task!(daily.store_purchase, "黑市采购", self.store_purchase(1).await);

        // Task ID 14: Salt Jar (always run smart_bottle).
        if status.bottle { daily.bottle_task = true; }
        self.smart_bottle(&mut report).await;
        daily.bottle_task = true;

        // Task ID 13: Arena (three free attempts daily; count attempts in statistics["area:arena:last"]).
        if status.arena { daily.arena = true; }
        if !daily.arena {
            if self.level_id() < 400 {
                report.skip_with_prefix(log_prefix, "竞技场 (关卡<400)");
                daily.arena = true;
            } else {
            let arena_free: u32 = 3;
            let arena_remaining = arena_free.saturating_sub(status.arena_done_today);

            if arena_remaining == 0 {
                report.skip_with_prefix(log_prefix, &format!("竞技场 (已打{}/{})", status.arena_done_today, arena_free));
            } else {
                let switched = if let Some(plan) = config.formations {
                    match switch_to_context_formation(self, plan.arena, "arena", original_team).await {
                        Ok(v) => v,
                        Err(e) => {
                            report.fail_with_prefix(log_prefix, "切换竞技场阵容", &e);
                            false
                        }
                    }
                } else { false };
                let r = self.arena_startarea().await;
                report.run_with_prefix(log_prefix, &format!("进入竞技场 (剩余{}/{})", arena_remaining, arena_free), &r);
                delay().await;
                if r.is_ok() {
                    for i in 1..=arena_remaining {
                        match self.arena_getareatarget().await {
                            Ok(targets) => {
                                let target_id = targets.pointer("/roleList/0/roleId")
                                    .or_else(|| targets.pointer("/0/roleId"))
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                if target_id > 0 {
                                    let r = self.fight_startareaarena(target_id).await;
                                    report.run_with_prefix(log_prefix,
                                        &format!("竞技场战斗 #{}/{} (vs {})", i, arena_remaining, target_id), &r);
                                    if let Err(ref e) = r {
                                        if e.contains("200750") { break; }
                                    }
                                }
                            }
                            Err(e) => {
                                report.fail_with_prefix(log_prefix, &format!("获取对手 #{}/{}", i, arena_remaining), &e);
                                break;
                            }
                        }
                        delay().await;
                    }
                }
                if let Err(e) = restore_context_formation(self, switched, "arena", original_team).await {
                    report.fail_with_prefix(log_prefix, "恢复竞技场阵容", &e);
                }
            }
            daily.arena = true;
            }
        } else { report.skip_with_prefix(log_prefix, "竞技场"); }

        // --- Non-main daily tasks ---
        // Boss battles.
        if !daily.boss {
            let day_of_week = chrono::Local::now().weekday().num_days_from_sunday() as usize;
            let boss_id = DAY_BOSS_MAP[day_of_week];
            let switched = if let Some(plan) = config.formations {
                match switch_to_context_formation(self, plan.boss_daily, "boss_daily", original_team).await {
                    Ok(v) => v,
                    Err(e) => {
                        report.fail_with_prefix(log_prefix, "切换每日BOSS阵容", &e);
                        false
                    }
                }
            } else { false };
            for i in 1..=3 {
                let r = self.fight_startboss(boss_id).await;
                report.run_with_prefix(log_prefix, &format!("Boss战 #{}/3 (id={})", i, boss_id), &r);
                delay().await;
            }
            if let Err(e) = restore_context_formation(self, switched, "boss_daily", original_team).await {
                report.fail_with_prefix(log_prefix, "恢复每日BOSS阵容", &e);
            }
            daily.boss = true;
        } else { report.skip_with_prefix(log_prefix, "Boss战"); }

        // Legion Boss
        if !daily.legion_boss {
            let max: u32 = 2;
            let remaining = max.saturating_sub(status.legion_boss_done);
            let switched = if remaining > 0 {
                if let Some(plan) = config.formations {
                    match switch_to_context_formation(self, plan.boss_legion, "boss_legion", original_team).await {
                        Ok(v) => v,
                        Err(e) => {
                            report.fail_with_prefix(log_prefix, "切换军团BOSS阵容", &e);
                            false
                        }
                    }
                } else { false }
            } else { false };
            if remaining > 0 {
                for i in 1..=remaining {
                    let r = self.fight_startlegionboss().await;
                    report.run_with_prefix(log_prefix,
                        &format!("俱乐部Boss #{}/{}", i, remaining), &r);
                    delay().await;
                }
            }
            if let Err(e) = restore_context_formation(self, switched, "boss_legion", original_team).await {
                report.fail_with_prefix(log_prefix, "恢复军团BOSS阵容", &e);
            }
            daily.legion_boss = true;
        } else { report.skip_with_prefix(log_prefix, "俱乐部Boss"); }

        // Benefits check-in.
        do_task!(daily.signin, "福利签到", self.system_signinreward().await);

        // Club check-in.
        do_task!(daily.legion_signin, "俱乐部签到", self.legion_signin().await);

        // Discount gift pack.
        do_task!(daily.discount, "每日折扣礼包", self.discount_claimreward().await);

        // Card gift packs.
        if !daily.card {
            let r1 = self.card_claimfree().await;
            report.run_with_prefix(log_prefix, "免费礼包卡", &r1);
            delay().await;
            let r2 = self.card_claimreward(4003).await;
            report.run_with_prefix(log_prefix, "永久卡礼包", &r2);
            delay().await;
            if is_done_result(&r1) && is_done_result(&r2) { daily.card = true; }
        } else { report.skip_with_prefix(log_prefix, "卡牌礼包"); }

        // Treasure Pavilion.
        do_task!(daily.collection, "珍宝阁免费奖励", self.collection_claimfreereward().await);

        // Fishing.
        if !daily.artifact && !status.artifact_lottery_today {
            for i in 1..=3 {
                let r = self.artifact_lottery().await;
                report.run_with_prefix(log_prefix, &format!("免费钓鱼 #{}/3", i), &r);
                delay().await;
            }
            daily.artifact = true;
        } else {
            if status.artifact_lottery_today { daily.artifact = true; }
            report.skip_with_prefix(log_prefix, "免费钓鱼");
        }

        // Genie sweeps.
        if !daily.genie {
            if self.level_id() < 3000 {
                report.skip_with_prefix(log_prefix, "灯神扫荡 (关卡<3000)");
                daily.genie = true;
            } else {
            let kingdoms = [
                (1, "魏国", status.genie_1_today),
                (2, "蜀国", status.genie_2_today),
                (3, "吴国", status.genie_3_today),
                (4, "群雄", status.genie_4_today),
            ];
            for (id, name, done) in kingdoms {
                if !done {
                    let r = self.genie_sweep(id).await;
                    report.run_with_prefix(log_prefix, &format!("{}灯神扫荡", name), &r);
                    delay().await;
                }
            }
            for i in 1..=3 {
                let r = self.genie_buysweep().await;
                report.run_with_prefix(log_prefix, &format!("免费扫荡卷 #{}/3", i), &r);
                delay().await;
            }
            daily.genie = true;
            } // end level check
        } else { report.skip_with_prefix(log_prefix, "灯神扫荡"); }

        // Free capsule draw.
        if config.gacha_enabled && !daily.gacha {
            if !status.gacha_today {
                let r = self.gacha_drawreward().await;
                report.run_with_prefix(log_prefix, "免费扭蛋", &r);
            } else {
                report.skip_with_prefix(log_prefix, "免费扭蛋 (已完成)");
            }
            daily.gacha = true;
        }

        // Claim task points (best effort; does not block the completion flag).
        if !daily.task_rewards {
            for task_id in 1..=10 {
                let r = self.task_claimdailypoint(task_id).await;
                report.run_with_prefix(log_prefix, &format!("领取任务{}积分", task_id), &r);
                delay().await;
            }
            let r_daily = self.task_claimdailyreward().await;
            report.run_with_prefix(log_prefix, "领取每日任务奖励", &r_daily);
            delay().await;
            let r_week = self.task_claimweekreward().await;
            report.run_with_prefix(log_prefix, "领取每周任务奖励", &r_week);
            delay().await;
            let r_war = self.activity_recyclewarorderrewardclaim().await;
            report.run_with_prefix(log_prefix, "领取战令奖励", &r_war);
            delay().await;
            daily.task_rewards = true;
        } else { report.skip_with_prefix(log_prefix, "任务奖励"); }

        // Mail.
        do_task!(daily.mail, "领取邮件", self.mail_claimallattachment(0).await);

        // Salted-Fish King Dreamscape (open Sunday, Monday, Wednesday, and Thursday).
        let nightmare_day = chrono::Local::now().weekday().num_days_from_sunday();
        if !daily.nightmare {
            if matches!(nightmare_day, 0 | 1 | 3 | 4) {
                let r = self.dungeon_selecthero(json!({"battleTeam": {"0": 107}})).await;
                report.run_with_prefix(log_prefix, "咸王梦境", &r);
                if is_done_result(&r) {
                    daily.nightmare = true;
                }
                delay().await;
            } else {
                report.skip_with_prefix(log_prefix, "咸王梦境 (当前未开放)");
            }
        } else { report.skip_with_prefix(log_prefix, "咸王梦境"); }

        // Dreamscape store purchases (same availability as Dreamscape; optional by configuration).
        if let Some(dream_shop) = config.dream_shop.as_ref() {
            if !daily.dream_shop {
                if matches!(nightmare_day, 0 | 1 | 3 | 4) {
                    run_dream_shop_daily(self, &mut report, log_prefix, dream_shop).await;
                    daily.dream_shop = true;
                } else {
                    report.skip_with_prefix(log_prefix, "梦境商店购买 (当前未开放)");
                }
            } else {
                report.skip_with_prefix(log_prefix, "梦境商店购买");
            }
        }

        // Club vehicle dispatch and collection.
        if config.car_enabled {
            if !daily.car_send_done {
                if Self::is_car_send_window() {
                    let (claim_report, next_claim) = self.claim_all_cars(log_prefix).await;
                    for r in claim_report.results { report.results.push(r); }
                    let send_report = self.smart_send_car(log_prefix).await;
                    for r in send_report.results { report.results.push(r); }
                    daily.car_send_done = true;
                    let now = crate::state::now_secs();
                    let post_send = now + 4.0 * 3600.0;
                    daily.next_car_claim_time = if next_claim > 0.0 && next_claim < post_send {
                        next_claim
                    } else {
                        post_send
                    };
                } else {
                    report.skip_with_prefix(log_prefix, "发车 (不在发车窗口)");
                }
            } else {
                report.skip_with_prefix(log_prefix, "发车/收车");
            }
        }

        report
    }

    // ============================================================
    // periodic_tasks (hangup/bottle/legacy/tower/evotower)
    // ============================================================

    /// is_evotower_active
    pub fn is_evotower_active() -> bool {
        use chrono::NaiveDate;
        let start = NaiveDate::from_ymd_opt(2025, 12, 12)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .expect("valid local datetime");
        let elapsed = chrono::Local::now() - start;
        if elapsed.num_seconds() < 0 {
            return false;
        }
        let week_secs: i64 = 7 * 24 * 3600;
        let cycle_secs = 3 * week_secs;
        (elapsed.num_seconds() % cycle_secs) < week_secs
    }

    /// run_periodic_tasks, update periodic state
    pub async fn run_periodic_tasks(
        &mut self,
        periodic: &mut crate::state::RolePeriodicState,
        config: &crate::config::BatchConfig,
        tower_team: u64,
        evotower_team: u64,
        log_prefix: &str,
    ) -> DailyTaskReport {
        let mut report = DailyTaskReport::new();
        let delay = || tokio::time::sleep(tokio::time::Duration::from_millis(500));
        let mut needs_refresh_role_info = false;
        let mut bottle_renewed = false;

        // hangup
        if periodic.needs_hangup(config.hangup_threshold_hours) {
            debug!(target: "periodic", prefix = log_prefix, "hangup threshold reached, run smart_hangup");
            self.smart_hangup(&mut report).await;
            needs_refresh_role_info = true;
        }

        // bottle
        if periodic.needs_bottle(config.bottle_threshold_hours) {
            debug!(target: "periodic", prefix = log_prefix, "bottle threshold reached, run smart_bottle");
            self.smart_bottle(&mut report).await;
            needs_refresh_role_info = true;
            bottle_renewed = true;
        }

        // legacy
        if periodic.needs_legacy(config.legacy_interval_hours) {
            if self.level_id() > 8000 {
                if !bottle_renewed {
                    self.smart_bottle(&mut report).await;
                }
                needs_refresh_role_info = true;
                let r = self.legacy_claimhangup().await;
                report.run_with_prefix(log_prefix, "legacy claim", &r);
                if crate::error_codes::is_done_result(&r) {
                    periodic.legacy_last_claim = crate::state::now_secs();
                }
                delay().await;
            } else {
                report.skip_with_prefix(log_prefix, "legacy claim (level_id ≤ 8000)");
                periodic.legacy_last_claim = crate::state::now_secs();
            }
        }

        let mut role_info_refreshed = !needs_refresh_role_info;
        if needs_refresh_role_info {
            match self.role_getroleinfo().await {
                Ok(info) => {
                    self.role_info = Some(info);
                    role_info_refreshed = true;
                }
                Err(e) => {
                    warn!(target: "periodic", error = %e, "failed to refresh role info after periodic actions");
                }
            }
        }

        if role_info_refreshed {
            self.update_periodic_state(periodic);
        }

        // tower
        if config.tower_enabled && periodic.needs_tower() {
            self.run_tower_climb(periodic, tower_team, log_prefix, &mut report).await;
        }

        // evotower
        if config.evotower_enabled && Self::is_evotower_active() && periodic.needs_evotower() {
            if !self.is_evotower_unlocked() {
                info!(target: "daily", log_prefix = log_prefix, "evotower be skipped(level_id < 7000)");
                periodic.evo_next_check = crate::state::now_secs() + 86400.0;
            } else {
                self.run_evotower_climb(periodic, evotower_team, log_prefix, &mut report).await;
            }
        }

        report
    }

    /// fight_starttower (fight_starttower 1 step)
    async fn run_tower_climb(
        &mut self,
        periodic: &mut crate::state::RolePeriodicState,
        tower_team: u64,
        log_prefix: &str,
        report: &mut DailyTaskReport,
    ) {
        const MAX_CLIMBS: u32 = 100;
        let delay = || tokio::time::sleep(tokio::time::Duration::from_millis(500));
        let long_delay = || tokio::time::sleep(tokio::time::Duration::from_secs(5));

        if self.role_info.as_ref().map(tower_is_cleared).unwrap_or(false) {
            periodic.tower_cleared = true;
            periodic.tower_next_check = 0.0;
            info!(target: "tower", "tower already fully cleared, skip");
            return;
        }

        let _ = self.tower_getinfo().await;
        if let Ok(info) = self.role_getroleinfo().await {
            self.role_info = Some(info);
        }
        let mut energy: i64 = self.role_info.as_ref()
            .and_then(|v| v.pointer("/role/tower/energy").or_else(|| v.pointer("/tower/energy")))
            .and_then(|v| v.as_i64()).unwrap_or(0);
        let mut tower_id: u64 = self.role_info.as_ref()
            .and_then(tower_id_from_role_info)
            .unwrap_or(0);

        if tower_id >= TOWER_CLEAR_ID {
            periodic.tower_cleared = true;
            periodic.tower_next_check = 0.0;
            info!(target: "tower", tower_id, "tower already fully cleared, skip");
            return;
        }

        let original_team = match self.current_team_id().await {
            Ok(id) => Some(id),
            Err(_) => None,
        };
        let switched = if let Some(orig) = original_team {
            if orig != tower_team {
                let _ = self.switch_team(tower_team).await;
                true
            } else { false }
        } else { false };

        // check the previous tower's reward
        if tower_id > 0 && tower_id % 10 == 0 {
            let reward_floor = tower_id / 10;
            if reward_floor > 0 {
                let r = self.tower_claimreward(reward_floor).await;
                report.run_with_prefix(log_prefix, &format!("claim tower reward {}", reward_floor), &r);
                delay().await;
                if let Ok(info) = self.role_getroleinfo().await {
                    self.role_info = Some(info);
                }
                energy = self.role_info.as_ref()
                    .and_then(|v| v.pointer("/role/tower/energy").or_else(|| v.pointer("/tower/energy")))
                    .and_then(|v| v.as_i64()).unwrap_or(energy);
                tower_id = self.role_info.as_ref()
                    .and_then(|v| v.pointer("/role/tower/id").or_else(|| v.pointer("/tower/id")))
                    .and_then(|v| v.as_u64()).unwrap_or(tower_id);
            }
        }

        info!(target: "tower", energy, tower_id, "tower climb start");

        let mut climb_count = 0u32;
        let mut consecutive_failures = 0u32;
        while energy > 0 && climb_count < MAX_CLIMBS {
            match self.fight_starttower().await {
                Ok(resp) => {
                    climb_count += 1;
                    consecutive_failures = 0;
                    energy = energy.saturating_sub(1);
                    if let Some(id) = resp.pointer("/battleData/options/towerId")
                        .and_then(|v| v.as_u64())
                    {
                        tower_id = id;
                    }
                    info!(target: "tower", climb_count, energy, tower_id, "tower fight ok");
                }
                Err(e) => {
                    let code = crate::error_codes::extract_code_from_error(&e);
                    match code {
                        Some(1500040) => {
                            let reward_floor = tower_id / 10;
                            if reward_floor > 0 {
                                let r = self.tower_claimreward(reward_floor).await;
                                report.run_with_prefix(log_prefix, &format!("tower_claimreward {}", reward_floor), &r);
                            }
                            delay().await;
                            if let Ok(info) = self.role_getroleinfo().await {
                                self.role_info = Some(info);
                            }
                            energy = self.role_info.as_ref()
                                .and_then(|v| v.pointer("/role/tower/energy").or_else(|| v.pointer("/tower/energy")))
                                .and_then(|v| v.as_i64()).unwrap_or(energy);
                            tower_id = self.role_info.as_ref()
                                .and_then(|v| v.pointer("/role/tower/id").or_else(|| v.pointer("/tower/id")))
                                .and_then(|v| v.as_u64()).unwrap_or(tower_id);
                            consecutive_failures = 0;
                            continue;
                        }
                        Some(1500010) => {
                            periodic.tower_cleared = true;
                            periodic.tower_next_check = 0.0;
                            info!(target: "tower", error = %e, code, "tower fully cleared");
                            break;
                        }
                        Some(1500020) => {
                            info!(target: "tower", error = %e, code, "tower energy exhausted");
                            break;
                        }
                        Some(200400) => {
                            info!(target: "tower", "tower rate limited, backoff 5s");
                            long_delay().await;
                            continue;
                        }
                        _ => {
                            consecutive_failures += 1;
                            report.fail_with_prefix(log_prefix, "tower fight", &e);
                            if consecutive_failures >= 3 {
                                info!(target: "tower", consecutive_failures, "tower consecutive failures, stop");
                                break;
                            }
                            delay().await;
                        }
                    }
                }
            }
            delay().await;
        }

        info!(target: "tower", climb_count, energy, tower_id, "tower climb done");

        if let Ok(info) = self.role_getroleinfo().await {
            self.role_info = Some(info);
            energy = self.role_info.as_ref()
                .and_then(|v| v.pointer("/role/tower/energy").or_else(|| v.pointer("/tower/energy")))
                .and_then(|v| v.as_i64()).unwrap_or(energy);
            self.update_periodic_state(periodic);
        }
        if tower_id >= TOWER_CLEAR_ID {
            periodic.tower_cleared = true;
            periodic.tower_next_check = 0.0;
        }
        if !periodic.tower_cleared {
            periodic.tower_next_check = crate::state::now_secs()
                + (10i64.saturating_sub(energy) as f64 * 1800.0);
        }

        if switched {
            let _ = self.switch_team(original_team.unwrap_or(tower_team)).await;
        }
    }

    /// evotower (evotower_readyfight + evotower_fight 2 steps) + mergebox
    async fn run_evotower_climb(
        &mut self,
        periodic: &mut crate::state::RolePeriodicState,
        evotower_team: u64,
        log_prefix: &str,
        report: &mut DailyTaskReport,
    ) {
        let delay = || tokio::time::sleep(tokio::time::Duration::from_millis(500));
        let original_team = match self.current_team_id().await {
            Ok(id) => Some(id),
            Err(_) => None,
        };
        let switched = if let Some(orig) = original_team {
            if orig != evotower_team {
                let _ = self.switch_team(evotower_team).await;
                true
            } else { false }
        } else { false };

        let evo_info = match self.evotower_getinfo().await {
            Ok(v) => v,
            Err(_) => {
                if switched { let _ = self.switch_team(original_team.unwrap_or(evotower_team)).await; }
                return;
            }
        };
        let mut energy: i64 = evo_info.pointer("/evoTower/energy").and_then(|v| v.as_i64()).unwrap_or(0);
        let mut tower_id: u64 = evo_info.pointer("/evoTower/towerId").and_then(|v| v.as_u64()).unwrap_or(0);

        if tower_id > 0 && tower_id % 10 == 0 {
            let _ = self.evotower_claimreward(json!({})).await;
            delay().await;
        }

        info!(target: "evo", energy, tower_id, "evotower climb start");

        let mut climb_count = 0u32;
        while energy > 0 {
            let _ = self.evotower_readyfight(json!({})).await;
            delay().await;

            let fight_resp = match self.evotower_fight(json!({"battleNum": 1, "winNum": 1})).await {
                Ok(v) => v,
                Err(e) => {
                    report.fail_with_prefix(log_prefix, "evotower fight", &e);
                    break;
                }
            };
            climb_count += 1;
            energy = energy.saturating_sub(1);

            let won = fight_resp.pointer("/winList/0").and_then(|v| v.as_bool()).unwrap_or(false);

            if let Ok(info) = self.evotower_getinfo().await {
                energy = info.pointer("/evoTower/energy").and_then(|v| v.as_i64()).unwrap_or(energy);
                tower_id = info.pointer("/evoTower/towerId").and_then(|v| v.as_u64()).unwrap_or(tower_id);
            }

            let task_id = match climb_count {
                3 => Some(1),
                6 => Some(2),
                10 => Some(3),
                _ => None,
            };
            if let Some(tid) = task_id {
                let _ = self.evotower_claimtask(tid).await;
            }

            // section rewards (tower_id == 10)
            if won && tower_id > 0 && (tower_id % 10) + 1 == 1 {
                let _ = self.evotower_claimreward(json!({})).await;
            }

            info!(target: "evo", climb_count, energy, tower_id, won, "evotower fight");
            delay().await;
        }

        info!(target: "evo", climb_count, energy, tower_id, "evotower climb done");

        // Mergebox
        self.run_mergebox_tail(log_prefix).await;

        periodic.evo_next_check = crate::state::now_secs() + (10i64.saturating_sub(energy) as f64 * 1800.0);

        if switched {
            let _ = self.switch_team(original_team.unwrap_or(evotower_team)).await;
        }
    }

    /// (Skin Challenge) BOSS Challenge
    pub async fn run_skinc_climb(&mut self, log_prefix: &str) -> Result<(), String> {
        use chrono::Datelike;
        let info = match self.towers_getinfo().await {
            Ok(v) => v,
            Err(e) => {
                if e.contains("7900021") {
                    tracing::info!(target: "skinc", log_prefix = log_prefix, "skin challenge activity is not open (7900021)");
                    return Ok(());
                }
                tracing::warn!(target: "skinc", log_prefix = log_prefix, "get activity faild: {}", e);
                return Err(e);
            }
        };

        // 1. is activity ended
        let act_id = info.pointer("/actId")
            .or_else(|| info.pointer("/towerData/actId"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        if act_id < 100000 {
            tracing::warn!(target: "skinc", log_prefix = log_prefix, "activity not open or no actId: {}", act_id);
            return Ok(());
        }

        let act_str = act_id.to_string();
        if act_str.len() >= 6 {
            let year = format!("20{}", &act_str[0..2]).parse::<i32>().unwrap_or(0);
            let month = act_str[2..4].parse::<u32>().unwrap_or(0);
            let day = act_str[4..6].parse::<u32>().unwrap_or(0);

            if let Some(start_date) = chrono::NaiveDate::from_ymd_opt(year, month, day) {
                let start_dt = start_date.and_hms_opt(0, 0, 0).unwrap().and_local_timezone(chrono::Local).unwrap();
                let end_dt = start_dt + chrono::TimeDelta::try_days(7).unwrap();
                let now = chrono::Local::now();

                if now < start_dt || now >= end_dt {
                    tracing::warn!(target: "skinc", log_prefix = log_prefix, "activity ended (actId={})", act_id);
                    return Ok(());
                }
            }
        }

        // 2. identy today's Boss
        let today_weekday = chrono::Local::now().weekday().num_days_from_sunday(); // Sun=0, Mon=1...
        let open_towers: Vec<u64> = match today_weekday {
            5 => vec![1], // Fri
            6 => vec![2], // Sat
            0 => vec![3], // Sun
            1 => vec![4], // Mon
            2 => vec![5], // Tue
            3 => vec![6], // Wed
            4 => vec![1, 2, 3, 4, 5, 6], // Thu
            _ => vec![],
        };

        // 3. filter not clear Boss
        let mut level_reward_map = info.pointer("/levelRewardMap")
            .or_else(|| info.pointer("/towerData/levelRewardMap"))
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let mut target_towers = Vec::new();
        for t in open_towers {
            let key_str = format!("{}008", t);
            if !level_reward_map.contains_key(&key_str) {
                target_towers.push(t);
            }
        }

        if target_towers.is_empty() {
            tracing::info!(target: "skinc", log_prefix = log_prefix, "Today's Boss all clear");
            return Ok(());
        }

        // 4. challenge
        for t in target_towers {
            let floor = skinc_current_floor(t, &level_reward_map);
            tracing::info!(target: "skinc", log_prefix = log_prefix, floor, "begin challenge Boss {}", t);
            let mut need_start = true;
            let mut fail_count = 0;

            loop {
                let floor = skinc_current_floor(t, &level_reward_map);
                if need_start {
                    self.towers_start(serde_json::json!({"towerType": t})).await
                        .map_err(|e| format!("Boss {} start failed: {}", t, e))?;
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                }

                match self.towers_fight(serde_json::json!({"towerType": t})).await {
                    Ok(fight_res) => {
                        let cur_hp = skinc_remaining_hp(&fight_res);
                        if cur_hp == Some(0.0) {
                            tracing::info!(target: "skinc", log_prefix = log_prefix, floor, "Boss {} challenge succ, next one", t);
                            need_start = false;
                            fail_count = 0;

                            // all pass check
                            let res = self.towers_getinfo().await
                                .map_err(|e| format!("Boss {} refresh failed: {}", t, e))?;
                            level_reward_map = res.pointer("/levelRewardMap")
                                .or_else(|| res.pointer("/towerData/levelRewardMap"))
                                .and_then(|v| v.as_object())
                                .cloned()
                                .unwrap_or_default();
                            if level_reward_map.contains_key(&format!("{}008", t)) {
                                tracing::info!(target: "skinc", log_prefix = log_prefix, "Boss {} all clear", t);
                                break;
                            }
                            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                        } else {
                            fail_count += 1;
                            tracing::warn!(target: "skinc", log_prefix = log_prefix, floor, remaining_hp = ?cur_hp, "Boss {} challenge failed ({} times)", t, fail_count);
                            need_start = true;
                            if fail_count >= 3 {
                                tracing::error!(target: "skinc", log_prefix = log_prefix, "Boss {} failed 3 times, skip", t);
                                break;
                            }
                            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                        }
                    }
                    Err(e) => {
                        tracing::error!(target: "skinc", log_prefix = log_prefix, "Exception Request: {}", e);
                        return Err(format!("Boss {} fight failed: {}", t, e));
                    }
                }
            }
        }

        Ok(())
    }

    async fn run_mergebox_tail(&self, _log_prefix: &str) {
        let delay = || tokio::time::sleep(tokio::time::Duration::from_millis(300));

        // free energy
        if let Ok(info) = self.mergebox_getinfo(1).await {
            let free_energy = info.pointer("/mergeBox/freeEnergy")
                .and_then(|v| v.as_i64()).unwrap_or(0);
            if free_energy > 0 {
                let _ = self.mergebox_claimfreeenergy(1).await;
                info!(target: "evo", free_energy, "mergebox claim free energy");
                delay().await;
            }
        }

        // open box (lotteryLeftCnt)
        if let Ok(evo_info) = self.evotower_getinfo().await {
            let mut lottery_left = evo_info.pointer("/evoTower/lotteryLeftCnt")
                .and_then(|v| v.as_i64()).unwrap_or(0);

            while lottery_left > 0 {
                if let Ok(merge_info) = self.mergebox_getinfo(1).await {
                    let cost_total = merge_info.pointer("/mergeBox/costTotalCnt")
                        .and_then(|v| v.as_i64()).unwrap_or(0);

                    let pos = if cost_total < 2 {
                        json!({"gridX": 4, "gridY": 5})
                    } else if cost_total < 102 {
                        json!({"gridX": 7, "gridY": 3})
                    } else {
                        json!({"gridX": 6, "gridY": 3})
                    };

                    let r = self.mergebox_openbox(json!({"actType": 1, "pos": pos})).await;
                    match &r {
                        Ok(_) => { lottery_left = lottery_left.saturating_sub(1); }
                        Err(_) => {
                            // matrix is full, merge first
                            let _ = self.mergebox_automergeitem(1).await;
                            delay().await;
                            let _ = self.mergebox_claimmergeprogress(1, 1).await;
                            delay().await;
                            continue;
                        }
                    }
                    let _ = self.mergebox_claimcostprogress(1).await;
                    delay().await;
                } else {
                    break;
                }
            }
        }

        // mergebox
        if let Ok(_) = self.mergebox_getinfo(1).await {
            let _ = self.mergebox_automergeitem(1).await;
            delay().await;
            let _ = self.mergebox_claimmergeprogress(1, 1).await;
            let _ = self.mergebox_claimcostprogress(1).await;
        }
    }

    /// get level_id from role_info
    pub fn level_id(&self) -> u64 {
        self.role_info.as_ref()
            .and_then(|info| info.pointer("/role/levelId").or_else(|| info.pointer("/levelId")))
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    }

    pub fn is_evotower_unlocked(&self) -> bool {
        self.level_id() >= 4001
    }

    /// update_periodic_state
    pub fn update_periodic_state(&self, periodic: &mut crate::state::RolePeriodicState) {
        if let Some(ref info) = self.role_info {
            periodic.update_hangup_from_role(info);
            periodic.update_bottle_from_role(info);

            if tower_is_cleared(info) {
                if !periodic.tower_cleared {
                    info!(target: "tower", tower_id = ?tower_id_from_role_info(info), "tower clear state recorded");
                }
                periodic.tower_cleared = true;
                periodic.tower_next_check = 0.0;
            }
        }
    }
}

// ============================================================
// BottleStatus
// ============================================================

/// BottleStatus (get from role.bottleHelpers)
#[derive(Debug, Clone, Default)]
pub struct BottleStatus {
    pub stop_time: f64,
    pub remaining: f64,
    pub is_running: bool,
}

impl BottleStatus {
    /// format time to HH:MM:SS
    pub fn fmt_time(seconds: f64) -> String {
        let total = seconds.max(0.0) as u64;
        let h = total / 3600;
        let m = (total % 3600) / 60;
        let s = total % 60;
        format!("{:02}:{:02}:{:02}", h, m, s)
    }

    pub fn summary(&self) -> String {
        format!("Bottle: {} | Remaining {}",
            if self.is_running { "Running" } else { "Suspend" },
            Self::fmt_time(self.remaining),
        )
    }
}

// ============================================================
// HangUpStatus
// ============================================================

/// HangUpStatus (get from role.hangUp)
#[derive(Debug, Clone, Default)]
pub struct HangUpStatus {
    pub last_time: f64,
    pub hangup_time: f64,
    pub elapsed: f64,
    pub remaining: f64,
    pub is_active: bool,
}

impl HangUpStatus {
    /// elapsed_hours (hour)
    pub fn elapsed_hours(&self) -> f64 {
        self.elapsed / 3600.0
    }

    /// remaining_hours (hour)
    pub fn remaining_hours(&self) -> f64 {
        self.remaining / 3600.0
    }

    /// hangup_hours (hour)
    pub fn hangup_hours(&self) -> f64 {
        self.hangup_time / 3600.0
    }

    /// needs_extend
    pub fn needs_extend(&self) -> bool {
        self.elapsed < 8.0 * 3600.0
    }

    /// format time to HH:MM:SS
    fn fmt_time(seconds: f64) -> String {
        let total = seconds.max(0.0) as u64;
        let h = total / 3600;
        let m = (total % 3600) / 60;
        let s = total % 60;
        format!("{:02}:{:02}:{:02}", h, m, s)
    }

    pub fn summary(&self) -> String {
        format!("Hangup: {} | Elapsed{} / Total{} | Remaining{} | {}",
            if self.is_active { "in processing" } else { "suspend" },
            Self::fmt_time(self.elapsed),
            Self::fmt_time(self.hangup_time),
            Self::fmt_time(self.remaining),
            if self.needs_extend() { "need extend" } else { "enough time" },
        )
    }
}

// ============================================================
// Daily Task State
// ============================================================

#[derive(Debug, Clone, Default)]
pub struct DailyTaskStatus {
    // 10 daily task (key from dailyTask.complete)
    pub login: bool,           // ID 1: Log in to the game once.
    pub share: bool,           // ID 2: Share the game once.
    pub friend_gift: bool,     // ID 3: Send gold to friends three times.
    pub recruit: bool,         // ID 4: Recruit twice.
    pub hangup_reward: bool,   // ID 5: Claim hangup rewards five times.
    pub buy_gold: bool,        // ID 6: Perform gold conversion three times.
    pub open_box: bool,        // ID 7: Open chests three times.
    pub store_purchase: bool,  // ID 12: Buy an item from the Black Market once.
    pub arena: bool,           // ID 13: Complete one Arena battle.
    pub bottle: bool,          // ID 14: Harvest any one Salt Jar.

    // extend state (statistics/statisticsTime)
    pub legion_boss_done: u32,          // Club Boss attempts completed.
    pub arena_done_today: u32,          // Arena attempts completed today (three free attempts daily).
    pub buy_gold_today: bool,           // Whether gold conversion was performed today.
    pub artifact_lottery_today: bool,   // Whether fishing was performed today.
    pub genie_1_today: bool,            // Wei Genie already swept.
    pub genie_2_today: bool,            // Shu Genie already swept.
    pub genie_3_today: bool,            // Wu Genie already swept.
    pub genie_4_today: bool,            // Qun Genie already swept.
    pub gacha_today: bool,              // Whether a capsule draw was performed today.
}

impl DailyTaskStatus {
    /// done task number
    pub fn completed_count(&self) -> u32 {
        [self.login, self.share, self.friend_gift, self.recruit,
         self.hangup_reward, self.buy_gold, self.open_box,
         self.store_purchase, self.arena, self.bottle]
            .iter().filter(|&&x| x).count() as u32
    }

    /// pending task id list
    pub fn pending_task_ids(&self) -> Vec<u32> {
        let tasks = [
            (1, self.login), (2, self.share), (3, self.friend_gift),
            (4, self.recruit), (5, self.hangup_reward), (6, self.buy_gold),
            (7, self.open_box), (12, self.store_purchase),
            (13, self.arena), (14, self.bottle),
        ];
        tasks.iter().filter(|(_, done)| !done).map(|(id, _)| *id).collect()
    }

    /// format state digest
    pub fn summary(&self) -> String {
        let tasks = [
            (1,  "登录游戏",     self.login),
            (2,  "分享游戏",     self.share),
            (3,  "赠送好友金币", self.friend_gift),
            (4,  "进行招募",     self.recruit),
            (5,  "挂机奖励",     self.hangup_reward),
            (6,  "免费点金",     self.buy_gold),
            (7,  "开启宝箱",     self.open_box),
            (12, "黑市采购",     self.store_purchase),
            (13, "竞技场战斗",   self.arena),
            (14, "盐罐收获",     self.bottle),
        ];
        let mut lines = Vec::new();
        for (id, name, done) in tasks {
            let mark = if done { "+" } else { "-" };
            lines.push(format!("  [{}] {:>2}. {}", mark, id, name));
        }
        lines.push(format!("  DONE: {}/10", self.completed_count()));
        lines.join("\n")
    }
}

// ============================================================
// Task Report
// ============================================================

/// TaskResult
#[derive(Debug, Clone)]
pub enum TaskResult {
    Ok(String),
    Skipped(String),
    Failed(String, String),
}

/// Report for daily task
#[derive(Debug, Clone)]
pub struct DailyTaskReport {
    pub results: Vec<TaskResult>,
}

impl DailyTaskReport {
    pub fn new() -> Self {
        Self { results: Vec::new() }
    }

    pub fn run(&mut self, name: &str, result: Result<Value, String>) {
        match result {
            Ok(_) => {
                info!(target: "task", "[OK] {}", name);
                self.results.push(TaskResult::Ok(name.to_string()));
            }
            Err(e) => {
                if let Some(code) = crate::error_codes::extract_code_from_error(&e) {
                    if crate::error_codes::is_done_error(code) {
                        info!(target: "task", code = code, error = %e, "[~~] {}", name);
                        self.results.push(TaskResult::Skipped(name.to_string()));
                    } else {
                        warn!(target: "task", code = code, error = %e, "[X] {}", name);
                        self.results.push(TaskResult::Failed(name.to_string(), e));
                    }
                } else {
                    warn!(target: "task", error = %e, "[X] {}", name);
                    self.results.push(TaskResult::Failed(name.to_string(), e));
                }
            }
        }
    }

    pub fn skip(&mut self, name: &str) {
        info!(target: "task", "[~~] {}", name);
        self.results.push(TaskResult::Skipped(name.to_string()));
    }

    pub fn fail(&mut self, name: &str, err: &str) {
        warn!(target: "task", error = err, "[X] {}", name);
        self.results.push(TaskResult::Failed(name.to_string(), err.to_string()));
    }

    // --- with log prefix (for scheduler) ---
    pub fn run_with_prefix(&mut self, _prefix: &str, name: &str, result: &Result<Value, String>) {
        match result {
            Ok(_) => {
                info!(target: "task", "[OK] {}", name);
                self.results.push(TaskResult::Ok(name.to_string()));
            }
            Err(e) => {
                if let Some(code) = crate::error_codes::extract_code_from_error(e) {
                    if crate::error_codes::is_done_error(code) {
                        info!(target: "task", code = code, error = %e, "[~~] {}", name);
                        self.results.push(TaskResult::Skipped(name.to_string()));
                    } else {
                        warn!(target: "task", code = code, error = %e, "[X] {}", name);
                        self.results.push(TaskResult::Failed(name.to_string(), e.clone()));
                    }
                } else {
                    warn!(target: "task", error = %e, "[X] {}", name);
                    self.results.push(TaskResult::Failed(name.to_string(), e.clone()));
                }
            }
        }
    }

    pub fn skip_with_prefix(&mut self, _prefix: &str, name: &str) {
        info!(target: "task", "[~~] {}", name);
        self.results.push(TaskResult::Skipped(name.to_string()));
    }

    pub fn fail_with_prefix(&mut self, _prefix: &str, name: &str, err: &str) {
        warn!(target: "task", error = err, "[X] {}", name);
        self.results.push(TaskResult::Failed(name.to_string(), err.to_string()));
    }

    pub fn ok_count(&self) -> usize {
        self.results.iter().filter(|r| matches!(r, TaskResult::Ok(_))).count()
    }

    pub fn skip_count(&self) -> usize {
        self.results.iter().filter(|r| matches!(r, TaskResult::Skipped(_))).count()
    }

    pub fn fail_count(&self) -> usize {
        self.results.iter().filter(|r| matches!(r, TaskResult::Failed(_, _))).count()
    }

    pub fn summary(&self) -> String {
        format!("=== TASK DONE: {} Succ, {} Skipped, {} Failed, Total {} ===",
            self.ok_count(), self.skip_count(), self.fail_count(), self.results.len())
    }
}

async fn switch_to_context_formation(
    game: &GameClient,
    target_team: u64,
    context: &'static str,
    original_team: Option<u64>,
) -> Result<bool, String> {
    let Some(original_team) = original_team else {
        return Ok(false);
    };
    if original_team == target_team {
        info!(target: "formation", context = context, current = original_team, "formation unchanged");
        return Ok(false);
    }
    game.switch_team(target_team).await?;
    info!(target: "formation", context = context, from = original_team, to = target_team, "formation switch");
    Ok(true)
}

async fn restore_context_formation(
    game: &GameClient,
    switched: bool,
    context: &'static str,
    original_team: Option<u64>,
) -> Result<(), String> {
    if !switched {
        return Ok(());
    }
    let Some(original_team) = original_team else {
        return Ok(());
    };
    let current_team = game.current_team_id().await?;
    if current_team == original_team {
        return Ok(());
    }
    game.switch_team(original_team).await?;
    info!(target: "formation", context = context, from = current_team, to = original_team, "formation restore");
    Ok(())
}

async fn run_dream_shop_daily(
    game: &GameClient,
    report: &mut DailyTaskReport,
    log_prefix: &str,
    dream_shop: &DreamShopConfig,
) {
    info!(target: "daily", enabled = dream_shop.enabled, items = dream_shop.purchase_list.len(), "dream shop resolved");

    if dream_shop.purchase_list.is_empty() {
        report.skip_with_prefix(log_prefix, "dream_shopping (empty list)");
        return;
    }

    let role_info = match game.role_getroleinfo().await {
        Ok(v) => v,
        Err(e) => {
            report.fail_with_prefix(log_prefix, "dream_shopping (get role_info)", &e);
            return;
        }
    };

    let level_id = game.level_id();
    if level_id < 1000 {
        report.skip_with_prefix(log_prefix, "dream_shopping (level_id < 1000)");
        return;
    }

    let merchant = match role_info.pointer("/role/dungeon/merchant").and_then(|v| v.as_object()) {
        Some(v) => v,
        None => {
            report.skip_with_prefix(log_prefix, "dream_shopping (no shop info)");
            return;
        }
    };

    let mut operations = Vec::new();
    for item_key in &dream_shop.purchase_list {
        let Some((merchant_id, item_index)) = parse_dream_purchase_key(item_key) else {
            warn!(target: "daily", item_key = %item_key, "dream shop invalid purchase key");
            continue;
        };
        let merchant_items = match merchant.get(&merchant_id.to_string()).and_then(|v| v.as_array()) {
            Some(v) => v,
            None => continue,
        };
        for (pos, item) in merchant_items.iter().enumerate() {
            if item.as_u64() == Some(item_index) {
                operations.push((merchant_id, item_index, pos as u64));
            }
        }
    }

    if operations.is_empty() {
        report.skip_with_prefix(log_prefix, "dream_shopping (no goods matching)");
        return;
    }

    operations.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.2.cmp(&a.2)));
    let mut success = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;

    for (merchant_id, item_index, pos) in operations {
        let item_name = dream_item_name(merchant_id, item_index);
        let result = game.dungeon_buymerchant(json!({
            "id": merchant_id,
            "index": item_index,
            "pos": pos,
        })).await;

        match &result {
            Ok(resp) => {
                if resp.get("reward").is_some() {
                    success += 1;
                    info!(target: "daily", merchant_id, item_index, pos, item = item_name, "dream shop buy ok");
                } else {
                    skipped += 1;
                    warn!(target: "daily", merchant_id, item_index, pos, item = item_name, "dream shop buy returned without reward");
                }
            }
            Err(e) => {
                if crate::error_codes::is_done_result(&result) {
                    skipped += 1;
                    info!(target: "daily", merchant_id, item_index, pos, item = item_name, error = %e, "dream shop buy skipped");
                } else {
                    failed += 1;
                    warn!(target: "daily", merchant_id, item_index, pos, item = item_name, error = %e, "dream shop buy failed");
                }
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    info!(target: "daily", success, skipped, failed, marked_done = true, "dream shop done");
    report.run_with_prefix(log_prefix, &format!("dream_shopping (Success{} Skipped{} Failed{})", success, skipped, failed), &Ok(json!({})));
}

fn parse_dream_purchase_key(raw: &str) -> Option<(u64, u64)> {
    let (merchant_id, item_index) = raw.split_once('-')?;
    Some((merchant_id.parse().ok()?, item_index.parse().ok()?))
}

fn dream_item_name(merchant_id: u64, item_index: u64) -> &'static str {
    for (mid, items) in DREAM_MERCHANT_ITEMS {
        if *mid == merchant_id {
            if let Some(name) = items.get(item_index as usize) {
                return name;
            }
        }
    }
    "Unknown Goods"
}

fn find_use_team_id_rec(value: &Value) -> Option<u64> {
    match value {
        Value::Object(map) => {
            if let Some(team_id) = map.get("useTeamId").and_then(|v| v.as_u64()) {
                return Some(team_id);
            }
            for child in map.values() {
                if let Some(team_id) = find_use_team_id_rec(child) {
                    return Some(team_id);
                }
            }
            None
        }
        Value::Array(items) => {
            for item in items {
                if let Some(team_id) = find_use_team_id_rec(item) {
                    return Some(team_id);
                }
            }
            None
        }
        _ => None,
    }
}

/// extract_last_login
fn extract_last_login(role_resp: &Value) -> Option<i64> {
    role_resp.pointer("/statistics/lastLoginTime")
        .or_else(|| role_resp.pointer("/role/statistics/lastLoginTime"))
        .or_else(|| role_resp.pointer("/role/lastLoginAt"))
        .and_then(|v| v.as_i64())
}

fn tower_id_from_role_info(role_info: &Value) -> Option<u64> {
    role_info
        .pointer("/role/tower/id")
        .or_else(|| role_info.pointer("/tower/id"))
        .and_then(Value::as_u64)
}

fn tower_is_cleared(role_info: &Value) -> bool {
    tower_id_from_role_info(role_info).unwrap_or(0) >= TOWER_CLEAR_ID
}

fn skinc_remaining_hp(response: &Value) -> Option<f64> {
    response
        .pointer("/battleData/result/accept/ext/curHP")
        .and_then(Value::as_f64)
}

fn skinc_current_floor(tower_type: u64, level_reward_map: &serde_json::Map<String, Value>) -> u64 {
    for floor in (1..=8).rev() {
        if level_reward_map.contains_key(&format!("{}00{}", tower_type, floor)) {
            return if floor == 8 { 8 } else { floor + 1 };
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::{DailyTaskReport, skinc_current_floor, skinc_remaining_hp, tower_is_cleared};
    use serde_json::json;

    #[test]
    fn skinc_remaining_hp_accepts_integer_and_float_values() {
        let integer = json!({"battleData": {"result": {"accept": {"ext": {"curHP": 0}}}}});
        let float = json!({"battleData": {"result": {"accept": {"ext": {"curHP": 12.5}}}}});

        assert_eq!(skinc_remaining_hp(&integer), Some(0.0));
        assert_eq!(skinc_remaining_hp(&float), Some(12.5));
    }

    #[test]
    fn skinc_remaining_hp_rejects_missing_or_non_numeric_values() {
        let missing = json!({"battleData": {"result": {}}});
        let text = json!({"battleData": {"result": {"accept": {"ext": {"curHP": "0"}}}}});

        assert_eq!(skinc_remaining_hp(&missing), None);
        assert_eq!(skinc_remaining_hp(&text), None);
    }

    #[test]
    fn skinc_current_floor_follows_level_reward_map() {
        let no_progress = json!({});
        let floor_three = json!({"1001": true, "1002": true});
        let complete = json!({"6008": true});

        assert_eq!(skinc_current_floor(1, no_progress.as_object().unwrap()), 1);
        assert_eq!(skinc_current_floor(1, floor_three.as_object().unwrap()), 3);
        assert_eq!(skinc_current_floor(6, complete.as_object().unwrap()), 8);
    }

    #[test]
    fn tower_clear_state_is_detected_from_role_info() {
        assert!(!tower_is_cleared(&json!({"role": {"tower": {"id": 4499}}})));
        assert!(tower_is_cleared(&json!({"role": {"tower": {"id": 4500}}})));
        assert!(tower_is_cleared(&json!({"tower": {"id": 4501}})));
    }

    #[test]
    fn done_errors_are_reported_as_skipped() {
        let mut report = DailyTaskReport::new();

        report.run_with_prefix("test", "tower reward", &Err("[200120] done".to_string()));

        assert_eq!(report.skip_count(), 1);
        assert_eq!(report.fail_count(), 0);
    }
}

/// randomSeed
/// seed = (lastLoginTime ^ XOR_A) rotate16 ^ XOR_B ^ XOR_C
pub fn generate_random_seed(last_login_time: i64) -> u32 {
    let mut seed = (last_login_time as i32) as u32;
    seed ^= XOR_A;
    seed = (seed << 16) | (seed >> 16);
    seed ^= XOR_B;
    seed ^= XOR_C;
    seed
}

/// is_today_available
pub fn is_today_available(ts: Option<&Value>) -> bool {
    let ts = match ts.and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))) {
        Some(v) => v,
        None => return true,
    };
    let today = chrono::Local::now().date_naive();
    let record_date = chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.with_timezone(&chrono::Local).date_naive());
    match record_date {
        Some(d) => today != d,
        None => true,
    }
}

// ============================================================
// CarSend (smart_send_car, claim_all_cars)
// ============================================================

impl GameClient {
    /// Smart CarSend
    ///
    /// Mon/Tue (tickets): tickets/binggo -> Send, otherwise refresh till the tickets be exhausted -> Send
    /// Wen (BINGO): BINGO -> Send, otherwise refresh till the tickets be exhausted -> Send
    pub async fn smart_send_car(&self, log_prefix: &str) -> DailyTaskReport {
        let mut report = DailyTaskReport::new();
        let delay = || async { tokio::time::sleep(tokio::time::Duration::from_millis(500)).await };

        if !Self::is_car_send_window() {
            report.skip_with_prefix(log_prefix, "Smart CarSend (invalid time)");
            return report;
        }

        let is_wed = chrono::Local::now().weekday().num_days_from_monday() == 2;
        let strategy = if is_wed { "Wensday BINGO" } else { "Monday/Tuesday tickets" };
        info!(target: "car", strategy, "Smart CarSend");

        let car_resp = match self.car_getrolecar().await {
            Ok(v) => v,
            Err(e) => {
                report.fail_with_prefix(log_prefix, "acquire car infomation", &e);
                return report;
            }
        };
        let car_list = Self::normalize_car_list(&car_resp);
        let total_cars = car_list.len();

        let role_car = car_resp.get("roleCar").or_else(|| car_resp.get("body").and_then(|b| b.get("roleCar")));
        let send_cnt: u32 = role_car.and_then(|rc| rc.get("sendCnt")).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let send_cnt_reset_time = role_car.and_then(|rc| rc.get("sendCntResetTime")).and_then(|v| v.as_u64()).unwrap_or(0);
        let today_start = chrono::Local::now().date_naive()
            .and_hms_opt(0, 0, 0).unwrap()
            .and_local_timezone(chrono::Local).single()
            .map(|dt| dt.timestamp() as u64)
            .unwrap_or(0);
        let effective_send_cnt = if send_cnt_reset_time >= today_start { send_cnt } else { 0 };
        let unsent_cars = car_list.iter().filter(|(_, _, send_at, _, _, _)| *send_at == 0).count();
        info!(target: "car", total_cars, unsent_cars, send_cnt, effective_send_cnt, send_cnt_reset_time, today_start, "car list parse done");

        if effective_send_cnt as usize >= total_cars {
            report.skip_with_prefix(log_prefix, "Smart CarSend (already done)");
            return report;
        }
        if unsent_cars == 0 {
            report.skip_with_prefix(log_prefix, "Smart CarSend (no pending car)");
            return report;
        }

        let mut refresh_tickets: u32 = 0;
        let role_info = self.role_getroleinfo().await;
        if let Ok(info) = &role_info {
            refresh_tickets = info.pointer("/role/items/35002/quantity")
                .and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            info!(target: "car", refresh_tickets, "Tickets in package");
        }

        let mut helper_map: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        let mut sorted_helpers: Vec<(String, String, u32)> = Vec::new();
        let current_role_id: Option<String> = match &role_info {
            Ok(info) => info.pointer("/role/roleId").and_then(|v| v.as_u64()).map(|v| v.to_string()),
            Err(_) => None,
        };

        if let Ok(usage) = self.car_getmemberhelpingcnt().await {
            if let Some(map) = usage.get("memberHelpingCntMap").and_then(|v| v.as_object()) {
                for (k, v) in map {
                    helper_map.insert(k.clone(), v.as_u64().unwrap_or(0) as u32);
                }
            }
        }

        if let Ok(legion) = self.legion_getinfo().await {
            let members = legion.pointer("/info/members")
                .or_else(|| legion.pointer("/body/info/members"))
                .and_then(|v| v.as_object());
            if let Some(members) = members {
                sorted_helpers = members.values()
                    .filter(|m| {
                        current_role_id.as_ref().map_or(true, |rid| {
                            m.get("roleId").and_then(|v| v.as_u64()).map_or(true, |id| id.to_string() != *rid)
                        })
                    })
                    .map(|m| {
                        let id = m.get("roleId").and_then(|v| v.as_u64()).unwrap_or(0).to_string();
                        let name = m.get("name").or_else(|| m.get("nickname"))
                            .and_then(|v| v.as_str()).unwrap_or("?").to_string();
                        let rq = m.pointer("/custom/red_quench_cnt")
                            .and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        (id, name, rq)
                    })
                    .collect();
                sorted_helpers.sort_by(|a, b| b.2.cmp(&a.2));
            }
        }

        for (car_id, color, send_at, refresh_count, rewards, _) in &car_list {
            if *send_at != 0 { continue; }

            let mut car_color = *color;
            let mut car_refresh_count = *refresh_count;
            let mut car_rewards = rewards.clone();
            let mut car_helper_id: u64 = 0;

            if car_color >= 5 {
                if let Some(best) = sorted_helpers.iter().find(|(id, _, _)| {
                    *helper_map.get(id).unwrap_or(&0) < 4
                }) {
                    car_helper_id = best.0.parse::<u64>().unwrap_or(0);
                    *helper_map.entry(best.0.clone()).or_insert(0) += 1;
                    info!(target: "car", car_id, color = %Self::grade_label(car_color), helper = %best.1, "automatic distrubute guardian");
                }
            }

            let should_send_now = if is_wed {
                Self::is_big_prize(&car_rewards)
            } else {
                Self::has_refresh_ticket(&car_rewards) || Self::is_big_prize(&car_rewards)
            };

            if should_send_now {
                let reason = if Self::is_big_prize(&car_rewards) { "BINGO" } else { "tickets" };
                info!(target: "car", car_id, color = %Self::grade_label(car_color), reason, "Send immediately");
                let r = self.car_send(car_id, car_helper_id, "").await;
                report.run_with_prefix(log_prefix, &format!("CarSend [{}] {}", Self::grade_label(car_color), reason), &r);
                delay().await;
                continue;
            }

            loop {
                let free_refresh = car_refresh_count == 0;
                let can_refresh = free_refresh || refresh_tickets > 0;

                if !can_refresh {
                    info!(target: "car", car_id, color = %Self::grade_label(car_color), "Tickets exhausted, send car immediately");
                    let r = self.car_send(car_id, car_helper_id, "").await;
                    report.run_with_prefix(log_prefix, &format!("CarSend [{}] tickets exhausted", Self::grade_label(car_color)), &r);
                    delay().await;
                    break;
                }

                info!(target: "car", car_id, color = %Self::grade_label(car_color), free = free_refresh, tickets = refresh_tickets, "Car Refresh");
                let refresh_r = self.car_refresh(car_id).await;
                match &refresh_r {
                    Ok(data) => {
                        let car_data = data.get("car").unwrap_or(data);
                        if let Some(c) = car_data.get("color").and_then(|v| v.as_u64()) { car_color = c as u32; }
                        if let Some(c) = car_data.get("refreshCount").and_then(|v| v.as_u64()) { car_refresh_count = c as u32; }
                        if let Some(r) = car_data.get("rewards").and_then(|v| v.as_array()) { car_rewards = r.clone(); }
                    }
                    Err(e) => {
                        report.fail_with_prefix(log_prefix, "Car Refresh", e);
                        break;
                    }
                }

                if !free_refresh {
                    refresh_tickets = refresh_tickets.saturating_sub(1);
                }

                let should_send = if is_wed {
                    Self::is_big_prize(&car_rewards)
                } else {
                    Self::has_refresh_ticket(&car_rewards) || Self::is_big_prize(&car_rewards)
                };

                if should_send {
                    let reason = if Self::is_big_prize(&car_rewards) { "BINGO" } else { "tickets" };
                    info!(target: "car", car_id, color = %Self::grade_label(car_color), reason, "send after refresh");
                    let r = self.car_send(car_id, car_helper_id, "").await;
                    report.run_with_prefix(log_prefix, &format!("CarSend [{}] After Refresh{}", Self::grade_label(car_color), reason), &r);
                    delay().await;
                    break;
                }

                delay().await;
            }
        }

        info!(target: "car", strategy, "Smart Car Send DONE");
        report
    }

    /// Car Claim+Udate Engine + Rewards Claim
    pub async fn claim_all_cars(&self, log_prefix: &str) -> (DailyTaskReport, f64) {
        let mut report = DailyTaskReport::new();
        let delay = || async { tokio::time::sleep(tokio::time::Duration::from_millis(500)).await };

        let car_resp = match self.car_getrolecar().await {
            Ok(v) => v,
            Err(e) => {
                report.fail_with_prefix(log_prefix, "Obtail Car Info", &e);
                return (report, 0.0);
            }
        };
        let car_list = Self::normalize_car_list(&car_resp);

        let mut research_level: u32 = car_resp.get("roleCar")
            .or_else(|| car_resp.get("body").and_then(|b| b.get("roleCar")))
            .and_then(|rc| rc.pointer("/research/1"))
            .and_then(|v| v.as_u64()).unwrap_or(0) as u32;

        let mut claimed = 0u32;
        for (car_id, color, send_at, _, _, _) in &car_list {
            if !Self::can_claim_car(*send_at) { continue; }

            match self.car_claim(car_id).await {
                Ok(_) => {
                    claimed += 1;
                    info!(target: "car", car_id, color = %Self::grade_label(*color), "CarClaim Success");
                    report.run_with_prefix(log_prefix, &format!("CarClaim [{}]", Self::grade_label(*color)), &Ok(json!({})));
                }
                Err(e) => {
                    report.fail_with_prefix(log_prefix, &format!("CarClaim [{}]", Self::grade_label(*color)), &e);
                    delay().await;
                    continue;
                }
            }
            delay().await;

            let role_info = self.role_getroleinfo().await;
            let mut pieces: u32 = 0;
            if let Ok(info) = &role_info {
                pieces = info.pointer("/role/items/35009/quantity")
                    .and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            }

            while (research_level as usize) < Self::CAR_RESEARCH_COSTS.len()
                && pieces >= Self::CAR_RESEARCH_COSTS[research_level as usize]
            {
                match self.car_research(json!({"researchId": 1})).await {
                    Ok(_) => {
                        research_level += 1;
                        info!(target: "car", level = research_level, "Update engine");
                        if let Ok(info) = self.role_getroleinfo().await {
                            pieces = info.pointer("/role/items/35009/quantity")
                                .and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        }
                    }
                    Err(e) => {
                        report.fail_with_prefix(log_prefix, "Update engine", &e);
                        break;
                    }
                }
                delay().await;
            }

            let _ = self.car_claimpartconsumereward().await;
        }

        let next_claim = car_list.iter()
            .filter(|(_, _, send_at, _, _, _)| *send_at != 0)
            .map(|(_, _, send_at, _, _, _)| {
                let ts = if *send_at < 1_000_000_000_000 { *send_at as f64 } else { *send_at as f64 / 1000.0 };
                ts + 4.0 * 3600.0
            })
            .fold(0.0, |a, b| if a == 0.0 || b < a { b } else { a });

        if claimed == 0 {
            report.skip_with_prefix(log_prefix, "Car Cliaim (No Car)");
        } else {
            info!(target: "car", claimed, "Car claim done");
        }
        (report, next_claim)
    }
}
