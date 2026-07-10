use std::sync::Mutex;

pub struct HttpClient {
    client: reqwest::Client,
    base_url: String,
    rate_limiter: Mutex<RateLimiter>,
}

struct RateLimiter { max_requests: u32, window_ms: u64, requests: Vec<u64> }

impl RateLimiter {
    fn new(max_requests: u32, window_ms: u64) -> Self { Self { max_requests, window_ms, requests: Vec::new() } }
    fn clean_old(&mut self, now: u64) { let cutoff = now.saturating_sub(self.window_ms); self.requests.retain(|t| *t > cutoff); }
    fn add_request(&mut self, now: u64) { self.clean_old(now); if self.requests.len() < self.max_requests as usize { self.requests.push(now); } }
}

impl HttpClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            client,
            base_url: String::from("https://xxz-xyzw.hortorgames.com"),
            rate_limiter: Mutex::new(RateLimiter::new(25, 60000))
        }
    }

    pub fn with_base_url(base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            client,
            base_url: base_url.to_string(),
            rate_limiter: Mutex::new(RateLimiter::new(25, 60000))
        }
    }

    pub async fn post_authuser(&self, bin_data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as u64;
        { let mut limiter = self.rate_limiter.lock().unwrap(); limiter.add_request(now); }

        let url = format!("{}/login/authuser?_seq=1", self.base_url);
        let response = self.client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .header("Referrer-Policy", "no-referrer")
            .body(bin_data.to_vec())
            .send()
            .await?
            .bytes()
            .await?;
        Ok(response.to_vec())
    }

    pub async fn post_serverlist(&self, bin_data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as u64;
        { let mut limiter = self.rate_limiter.lock().unwrap(); limiter.add_request(now); }

        let url = format!("{}/login/serverlist?_seq=3", self.base_url);
        let response = self.client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .header("Referrer-Policy", "no-referrer")
            .body(bin_data.to_vec())
            .send()
            .await?
            .bytes()
            .await?;
        Ok(response.to_vec())
    }
}

impl Default for HttpClient { fn default() -> Self { Self::new() } }
