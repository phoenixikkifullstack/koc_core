use std::path::Path;

use crate::cli_context::CliContext;

#[derive(clap::Args, Debug, Clone)]
pub struct GroupArgs {}

pub async fn run(ctx: &CliContext, _args: &GroupArgs) -> Result<(), String> {
    let config = ctx.load_config()?;

    if config.groups.is_empty() {
        println!("No groups defined. Showing all roles:");
        let roles = config.all_roles();
        for (bin, sid) in &roles {
            let name = Path::new(bin).file_name().and_then(|s| s.to_str()).unwrap_or(bin);
            println!("  {} → {}", name, sid);
        }
        println!("Total: {} roles across {} bins", roles.len(), config.bins.len());
    } else {
        for group in &config.groups {
            let roles = config.group_roles(group).unwrap_or_default();
            println!("[{}] ({} roles)", group, roles.len());
            for (bin, sid) in &roles {
                let name = Path::new(bin).file_name().and_then(|s| s.to_str()).unwrap_or(bin);
                println!("  {} → {}", name, sid);
            }
        }
    }
    Ok(())
}
