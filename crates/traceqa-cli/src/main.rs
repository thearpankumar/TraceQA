//! The `traceqa` command. Thin on purpose: it turns arguments into the same
//! request every other client sends, and hands it to the engine.
//!
//! To add a subcommand: one `mod` line, one `Command` variant and one `match` arm below, each
//! list kept alphabetical so parallel additions merge cleanly. The command's code and its
//! `Args` struct live in its own file, whose `run` returns the exit code. The one exception is
//! the variant order, which is the order `traceqa --help` lists commands in.

mod args;
mod codegen;
mod diff;
mod doctor;
mod extract;
mod findings;
mod install;
mod interrupt;
mod login;
mod mcp;
mod serve;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "traceqa",
    version,
    about = "Find every interactive component on a page, in any browser."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write a Playwright page object (TypeScript or Python) for the components of a run.
    ///
    /// Then: traceqa codegen runs/RUN --lang ts
    Codegen(codegen::CodegenArgs),
    /// Compare two runs (browsers, viewports, or builds) by component.
    ///
    /// Prints Markdown, or the DiffFile JSON with --format json. Exit 0 when the diff ran.
    Diff(diff::DiffArgs),
    /// List every interactive component on a page.
    Extract(extract::ExtractArgs),
    /// Run automated accessibility and quality checks over a finished run.
    ///
    /// Prints a table and writes findings.json next to each components.json. These are
    /// automated checks, not a conformance claim.
    Findings(findings::FindingsArgs),
    /// Speak the client protocol on stdin and stdout. This is what SDKs start.
    Serve(serve::ServeArgs),
    /// Let AI tools such as Claude Code run TraceQA (an MCP server).
    ///
    /// Speaks the Model Context Protocol on stdin and stdout. Add it to Claude Code with:
    /// claude mcp add traceqa -- traceqa mcp
    Mcp(mcp::McpArgs),
    /// Install the browser drivers and browsers into TraceQA's own folder (~/.traceqa).
    Install(install::InstallArgs),
    /// Check the setup: where the drivers are, whether they start, and which browsers they open.
    Doctor(doctor::DoctorArgs),
    /// Sign in once in a browser window and save its state, for pages behind a sign-in.
    ///
    /// Then run: traceqa extract URL --storage-state FILE
    Login(login::LoginArgs),
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Codegen(args) => codegen::run(&args),
        Command::Diff(args) => diff::run(&args),
        Command::Doctor(args) => doctor::run(&args),
        Command::Extract(args) => extract::run(&args),
        Command::Findings(args) => findings::run(&args),
        Command::Install(args) => install::run(&args),
        Command::Login(args) => login::run(&args),
        Command::Mcp(args) => mcp::run(&args),
        Command::Serve(args) => serve::run(&args),
    }
}
