mod cli_car;
mod cli_command;
mod cli_context;
mod cli_daily;
mod cli_evotower;
mod cli_gacha;
mod cli_group;
mod cli_info;
mod cli_monthly;
mod cli_study;
mod cli_tower;
mod cli_skinc;
mod cli_verify;

use clap::Parser;
use cli_command::{run_command, CliCommand};
use cli_context::CliContext;
use koc_core::logging::init_logging;
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(name = "koc_cli", version, about = "Manual CLI for task verification")]
struct Args {
    /// YAML configuration file path
    #[arg(short, long, default_value = "config.yaml")]
    config: std::path::PathBuf,

    #[command(subcommand)]
    command: CliCommand,
}

#[tokio::main]
async fn main() {
    let _log_guard = init_logging("koc_cli");
    let args = Args::parse();
    let ctx = CliContext::new(args.config.clone());

    info!(target: "app", config = %args.config.display(), command = ?args.command, "koc_cli started");
    if let Err(e) = run_command(&args.command, &ctx).await {
        error!(target: "app", error = %e, "command failed");
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
