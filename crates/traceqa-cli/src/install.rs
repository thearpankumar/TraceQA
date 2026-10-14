//! `traceqa install` and `traceqa doctor`: setting up the drivers and browsers, and checking
//! that setup.
//!
//! The drivers are Python programs. They are installed with uv into a Python environment of
//! TraceQA's own, under `~/.traceqa` (see `traceqa_engine::home`), so they never touch the
//! user's other Python projects. The engine finds them there by itself.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::Args;
use traceqa_engine::driver::{Driver, DriverCommand, ProcessTransport};
use traceqa_engine::{driver_commands, home};

/// The Python packages that make up the drivers, and where they sit in the repository.
const PACKAGES: [(&str, &str); 3] = [
    ("traceqa-protocol", "python/traceqa-protocol"),
    ("traceqa-driver-playwright", "drivers/playwright-py"),
    ("traceqa-driver-webdriver", "drivers/webdriver-py"),
];

/// Browsers Playwright downloads. Safari comes with macOS and needs nothing.
const PLAYWRIGHT_BROWSERS: [&str; 5] = ["chromium", "chrome", "msedge", "firefox", "webkit"];

const PYTHON: &str = "3.12";

#[derive(Args)]
pub struct InstallArgs {
    /// Browsers to download. Known values: chromium, chrome, msedge, firefox, webkit, safari.
    #[arg(default_values_t = ["chromium".to_string()])]
    browsers: Vec<String>,
    /// Install the drivers from a TraceQA checkout on this machine instead of from GitHub.
    #[arg(long, value_name = "FOLDER")]
    from: Option<PathBuf>,
    /// Branch, tag or commit to install the drivers from, when installing from GitHub.
    #[arg(long = "ref", value_name = "REF", default_value = "master")]
    git_ref: String,
    /// Also install the system libraries the browsers need. Linux only; may ask for sudo.
    #[arg(long)]
    with_deps: bool,
}

/// How to get uv, the one thing `install` cannot do without.
fn uv_instructions() -> &'static str {
    if cfg!(windows) {
        "powershell -ExecutionPolicy ByPass -c \"irm https://astral.sh/uv/install.ps1 | iex\""
    } else {
        "curl -LsSf https://astral.sh/uv/install.sh | sh"
    }
}

fn find_uv() -> Option<PathBuf> {
    std::env::var_os("TRACEQA_UV")
        .map(PathBuf::from)
        .or_else(|| home::on_path("uv"))
}

/// What to give `uv pip install`: folders of a checkout, or GitHub URLs.
fn package_specs(from: Option<&Path>, git_ref: &str) -> Vec<String> {
    let repository = env!("CARGO_PKG_REPOSITORY");
    PACKAGES
        .iter()
        .map(|(name, folder)| match from {
            Some(root) => root.join(folder).display().to_string(),
            None => format!("{name} @ git+{repository}@{git_ref}#subdirectory={folder}"),
        })
        .collect()
}

/// Runs one step, showing it first. Its output goes straight to the user.
fn step(what: &str, program: &Path, args: &[String]) -> Result<(), String> {
    eprintln!("\n{what}");
    eprintln!("  > {} {}", program.display(), args.join(" "));
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| format!("could not run {}: {e}", program.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed ({status})"))
    }
}

pub fn install(args: InstallArgs) -> ExitCode {
    match try_install(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("\ntraceqa install: {why}");
            ExitCode::FAILURE
        }
    }
}

fn try_install(args: InstallArgs) -> Result<(), String> {
    let unknown: Vec<&String> = args
        .browsers
        .iter()
        .filter(|b| *b != "safari" && !PLAYWRIGHT_BROWSERS.contains(&b.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown browser {unknown:?}. Known: {}, safari",
            PLAYWRIGHT_BROWSERS.join(", ")
        ));
    }
    if let Some(from) = &args.from {
        if !from.join(PACKAGES[1].1).join("pyproject.toml").is_file() {
            return Err(format!("{} is not a TraceQA checkout", from.display()));
        }
    }
    let home = home::dir().ok_or_else(|| format!("no home folder found. Set {}.", home::HOME_ENV))?;
    let uv = find_uv().ok_or_else(|| {
        format!(
            "uv was not found. It installs the drivers. Install it with:\n\n    {}\n\nthen run traceqa install again.",
            uv_instructions()
        )
    })?;
    let env = home::env_dir(&home);
    let python = home::bin_dir(&env).join(home::program("python"));
    eprintln!("Installing TraceQA's drivers into {}", env.display());

    if !python.is_file() {
        let args = [
            "venv".into(),
            env.display().to_string(),
            "--python".into(),
            PYTHON.into(),
        ];
        step("Creating a Python environment", &uv, &args)?;
    }

    let mut pip = vec![
        "pip".into(),
        "install".into(),
        "--python".into(),
        python.display().to_string(),
    ];
    if let Some(root) = &args.from {
        // A checkout has a lock file: install the versions it was tested with, not the newest.
        let pins = home.join("constraints.txt");
        let mut export: Vec<String> = [
            "export",
            "--project",
            &root.display().to_string(),
            "--frozen",
            "--no-hashes",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        export.extend(["--no-emit-workspace".into(), "--no-dev".into(), "--quiet".into()]);
        for (name, _) in &PACKAGES[1..] {
            export.extend(["--package".into(), name.to_string()]);
        }
        export.extend(["--output-file".into(), pins.display().to_string()]);
        step("Reading the tested versions from the checkout's uv.lock", &uv, &export)?;
        pip.extend(["--constraint".into(), pins.display().to_string()]);
    }
    // Installing again brings the drivers up to date, even when their version has not changed.
    for (name, _) in PACKAGES {
        pip.extend(["--reinstall-package".into(), name.into()]);
    }
    pip.extend(package_specs(args.from.as_deref(), &args.git_ref));
    step("Installing the drivers", &uv, &pip)?;

    let downloads: Vec<String> = args.browsers.iter().filter(|b| *b != "safari").cloned().collect();
    if !downloads.is_empty() {
        let mut playwright = vec!["-m".into(), "playwright".into(), "install".into()];
        if args.with_deps {
            playwright.push("--with-deps".into());
        }
        playwright.extend(downloads);
        step("Downloading browsers", &python, &playwright)?;
    }
    if args.browsers.iter().any(|b| b == "safari") {
        eprintln!("\nSafari needs no download. Switch on \"Allow remote automation\" in Safari's Developer settings.");
    }
    eprintln!("\nDone. Check the setup with: traceqa doctor");
    Ok(())
}

/// Where a driver command came from, for people reading `doctor`.
fn origin(command: &DriverCommand) -> String {
    let program = Path::new(&command.program);
    if program.is_absolute() {
        let installed = home::dir().is_some_and(|home| program.starts_with(home::env_dir(&home)));
        let label = if installed { "installed" } else { "configured" };
        format!("{label}: {}", program.display())
    } else {
        match home::on_path(&command.program) {
            Some(path) => format!("on PATH: {}", path.display()),
            None => "not found".into(),
        }
    }
}

pub fn doctor(explicit: &[String]) -> ExitCode {
    println!("TraceQA {}", traceqa_engine::VERSION);
    match home::dir() {
        Some(dir) => {
            let installed = home::bin_dir(&home::env_dir(&dir)).is_dir();
            let note = if installed {
                "drivers installed here"
            } else {
                "nothing installed yet: run traceqa install"
            };
            println!("Home:    {} ({note})", dir.display());
        }
        None => println!("Home:    none found; set {}", home::HOME_ENV),
    }
    match find_uv() {
        Some(uv) => println!("uv:      {}", uv.display()),
        None => println!("uv:      not found; traceqa install needs it ({})", uv_instructions()),
    }

    println!("\nDrivers:");
    let mut browsers = Vec::new();
    for command in driver_commands(explicit) {
        println!("  {command}");
        println!("    {}", origin(&command));
        let connected = ProcessTransport::spawn(&command).and_then(|transport| Driver::connect(Box::new(transport)));
        match connected {
            Ok(driver) => {
                let info = &driver.info;
                println!(
                    "    works: {} {}, opens {}",
                    info.driver_name,
                    info.driver_version,
                    info.browsers.join(", ")
                );
                browsers.extend(info.browsers.iter().cloned());
            }
            Err(error) => println!("    does not start: {error}"),
        }
    }

    if browsers.is_empty() {
        println!("\nNo driver works. Run: traceqa install");
        ExitCode::FAILURE
    } else {
        browsers.sort();
        browsers.dedup();
        println!("\nReady. Browsers available: {}", browsers.join(", "));
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_come_from_a_checkout_or_from_github() {
        let local = package_specs(Some(Path::new("/src/traceqa")), "master");
        assert_eq!(
            local[1],
            Path::new("/src/traceqa")
                .join("drivers/playwright-py")
                .display()
                .to_string()
        );
        let remote = package_specs(None, "v0.2.0");
        assert_eq!(
            remote[0],
            "traceqa-protocol @ git+https://github.com/thearpankumar/TraceQA@v0.2.0#subdirectory=python/traceqa-protocol"
        );
        // The protocol package comes first, so the drivers' dependency on it is met by the same source.
        assert_eq!(remote.len(), 3);
    }

    #[test]
    fn an_unknown_browser_is_refused_before_anything_is_installed() {
        let args = InstallArgs {
            browsers: vec!["netscape".into()],
            from: None,
            git_ref: "master".into(),
            with_deps: false,
        };
        assert!(try_install(args).unwrap_err().contains("unknown browser"));
    }
}
