use crate::cli_context::CliContext;
use crate::cli_car::{run as run_car, CarArgs};
use crate::cli_daily::{run as run_daily, DailyArgs};
use crate::cli_evotower::{run as run_evotower, EvoTowerArgs};
use crate::cli_gacha::{run as run_gacha, GachaArgs};
use crate::cli_group::{run as run_group, GroupArgs};
use crate::cli_info::{run as run_info, InfoArgs};
use crate::cli_monthly::{run as run_monthly, MonthlyArgs};
use crate::cli_study::{run as run_study, StudyArgs};
use crate::cli_tower::{run as run_tower, TowerArgs};
use crate::cli_skinc::{run as run_skinc, SkinCArgs};
use crate::cli_verify::{run as run_verify, VerifyArgs};

#[derive(clap::Subcommand, Debug, Clone)]
pub enum CliCommand {
    /// Run weekly study task for one role
    Study(StudyArgs),
    /// Verify bin file and list roles
    Verify(VerifyArgs),
    /// Query or top up monthly fish/arena progress
    Monthly(MonthlyArgs),
    /// Climb tower until energy exhausted or max reached
    Tower(TowerArgs),
    /// Run skin challenge
    Skinc(SkinCArgs),
    /// Climb evo tower until energy exhausted or max reached
    Evotower(EvoTowerArgs),
    /// Smart send car or claim all car rewards
    Car(CarArgs),
    /// Run daily tasks: single role (--bin/--server-id), group (--group), or all (--force-all)
    Daily(DailyArgs),
    /// Query role info or evotower info
    Info(InfoArgs),
    /// Free gacha draw
    Gacha(GachaArgs),
    /// List role groups
    Group(GroupArgs),
}

pub async fn run_command(command: &CliCommand, ctx: &CliContext) -> Result<(), String> {
    match command {
        CliCommand::Study(args) => run_study(ctx, args).await,
        CliCommand::Verify(args) => run_verify(ctx, args).await,
        CliCommand::Monthly(args) => run_monthly(ctx, args).await,
        CliCommand::Tower(args) => run_tower(ctx, args).await,
        CliCommand::Skinc(args) => run_skinc(ctx, args).await,
        CliCommand::Evotower(args) => run_evotower(ctx, args).await,
        CliCommand::Car(args) => run_car(ctx, args).await,
        CliCommand::Daily(args) => run_daily(ctx, args).await,
        CliCommand::Info(args) => run_info(ctx, args).await,
        CliCommand::Gacha(args) => run_gacha(ctx, args).await,
        CliCommand::Group(args) => run_group(ctx, args).await,
    }
}
