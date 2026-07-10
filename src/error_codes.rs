use std::collections::HashMap;
use std::sync::LazyLock;

/// error code from server -> chinese description map
static ERROR_CODE_MAP: LazyLock<HashMap<i32, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    // general
    m.insert(200020, "Something went wrong; try restarting the client");
    m.insert(200160, "Feature not unlocked");
    m.insert(200330, "Invalid ID");
    m.insert(200400, "Action performed too quickly; try again later");
    m.insert(200750, "Arena battle attempts exhausted");
    m.insert(200760, "Interface state changed; log in again");
    // cargo
    m.insert(400000, "Item does not exist");
    m.insert(400010, "Insufficient item quantity");
    m.insert(400030, "Daily free attempts exhausted");
    m.insert(400190, "No check-in rewards available to claim");
    m.insert(400340, "Attempts exhausted");
    // task
    m.insert(700010, "Task completion conditions not met");
    m.insert(700020, "Task already claimed");
    // singin/bonus
    m.insert(1000020, "Today's reward already claimed");
    // shop
    m.insert(1300040, "No Black Market purchase option is configured");
    m.insert(1300050, "Adjust the purchase quantity");
    // month card
    m.insert(1400010, "No monthly card purchased; cannot claim daily rewards");
    // nightmare
    m.insert(2600040, "Salted-Fish King Dreamscape challenge attempts exhausted");
    m.insert(2600050, "Salted-Fish King Dreamscape cleared or needs no further action");
    m.insert(2600080, "Salted-Fish King Dreamscape cleared or needs no further action");
    // tower
    m.insert(1500010, "All floors cleared");
    m.insert(1500020, "Insufficient energy");
    m.insert(1500040, "Previous tower reward has not been claimed");
    // evotower
    m.insert(2100010, "Evo Tower event is not active or has ended");
    m.insert(12200020, "Evo Tower has pending chapter rewards or challenge state");
    m.insert(12200040, "Evo Tower task conditions are not met; cannot claim yet");
    m.insert(12200050, "Evo Tower task already claimed; cannot claim again");
    // club
    m.insert(2300070, "Not a club member");
    m.insert(2300190, "Already checked in today");
    m.insert(2300250, "Today's Club Boss attempts exhausted");
    m.insert(2300370, "Club purchase limit exceeded");
    // genie/sweep
    m.insert(3300050, "Purchase limit exceeded");
    m.insert(3300060, "Sweep conditions not met");
    // something else
    m.insert(3500020, "No rewards available to claim");
    // password
    m.insert(7500100, "Incorrect password");
    m.insert(7500120, "Password attempt limit reached");
    m.insert(7500140, "Enter the password first");
    // counter
    m.insert(7900023, "Usage limit reached");
    // negtive code
    m.insert(-10006, "Today's reward already claimed or attempts exhausted");
    // car
    m.insert(12000050, "Daily vehicle dispatch limit reached");
    m.insert(12000060, "Outside the vehicle dispatch window");
    m.insert(12000116, "Today's free reward already claimed");
    // tower
    m.insert(200120, "Reward does not exist or cannot be claimed");
    // merge box
    m.insert(12300040, "No empty slots remain");
    m.insert(12300080, "Unlock conditions not met");
    // hangup
    m.insert(12400000, "Hangup rewards are being claimed too frequently");
    m
});

/// match chinese map, otherwise return None
pub fn lookup(code: i32) -> Option<&'static str> {
    ERROR_CODE_MAP.get(&code).copied()
}

/// format error infomations: errorCodeMap --> hint --> code
pub fn format_error(code: i32, hint: Option<&str>) -> String {
    if let Some(desc) = lookup(code) {
        format!("[{}] {}", code, desc)
    } else if let Some(h) = hint {
        format!("[{}] {}", code, h)
    } else {
        format!("[{}] unknown code", code)
    }
}

/// codes below specified task were "DONE/CANNOT HANDLER/NO MORE RETRY", mark 'done' and no more try
const DONE_ERROR_CODES: &[i32] = &[
    // --- done/exhausted ---
    400030,   // Today's free attempts are exhausted.
    400190,   // No check-in rewards are available.
    400340,   // Today's attempts are exhausted.
    700020,   // This task has already been claimed.
    1000020,  // Today's reward has already been claimed.
    2300190,  // Already checked in today.
    2300250,  // Today's Club Boss attempts are exhausted.
    3300050,  // The purchase limit has been reached.
    3500020,  // No rewards are available.
    7900023,  // The usage limit has been reached.
    -10006,   // Today's reward has been claimed or attempts are exhausted.
    12000116, // Today's free reward has already been claimed.
    200750,   // Arena battle attempts are exhausted.
    12400000, // Hangup rewards are being claimed too frequently.
    // --- condtion not be reached (it's no need to try) ---
    200020,   // Generic issue, usually insufficient level or prerequisites.
    200120,   // The reward does not exist or cannot be claimed.
    200160,   // The feature is unavailable because the role level is too low.
    2100010,  // The Evo Tower event is not active or has ended.
    12200050, // The Evo Tower task was already claimed and cannot be claimed again.
    400000,   // The item does not exist.
    400010,   // Insufficient item quantity.
    1300040,  // No Black Market purchase option is configured.
    1300050,  // The purchase count must be changed.
    1400010,  // No monthly card was purchased; this is permanent.
    2600040,  // Salted-Fish King Dreamscape attempts are exhausted.
    2600050,  // Salted-Fish King Dreamscape is cleared or needs no further action.
    2600080,  // Salted-Fish King Dreamscape is cleared or needs no further action.
    2300070,  // The role has not joined a club; this is permanent.
    2300370,  // The club purchase limit has been exceeded.
    3300060,  // Sweep conditions are not met.
];

/// some error code means "alread done, no more try"
pub fn is_done_error(code: i32) -> bool {
    DONE_ERROR_CODES.contains(&code)
}

/// format: "[12345] " → Some(12345)
pub fn extract_code_from_error(err: &str) -> Option<i32> {
    if err.starts_with('[') {
        if let Some(end) = err.find(']') {
            return err[1..end].parse().ok();
        }
    }
    None
}

/// same as @is_done_error
pub fn is_done_result(result: &Result<serde_json::Value, String>) -> bool {
    match result {
        Ok(_) => true,
        Err(e) => {
            if let Some(code) = extract_code_from_error(e) {
                is_done_error(code)
            } else {
                false
            }
        }
    }
}
