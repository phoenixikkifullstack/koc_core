use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, RwLock};
use tracing::{info, warn};

use crate::error_codes;
use crate::kpi::DailyTaskReport;
use crate::websocket::WebSocketClient;

// ============================================================
// Question
// ============================================================

/// embedded JSON while compile
const EMBEDDED_ANSWER_JSON: &str = include_str!("../data/answer.json");

/// Question
#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnswerItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: u32,
}

/// Global Question: HashMap<Qeustion(normalization)>
/// parse from embedded JSON in the first time
static ANSWER_MAP: LazyLock<RwLock<HashMap<String, u32>>> = LazyLock::new(|| {
    let map = parse_answer_json(EMBEDDED_ANSWER_JSON);
    info!(target: "study", count = map.len(), "embedded question bank loaded");
    RwLock::new(map)
});

/// Json -> Hashmap
fn parse_answer_json(json_str: &str) -> HashMap<String, u32> {
    let items: Vec<AnswerItem> = serde_json::from_str(json_str).unwrap_or_default();
    let mut map = HashMap::with_capacity(items.len());
    for item in items {
        if !item.name.is_empty() && item.value > 0 {
            let key = normalize_text(&item.name);
            map.insert(key, item.value);
        }
    }
    map
}

/// normalization text :trim space + tolower
fn normalize_text(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

// ============================================================
// Load question (hot reload)
// ============================================================

/// load external json file, will cover embedded
/// return question count
pub fn load_external(path: &Path) -> Result<usize, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let map = parse_answer_json(&content);
    let count = map.len();
    if count == 0 {
        return Err("External answer.json is empty or invalid".to_string());
    }
    let mut global = ANSWER_MAP.write().unwrap();
    *global = map;
    info!(target: "study", count = count, path = %path.display(), "external question bank loaded");
    Ok(count)
}

/// Try load external
pub fn try_load_external(path: Option<&str>) {
    if let Some(p) = path {
        let path = Path::new(p);
        if path.exists() {
            match load_external(path) {
                Ok(n) => info!(target: "study", count = n, "external question bank load succeeded"),
                Err(e) => warn!(target: "study", error = %e, "external question bank load failed, fallback to embedded"),
            }
        }
    }
}

/// Question Count
pub fn question_count() -> usize {
    ANSWER_MAP.read().unwrap().len()
}

// ============================================================
// Match answer
// ============================================================

/// Match answer
/// 1. match whole world
/// 2. match parts
/// 3. default -> 1
pub fn find_answer(question: &str) -> u32 {
    let clean = normalize_text(question);
    if clean.is_empty() {
        return 1;
    }

    let map = ANSWER_MAP.read().unwrap();

    // match whole world
    if let Some(&v) = map.get(&clean) {
        return v;
    }

    // match parts
    for (k, &v) in map.iter() {
        if clean.contains(k.as_str()) || k.contains(clean.as_str()) {
            return v;
        }
    }

    // default 1
    1
}

// ============================================================
// state check(Done or Pending)
// ============================================================

/// Is time in current week
fn is_in_current_week(timestamp_secs: i64) -> bool {
    use chrono::{DateTime, Datelike, Local};

    let now = Local::now();
    let now_week = now.iso_week().week();
    let now_year = now.iso_week().year();

    match DateTime::from_timestamp(timestamp_secs, 0) {
        Some(dt_utc) => {
            let dt_local = dt_utc.with_timezone(&Local);
            dt_local.iso_week().week() == now_week && dt_local.iso_week().year() == now_year
        }
        None => false,
    }
}

/// Is study completed this week
/// condition: role.study.maxCorrectNum >= 10 && isInCurrentWeek(role.study.beginTime)
pub fn is_study_completed_this_week(role_info: Option<&Value>) -> bool {
    let info = match role_info {
        Some(v) => v,
        None => return false,
    };

    let study = info.pointer("/role/study")
        .or_else(|| info.pointer("/study"));

    let study = match study {
        Some(v) => v,
        None => return false,
    };

    let max_correct = study.get("maxCorrectNum")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let begin_time = study.get("beginTime")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    max_correct >= 10 && is_in_current_week(begin_time)
}

// ============================================================
// study process
// ============================================================

/// whole study process:
/// 1. study_startgame (wait response, obtain questionList + studyId)
/// 2. question: find_answer → study_answer (fire-and-forget, snap 300ms)
/// 3. wait for 1500ms
/// 4. study_claimreward x10 (fire-and-forget, snap 200ms)
pub async fn run_study(
    ws: &WebSocketClient,
    report: &mut DailyTaskReport,
    log_prefix: &str,
) -> Result<(), String> {
    let delay_ms = |ms: u64| tokio::time::sleep(tokio::time::Duration::from_millis(ms));

    // Step 1: start study
    let resp = ws.send_with_response("study_startgame", Some(json!({})), 10000).await;

    let resp = match resp {
        Ok(v) => v,
        Err(e) => {
            if error_codes::is_done_result(&Err(e.clone())) {
                report.run_with_prefix(log_prefix, "STUDY", &Err(e));
                return Ok(()); // no need retry
            }
            report.fail_with_prefix(log_prefix, "STUDY BEGIN", &e);
            return Err(e); // need retry
        }
    };

    // Step 2: get question list and studyId
    let question_list = resp.pointer("/questionList")
        .or_else(|| resp.pointer("/role/study/questionList"))
        .and_then(|v| v.as_array());

    let study_id = resp.pointer("/role/study/id")
        .or_else(|| resp.pointer("/studyId"))
        .and_then(|v| v.as_u64());

    let (questions, study_id) = match (question_list, study_id) {
        (Some(q), Some(id)) => (q.clone(), id),
        _ => {
            report.fail_with_prefix(log_prefix, "STUDY", "can not found question list or studyId");
            return Err("Missing questionList or studyId in study_startgame response".to_string());
        }
    };

    let total = questions.len();
    info!(target: "study", prefix = log_prefix, total = total, study_id = study_id, "study questions loaded");

    // Step 3: answer one by one
    let mut correct = 0;
    for (i, question) in questions.iter().enumerate() {
        let question_text = question.get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let question_id = question.get("id")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let answer = find_answer(question_text);

        // fire-and-forget
        let _ = ws.send("study_answer", Some(json!({
            "id": study_id,
            "option": [answer],
            "questionId": [question_id]
        }))).await;

        if answer > 0 { correct += 1; }

        if i < total - 1 {
            delay_ms(300).await;
        }
    }

    info!(target: "study", prefix = log_prefix, submitted = total, total = total, "study answers submitted");

    // Step 4: snap for a while
    delay_ms(1500).await;

    // Step 5: claim reward (rewardId 1-10)
    for reward_id in 1..=10u64 {
        let _ = ws.send("study_claimreward", Some(json!({
            "rewardId": reward_id
        }))).await;
        delay_ms(200).await;
    }

    report.run_with_prefix(log_prefix, &format!("DONE STUDY ({}/{}Q)", correct, total), &Ok(json!({})));
    Ok(())
}

// ============================================================
// test
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_question_count() {
        let count = question_count();
        assert!(count > 500, "Expected > 500 questions, got {}", count);
        println!("Embedded questions: {}", count);
    }

    #[test]
    fn test_find_answer_exact() {
        let answer = find_answer("《三国演义》中，「大意失街亭」的是马谩？");
        assert_eq!(answer, 1);
    }

    #[test]
    fn test_find_answer_default() {
        let answer = find_answer("这是一道不存在的题目12345");
        assert_eq!(answer, 1);
    }

    #[test]
    fn test_normalize_text() {
        assert_eq!(normalize_text("Hello World"), "helloworld");
        assert_eq!(normalize_text("  A  B  C  "), "abc");
    }

    #[test]
    fn test_is_in_current_week() {
        let now = chrono::Local::now().timestamp();
        assert!(is_in_current_week(now));
        // 1 year ago should not be in current week
        assert!(!is_in_current_week(now - 365 * 86400));
    }
}
