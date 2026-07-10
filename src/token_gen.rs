use clap::{Parser, Subcommand};
use std::path::PathBuf;
use koc_core::config::BatchConfig;
use koc_core::logging::{init_logging, ui_println};
use koc_core::wx_login::WxLoginClient;
use tracing::{error, info};

/// KOC Token / Bin file generate tool
#[derive(Parser, Debug)]
#[command(name = "token_gen", about = "KOC token/bin file generator")]
struct Args {
    /// path for configuration
    #[arg(short, long, default_value = "config.yaml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Generat bin from WeChat QRCode Scan
    Scan {
        /// output dir
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// yaml format data, can add to yaml file
        #[arg(long)]
        add_to_config: bool,
    },
    // For extend
    // Import { ... },
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let _log_guard = init_logging("token_gen");

    info!(target: "app", config = %args.config.display(), "token_gen started");

    // load configuration
    let config = BatchConfig::load(&args.config).ok();
    let default_output_dir = config
        .as_ref()
        .map(|c| c.bin_output_dir.clone())
        .unwrap_or_else(|| "bins/".to_string());

    match args.command {
        Commands::Scan { output, add_to_config } => {
            let output_dir = output
                .unwrap_or_else(|| PathBuf::from(&default_output_dir));

            info!(target: "token_gen", output_dir = %output_dir.display(), "resolved output directory");

            let client = WxLoginClient::new();

            match client.scan_and_save(&output_dir).await {
                Ok((bin_path, nickname)) => {
                    info!(target: "token_gen", nickname = %nickname, bin_path = %bin_path.display(), "generated bin file successfully");
                    ui_println(format!("[token_gen] Successfully generated bin for: {}", nickname));

                    if add_to_config {
                        if let Err(e) = WxLoginClient::add_to_config(&args.config, &bin_path) {
                            error!(target: "token_gen", error = %e, "failed to add generated bin into config");
                        }
                    } else {
                        ui_println("[token_gen] To add to scheduler, run with --add-to-config");
                        ui_println("[token_gen] Or manually add this YAML snippet to config.yaml:");
                        ui_println("  - bin: <replace-with-bin-name>");
                        ui_println("    roles:");
                        ui_println("      - server_id: <server-id>");
                    }
                }
                Err(e) => {
                    error!(target: "token_gen", error = %e, "scan_and_save failed");
                    std::process::exit(1);
                }
            }
        }
    }

    info!(target: "app", "token_gen finished");
}
