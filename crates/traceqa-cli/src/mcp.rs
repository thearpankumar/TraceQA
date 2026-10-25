//! `traceqa mcp`: the Model Context Protocol server.

use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use traceqa_engine::driver_commands;

use crate::args::DriverArgs;
use crate::serve::{install_server_interrupt, server_interrupt};

#[derive(Args)]
pub struct McpArgs {
    /// Where runs are written and read.
    #[arg(long, default_value = "runs")]
    out: String,
    /// A folder of saved sign-in states (see `traceqa login --save`). With it, the AI can name
    /// a file in this folder as `storage_state`; without it, the AI cannot use sign-ins at all.
    #[arg(long, value_name = "DIR")]
    state_dir: Option<PathBuf>,
    /// Let the AI attach to a running browser (cdp=ENDPOINT) or a WebDriver server
    /// (webdriver=URL). Off by default: those name a machine for this server to connect to.
    #[arg(long)]
    allow_remote_browsers: bool,
    /// Refuse pages on loopback, private (RFC 1918), shared (100.64/10), unique-local and
    /// multicast addresses, also when a redirect leads there. Cloud metadata and link-local
    /// addresses are always refused, with or without this flag.
    #[arg(long)]
    block_private: bool,
    /// With --block-private, still allow this host (a name or an address). Repeat for several.
    /// Never lifts the ban on metadata and link-local addresses.
    #[arg(long = "allow-host", value_name = "HOST")]
    allow_hosts: Vec<String>,
    /// How many extractions may run at once. More are answered "busy" at once, so a host that
    /// fires many calls cannot start many browsers. Reading tools have a higher limit of their own.
    #[arg(long, value_name = "N", default_value_t = traceqa_mcp::DEFAULT_MAX_CONCURRENT_EXTRACTS)]
    max_concurrent_extracts: usize,
    #[command(flatten)]
    drivers: DriverArgs,
}

pub fn run(args: &McpArgs) -> ExitCode {
    let runs_dir = match std::path::absolute(Path::new(&args.out)) {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("traceqa: {}: {error}", args.out);
            return ExitCode::from(2);
        }
    };
    let state_dir = match args.state_dir.as_deref().map(std::fs::canonicalize).transpose() {
        Ok(dir) if dir.as_ref().is_none_or(|dir| dir.is_dir()) => dir,
        _ => {
            eprintln!(
                "traceqa: --state-dir {} is not a folder",
                args.state_dir.as_deref().unwrap_or(Path::new("")).display()
            );
            return ExitCode::from(2);
        }
    };
    let config = traceqa_mcp::Config {
        runs_dir,
        drivers: driver_commands(&args.drivers.drivers),
        state_dir,
        allow_remote_browsers: args.allow_remote_browsers,
        policy: traceqa_mcp::UrlPolicy::new(args.block_private, &args.allow_hosts),
        max_concurrent_extracts: args.max_concurrent_extracts,
        interrupt: Some(server_interrupt()),
    };
    install_server_interrupt("traceqa: stopping: cancelling the calls. Press Ctrl-C again to quit at once.");
    // Stdout belongs to the protocol. Everything for people goes to stderr, which hosts log.
    match traceqa_mcp::serve(BufReader::new(io::stdin()), Box::new(io::stdout()), config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("traceqa: {error}");
            ExitCode::FAILURE
        }
    }
}
