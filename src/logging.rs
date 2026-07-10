use chrono::{FixedOffset, Utc};
use std::fmt;
use std::sync::OnceLock;
use tracing::{info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::prelude::*;

static APP_NAME: OnceLock<String> = OnceLock::new();

struct LocalTimeFormatter;

impl FormatTime for LocalTimeFormatter {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> fmt::Result {
        let tz = FixedOffset::east_opt(8 * 3600).expect("valid UTC+8 offset");
        write!(
            w,
            "{}",
            Utc::now().with_timezone(&tz).format("%Y-%m-%d %H:%M:%S")
        )
    }
}

pub fn init_logging(app_name: &str) -> WorkerGuard {
    let _ = APP_NAME.set(app_name.to_string());

    let file_appender = tracing_appender::rolling::daily("logs", format!("{}.log", app_name));
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let console_layer = tracing_subscriber::fmt::layer()
        .with_timer(LocalTimeFormatter)
        .with_target(true)
        .with_thread_names(false)
        .with_thread_ids(false)
        .with_ansi(false);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_timer(LocalTimeFormatter)
        .with_target(true)
        .with_thread_names(false)
        .with_thread_ids(false)
        .with_ansi(false)
        .with_writer(file_writer);

    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer);

    if let Err(e) = subscriber.try_init() {
        warn!(target: "logging", error = %e, "tracing subscriber already initialized");
    }

    info!(target: "app", app = app_name, "logging initialized");
    guard
}

pub fn ui_println(message: impl AsRef<str>) {
    let msg = message.as_ref();
    println!("{}", msg);
    info!(target: "ui", app = %app_name(), event = "ui_output", message = %msg);
}

pub fn ui_log_summary(event: &str, summary: impl AsRef<str>) {
    info!(target: "ui", app = %app_name(), event = event, summary = %summary.as_ref());
}

fn app_name() -> &'static str {
    APP_NAME.get().map(|s| s.as_str()).unwrap_or("koc")
}
