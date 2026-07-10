use std::path::PathBuf;

use crate::cli_context::{print_roles, CliContext};

#[derive(clap::Args, Debug, Clone)]
pub struct VerifyArgs {
    #[arg(long, value_name = "BIN")]
    pub bin: PathBuf,

    #[arg(long, value_name = "SERVER_ID")]
    pub server_id: Option<u64>,
}

pub async fn run(ctx: &CliContext, args: &VerifyArgs) -> Result<(), String> {
    let bin_data = ctx.read_bin(&args.bin)?;
    println!("Read {} bytes from {}", bin_data.len(), args.bin.display());

    match ctx.core.parse_bin(&bin_data) {
        Ok(data) => {
            let mut keys: Vec<_> = data.keys().cloned().collect();
            keys.sort();
            println!("Parsed OK, fields: {:?}", keys);
        }
        Err(e) => println!("Parse failed: {}", e),
    }

    let roles = ctx
        .core
        .get_server_list(&bin_data)
        .await
        .map_err(|e| format!("Server list failed: {}", e))?;
    print_roles(&roles);

    if let Some(target_server_id) = args.server_id {
        match roles.iter().find(|r| r.server_id == target_server_id) {
            Some(role) => {
                println!(
                    "\nTarget server_id {} found: name={} power={} level={}",
                    target_server_id, role.name, role.power, role.level
                );
            }
            None => {
                println!("\nTarget server_id {} not found in this bin", target_server_id);
            }
        }
    }

    Ok(())
}
