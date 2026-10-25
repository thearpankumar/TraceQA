//! `traceqa doctor`: checks the setup. The checks themselves live in `install.rs`, next to the
//! code that sets the drivers up.

use std::process::ExitCode;

use clap::Args;

use crate::args::DriverArgs;
use crate::install;

#[derive(Args)]
pub struct DoctorArgs {
    /// Also show the drivers' own log lines (on standard error).
    #[arg(long)]
    verbose: bool,
    #[command(flatten)]
    drivers: DriverArgs,
}

pub fn run(args: &DoctorArgs) -> ExitCode {
    install::doctor(&args.drivers.drivers, args.verbose)
}
