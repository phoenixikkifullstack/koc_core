use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::hortor_crypto;
use crate::logging::{ui_log_summary, ui_println};
use crate::{bon, crypto};

// ============================================================
// CONSTANT
// ============================================================

const WX_APPID: &str = "wxfb0d5667e5cb1c44";
const WX_BUNDLE_ID: &str = "com.hortor.games.xyzw";
const WX_SCOPE: &str = "snsapi_base,snsapi_userinfo,snsapi_friend,snsapi_message";

const WX_QR_BASE: &str = "https://open.weixin.qq.com";
const HORTOR_LOGIN_URL: &str = "https://comb-platform.hortorgames.com/comb-login-server/api/v1/login";

const WX_UA: &str = "Mozilla/5.0 (Linux; Android 7.0; Mi-4c Build/NRD90M; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/53.0.2785.49 Mobile MQQBrowser/6.2 TBS/043632 Safari/537.36 MicroMessenger/6.6.1.1220(0x26060135) NetType/WIFI Language/zh_CN";
const HORTOR_UA: &str = "Mozilla/5.0 (Linux; Android 12; 23117RK66C Build/V417IR; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/95.0.4638.74 Mobile Safari/537.36";

const POLL_INTERVAL_MS: u64 = 1000;
const POLL_TIMEOUT_SECS: u64 = 120;

// ============================================================
// WxLoginClient
// ============================================================

pub struct WxLoginClient {
    http: reqwest::Client,
}

impl WxLoginClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self { http }
    }

    // ============================================================
    // Step 1: Obtail WeChat QRCode
    // ============================================================

    /// obtail wechat qrcode
    /// returns: (qr_image_url, uuid)
    pub async fn get_qr_code(&self) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "{}/connect/app/qrconnect?appid={}&bundleid={}&scope={}&state=weixin",
            WX_QR_BASE, WX_APPID, WX_BUNDLE_ID, WX_SCOPE
        );

        let resp = self.http
            .get(&url)
            .header("User-Agent", WX_UA)
            .header("Referer", "https://open.weixin.qq.com/")
            .send()
            .await?
            .text()
            .await?;

        // acquire QRCode image url from html
        // match: <img class="auth_qrcode" src="..."
        let qr_url = extract_regex(&resp, "auth_qrcode")
            .or_else(|| {
                // fallback: get uuid from cgiData, construct QR URL
                extract_regex(&resp, "cgiData_uuid").map(|uuid| {
                    format!("{}/connect/qrcode/{}", WX_QR_BASE, uuid)
                })
            })
            .ok_or("Failed to extract QR code URL from response")?;

        // check url
        let qr_url = if qr_url.starts_with("http") {
            qr_url
        } else {
            format!("{}{}", WX_QR_BASE, qr_url)
        };

        // acquire UUID from QR URL(the last field)
        let uuid = qr_url
            .split('/')
            .last()
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("")
            .to_string();

        if uuid.is_empty() {
            return Err("Failed to extract UUID from QR code URL".into());
        }

        Ok((qr_url, uuid))
    }

    /// Download QRCode image -> identify content -> render ASCII in terminal
    pub async fn display_qr_terminal(&self, image_url: &str) {
        // Step 1: download QRCode image
        let image_bytes = match self.http
            .get(image_url)
            .header("User-Agent", WX_UA)
            .header("Referer", "https://open.weixin.qq.com/")
            .send()
            .await
            .and_then(|r| Ok(r))
        {
            Ok(resp) => match resp.bytes().await {
                Ok(b) => b.to_vec(),
                Err(e) => {
                    warn!(target: "token_gen", error = %e, "failed to download QR image");
                    ui_println(format!("Please open this URL in browser to scan: {}", image_url));
                    return;
                }
            },
            Err(e) => {
                warn!(target: "token_gen", error = %e, "failed to download QR image");
                ui_println(format!("Please open this URL in browser to scan: {}", image_url));
                return;
            }
        };

        // Step 2: decode image -> identify content from QRCode
        let qr_content = match decode_qr_from_image(&image_bytes) {
            Some(content) => content,
            None => {
                warn!(target: "token_gen", "failed to decode QR image, fallback to URL");
                ui_println(format!("Please open this URL in browser to scan: {}", image_url));
                return;
            }
        };

        // Step 3: re-encode qrcode crate to terminal ASCII
        use qrcode::QrCode;
        use qrcode::render::unicode;

        match QrCode::new(qr_content.as_bytes()) {
            Ok(code) => {
                let image = code
                    .render::<unicode::Dense1x2>()
                    .dark_color(unicode::Dense1x2::Light)
                    .light_color(unicode::Dense1x2::Dark)
                    .build();
                println!("{}", image);
                ui_log_summary("qr_ascii_rendered", format!("image_url={} qr_content_len={}", image_url, qr_content.len()));
            }
            Err(e) => {
                warn!(target: "token_gen", error = %e, "failed to render QR code");
                ui_println(format!("Please open this URL in browser to scan: {}", image_url));
            }
        }
    }

    // ============================================================
    // Step 2: polling scaning state
    // ============================================================

    /// polling user's scaning action
    /// internal 1s, up limit 120s
    /// returns: (wx_oauth_code, nickname)
    pub async fn poll_scan(&self, uuid: &str) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(POLL_TIMEOUT_SECS);

        loop {
            if start.elapsed() > timeout {
                return Err("QR code scan timeout (120s)".into());
            }

            let remaining = POLL_TIMEOUT_SECS - start.elapsed().as_secs().min(POLL_TIMEOUT_SECS);
            if remaining % 30 == 0 && remaining > 0 && remaining < POLL_TIMEOUT_SECS {
                info!(target: "token_gen", remaining_s = remaining, "waiting for scan confirmation");
            }

            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis();

            let poll_url = format!(
                "{}/connect/l/qrconnect?uuid={}&f=url&_={}",
                WX_QR_BASE, uuid, ts
            );

            let resp = match self.http
                .get(&poll_url)
                .header("User-Agent", WX_UA)
                .header("Referer", "https://open.weixin.qq.com/")
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await
            {
                Ok(r) => r.text().await.unwrap_or_default(),
                Err(_) => {
                    // network error, keep going
                    tokio::time::sleep(tokio::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
                    continue;
                }
            };

            // check QR expired
            if resp.contains("window.wx_errcode=408") {
                return Err("QR code expired".into());
            }

            // Scaning Success
            if resp.contains("window.wx_errcode=405") {
                // extract OAuth code
                let code = extract_regex(&resp, "wx_code")
                    .ok_or("Failed to extract OAuth code from scan response")?;

                // extract nickname
                let nickname = extract_regex(&resp, "wx_nickname")
                    .unwrap_or_else(|| "unknown".to_string());

                info!(target: "token_gen", code_len = code.len(), nickname = %nickname, "received OAuth code");

                return Ok((code, nickname));
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
        }
    }

    // ============================================================
    // Step 3: Hortor platform login
    // ============================================================

    /// OAuth code -> Hortor combUser
    pub async fn hortor_login(&self, wx_code: &str) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        // payload
        let payload = json!({
            "gameId": "xyzwapp",
            "code": wx_code,
            "gameTp": "app",
            "sysInfo": r#"{"system":"Android","hortorSDKVersion":"4.0.6-cn","model":"22081212C","brand":"Redmi"}"#,
            "channel": "android",
            "appFrom": "com.tencent.mm",
            "noLogin": "2",
            "distinctId": "DID-a38175b7-14ce-4b36-aa89-3e092ea03ea6",
            "state": "hortor",
            "packageName": "com.hortor.games.xyzw",
            "tp": "app-we",
            "signPrint": "E6:F7:FE:A9:EC:8E:24:D0:4F:2A:32:50:28:78:E1:C5:5E:70:81:13"
        });

        let payload_str = serde_json::to_string(&payload)?;

        // encrypt payload
        let encrypted = hortor_crypto::encode_payload(&payload_str);

        // construct request URL
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();

        let url = format!(
            "{}?gameId=xyzwapp&timestamp={}&version=android-4.2.1-cn-release&cryptVersion=1.1.0&gameTp=app&system=android&deviceUniqueId=DID-0e782e88-2f3b-4f5b-9020-47f5e5a5a026&packageName=com.hortorgames.xyzw",
            HORTOR_LOGIN_URL, ts
        );

        let resp = self.http
            .post(&url)
            .header("User-Agent", HORTOR_UA)
            .header("Content-Type", "text/plain; charset=utf-8")
            .header("Accept", "*/*")
            .body(encrypted)
            .send()
            .await?
            .text()
            .await?;

        // Response parse
        let json: Value = serde_json::from_str(&resp)
            .map_err(|e| format!("Failed to parse Hortor response: {} (body: {})", e, &resp[..200.min(resp.len())]))?;

        let err_code = json.pointer("/meta/errCode").and_then(|v| v.as_i64()).unwrap_or(-1);
        if err_code != 0 {
            let err_msg = json.pointer("/meta/errMsg").and_then(|v| v.as_str()).unwrap_or("unknown");
            return Err(format!("Hortor login failed: code={}, msg={}", err_code, err_msg).into());
        }

        let comb_user = json.pointer("/data/combUser")
            .ok_or("Hortor response missing data.combUser")?
            .clone();

        Ok(comb_user)
    }

    // ============================================================
    // Step 4: combUser → bin
    // ============================================================

    /// combUser -> bin
    /// construct {platform, platformExt, info, serverId, scene, referrerInfo}
    /// → BON encode → "x" encrypt
    pub fn generate_bin(comb_user: &Value, server_id: Option<u64>) -> Result<Vec<u8>, String> {
        let mut data = serde_json::Map::new();
        data.insert("platform".to_string(), json!("hortor"));
        data.insert("platformExt".to_string(), json!("mix"));
        data.insert("info".to_string(), comb_user.clone());
        data.insert("serverId".to_string(), match server_id {
            Some(id) => json!(id),
            None => Value::Null,
        });
        data.insert("scene".to_string(), json!(0));
        data.insert("referrerInfo".to_string(), json!(""));

        // BON encode
        let bon_bytes = bon::encode(&Value::Object(data));

        // Encrypt with "x" scheme (same as g_utils.encode default)
        let enc = crypto::get_encryptor("x");
        Ok(enc.encrypt(&bon_bytes))
    }

    // ============================================================
    // full processing
    // ============================================================

    /// QR → scan → login → bin
    /// returns: (bin path, nickname)
    pub async fn scan_and_save(&self, output_dir: &Path) -> Result<(PathBuf, String), Box<dyn std::error::Error + Send + Sync>> {
        // Step 1: acquire QRCode
        info!(target: "token_gen", "fetching WeChat QR code");
        let (qr_url, uuid) = self.get_qr_code().await?;

        ui_println(format!("[token_gen] Please scan with WeChat ({}s timeout):", POLL_TIMEOUT_SECS));

        // display QRCode in terminal
        self.display_qr_terminal(&qr_url).await;
        info!(target: "token_gen", uuid = %uuid, "qr displayed in terminal");

        // Step 2: polling user's scaning action
        let (wx_code, nickname) = self.poll_scan(&uuid).await?;
        ui_println(format!("[token_gen] Scan confirmed! User: {}", nickname));

        // Step 3: Hortor login
        info!(target: "token_gen", "logging into Hortor platform");
        let comb_user = self.hortor_login(&wx_code).await?;
        info!(target: "token_gen", "hortor login successful");

        // Step 4: get bin
        info!(target: "token_gen", "generating bin file");
        let bin_data = Self::generate_bin(&comb_user, None)?;

        // check output path
        std::fs::create_dir_all(output_dir)
            .map_err(|e| format!("Failed to create output dir {:?}: {}", output_dir, e))?;

        // purne nickname
        let safe_name = sanitize_filename(&nickname);
        let filename = format!("{}.bin", safe_name);
        let output_path = output_dir.join(&filename);

        // duplicat file with timestamp suffix
        let output_path = if output_path.exists() {
            let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let filename = format!("{}_{}.bin", safe_name, ts);
            output_dir.join(filename)
        } else {
            output_path
        };

        std::fs::write(&output_path, &bin_data)
            .map_err(|e| format!("Failed to write bin file {:?}: {}", output_path, e))?;

        info!(target: "token_gen", path = %output_path.display(), bytes = bin_data.len(), "bin saved");

        Ok((output_path, nickname))
    }

    // ============================================================
    // Add to config.yaml
    // ============================================================

    /// avoid rewrite config.yaml, that will change file struct(comments...)
    /// display new YAML section, user can add it by themself
    pub fn add_to_config(config_path: &Path, bin_path: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _ = config_path;
        let bin_name = bin_path.file_name().and_then(|v| v.to_str()).unwrap_or("new.bin");
        Err(format!(
            "Auto-updating config.yaml is disabled to preserve comments. Please append manually:\n  - bin: {}\n    roles:\n      - server_id: <server-id>",
            bin_name
        ).into())
    }
}

// ============================================================
// helper function
// ============================================================

/// Decode QR from image
fn decode_qr_from_image(image_bytes: &[u8]) -> Option<String> {
    use image::ImageReader;
    use std::io::Cursor;

    // decode image
    let img = ImageReader::new(Cursor::new(image_bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;

    let luma = img.to_luma8();

    // identify with rqrr
    let mut prepared = rqrr::PreparedImage::prepare(luma);
    let grids = prepared.detect_grids();

    for grid in grids {
        if let Ok((_, content)) = grid.decode() {
            return Some(content);
        }
    }

    None
}

/// extract regex
fn extract_regex(text: &str, pattern: &str) -> Option<String> {
    match pattern {
        "auth_qrcode" => {
            // <img class="auth_qrcode" src="https://...qrcode/UUID"
            for segment in text.split('"') {
                if segment.contains("/connect/qrcode/") && segment.starts_with("http") {
                    return Some(segment.to_string());
                }
            }
            None
        }
        "cgiData_uuid" => {
            // window.cgiData = { uuid: "XXXXX" }
            if let Some(pos) = text.find("uuid:") {
                let rest = &text[pos + 5..];
                // skip space and quote
                let rest = rest.trim_start();
                let quote = rest.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let inner = &rest[1..];
                    let end = inner.find(quote)?;
                    return Some(inner[..end].to_string());
                }
            }
            if let Some(pos) = text.find("\"uuid\"") {
                let rest = &text[pos + 6..];
                for quote in ['"', '\''] {
                    if let Some(q1) = rest.find(quote) {
                        let after = &rest[q1 + 1..];
                        if let Some(q2) = after.find(quote) {
                            let val = &after[..q2];
                            if !val.is_empty() && val.len() < 100 {
                                return Some(val.to_string());
                            }
                        }
                    }
                }
            }
            None
        }
        "wx_code" => {
            // wx_redirecturl='...code=XXXXX'
            // locate wx_redirecturl first, then search 'code='
            if let Some(pos) = text.find("wx_redirecturl") {
                let rest = &text[pos..];
                if let Some(code_pos) = rest.find("code=") {
                    let start = code_pos + 5;
                    let rest2 = &rest[start..];
                    let end = rest2.find(|c: char| !c.is_alphanumeric()).unwrap_or(rest2.len());
                    if end > 0 {
                        return Some(rest2[..end].to_string());
                    }
                }
            }
            None
        }
        "wx_nickname" => {
            // window.wx_nickname = 'xxx' or "xxx"
            if let Some(pos) = text.find("wx_nickname") {
                let rest = &text[pos..];
                for quote in ['\'', '"'] {
                    if let Some(q1) = rest.find(quote) {
                        let after = &rest[q1 + 1..];
                        if let Some(q2) = after.find(quote) {
                            return Some(after[..q2].to_string());
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// prune some characters
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            '\0' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}
