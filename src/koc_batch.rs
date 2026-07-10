use clap::Parser;
use std::path::PathBuf;
use tracing::{error, info};
use koc_core::logging::init_logging;
use koc_core::scheduler::Scheduler;

/// KOC daily tasks batch scheduler
#[derive(Parser, Debug)]
#[command(name = "koc_batch", about = "KOC daily task batch scheduler")]
struct Args {
    /// configuration path
    #[arg(short, long, default_value = "config.yaml")]
    config: PathBuf,

    /// state file path
    #[arg(short, long, default_value = "state.json")]
    state: PathBuf,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let _log_guard = init_logging("koc_batch");

    info!(target: "app", config = %args.config.display(), state = %args.state.display(), "koc_batch started");

    // initialize scheduler
    let mut scheduler = match Scheduler::init(args.config, args.state).await {
        Ok(s) => s,
        Err(e) => {
            error!(target: "app", error = %e, "failed to initialize scheduler");
            std::process::exit(1);
        }
    };

    // graceful exit: capture Ctrl+C
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
    };

    tokio::select! {
        result = scheduler.run() => {
            if let Err(e) = result {
                error!(target: "scheduler", error = %e, "scheduler exited with error");
            }
        }
        _ = ctrl_c => {
            info!(target: "app", "received Ctrl+C, shutting down");
        }
    }

    info!(target: "app", "scheduler exited");
}
