//! `traceqa serve`: the client protocol on stdin and stdout. This is what clients in other languages start.

use std::io::{self, BufReader};
use std::process::ExitCode;

use clap::Args;
use traceqa_engine::{driver_commands, serve};

use crate::args::DriverArgs;
use crate::interrupt;

#[derive(Args)]
pub struct ServeArgs {
    /// How many runs may be going at once. A run.start beyond this is refused with a "busy"
    /// error (retryable) instead of starting more browsers.
    #[arg(long, value_name = "N", default_value_t = serve::DEFAULT_MAX_RUNS)]
    max_runs: usize,
    #[command(flatten)]
    drivers: DriverArgs,
}

pub fn run(args: &ServeArgs) -> ExitCode {
    install_server_interrupt("traceqa: stopping: cancelling the runs. Press Ctrl-C again to quit at once.");
    let stdin = BufReader::new(io::stdin());
    let options = serve::ServeOptions {
        max_runs: args.max_runs,
        interrupt: Some(server_interrupt()),
    };
    match serve::serve_with(
        stdin,
        Box::new(io::stdout()),
        driver_commands(&args.drivers.drivers),
        &options,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("traceqa: {error}");
            ExitCode::FAILURE
        }
    }
}

/// What `serve` and `mcp` share with the signal handler: the cancel flag and the abort request.
pub(crate) fn server_interrupt() -> traceqa_engine::Interrupt {
    traceqa_engine::Interrupt {
        cancel: &interrupt::CANCEL,
        abort: &interrupt::ABORT,
    }
}

/// Ctrl-C, SIGTERM and SIGHUP for a server: the first cancels every run and the process leaves
/// when none is in flight; a second one, or the bound, quits at once. Stdout carries the
/// protocol, so everything said here goes to stderr.
pub(crate) fn install_server_interrupt(first_message: &'static str) {
    interrupt::install(interrupt::Hooks {
        first_message,
        waits_for_run_folder: true,
        exits_when_idle: true,
        on_exit: || {},
        first_signal_bound: Some(interrupt::FIRST_SIGNAL_BOUND),
    });
}
