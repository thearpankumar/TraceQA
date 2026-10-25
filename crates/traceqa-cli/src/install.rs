//! `traceqa install` and `traceqa doctor`: setting up the drivers and browsers, and checking
//! that setup.
//!
//! The drivers are Python programs. They are installed with uv into a Python environment of
//! TraceQA's own, under `~/.traceqa` (see `traceqa_engine::home`), so they never touch the
//! user's other Python projects. The engine finds them there by itself.

mod axe;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::Args;
use traceqa_common::types::{BrowserTarget, SessionOpenParams, Viewport};
use traceqa_engine::driver::{Driver, DriverCommand, DriverError, ErrorKind, ProcessTransport};
use traceqa_engine::{driver_commands, home};

/// The Python packages that make up the drivers, and where they sit in the repository.
const PACKAGES: [(&str, &str); 3] = [
    ("traceqa-protocol", "python/traceqa-protocol"),
    ("traceqa-driver-playwright", "web-drivers/playwright-py"),
    ("traceqa-driver-webdriver", "web-drivers/webdriver-py"),
];

/// Browsers Playwright downloads. Safari comes with macOS and needs nothing.
const PLAYWRIGHT_BROWSERS: [&str; 5] = ["chromium", "chrome", "msedge", "firefox", "webkit"];

const PYTHON: &str = "3.12";

/// The tag this binary was released under, for example `v0.1.0`.
const RELEASE_TAG: &str = concat!("v", env!("CARGO_PKG_VERSION"));

#[derive(Args)]
pub struct InstallArgs {
    /// Browsers to download. Known values: chromium, chrome, msedge, firefox, webkit, safari.
    #[arg(default_values_t = ["chromium".to_string()])]
    browsers: Vec<String>,
    /// Install the drivers from a TraceQA checkout on this machine instead of from GitHub.
    #[arg(long, value_name = "FOLDER")]
    from: Option<PathBuf>,
    /// Branch, tag or commit to install the drivers from, when installing from GitHub. The
    /// default is the commit this binary was released from, so the binary and its drivers
    /// match. Only a full commit SHA is pinned: a branch or tag can move.
    #[arg(long = "ref", value_name = "REF")]
    git_ref: Option<String>,
    /// Also install the system libraries the browsers need. Linux only; may ask for sudo.
    #[arg(long)]
    with_deps: bool,
    /// Print what would be run and exit. Nothing is installed or written.
    #[arg(long)]
    dry_run: bool,
    /// Install only axe-core, for `traceqa extract --axe`: the pinned npm tarball is downloaded, its
    /// SHA-512 checked and axe.min.js and the licence extracted. axe-core is MPL-2.0 and is not
    /// part of TraceQA. Drivers and browsers are left alone.
    #[arg(long)]
    axe: bool,
}

/// The commit this binary was built from, 40 hex digits, or empty when it was not built from a
/// release commit (see build.rs). A tag can be moved; a commit cannot.
const COMMIT: &str = env!("TRACEQA_COMMIT");

/// The third-party packages the drivers need, pinned with hashes from uv.lock. Made by
/// scripts/gen_driver_requirements.py.
const DRIVER_REQUIREMENTS: &str = include_str!("../driver-requirements.txt");

const UV_DOCS: &str = "https://docs.astral.sh/uv/getting-started/installation/";

/// How to get uv, the one thing `install` cannot do without: a package manager, never a
/// script piped into a shell.
fn uv_install_command() -> &'static str {
    if cfg!(windows) {
        "winget install --id=astral-sh.uv -e"
    } else {
        "brew install uv    (or: pipx install uv)"
    }
}

fn uv_instructions() -> String {
    format!(
        "{}\n\n    Other ways, and a versioned installer with its published checksum: {UV_DOCS}",
        uv_install_command()
    )
}

fn is_full_commit(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The git ref to install the drivers from, and a message to show about it. What the person
/// asked for wins; otherwise the commit this binary was built from; otherwise its release tag.
fn resolve_ref(commit: &str, tag: &str, requested: Option<&str>) -> (String, Option<String>) {
    if let Some(requested) = requested {
        let note = (!is_full_commit(requested)).then(|| {
            format!("Installing from {requested}, which is not pinned to a commit. Use a full commit SHA to pin.")
        });
        return (requested.to_string(), note);
    }
    if is_full_commit(commit) {
        let note = format!("Installing the drivers from this binary's release commit {commit}, so they are pinned.");
        return (commit.to_string(), Some(note));
    }
    let note = format!("This binary was not built from a release commit; using tag {tag}, and tags can be moved.");
    (tag.to_string(), Some(note))
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

/// The two `uv` steps that install the drivers from GitHub, as (what, arguments).
///
/// First every third-party package, from the hashed requirements and nothing else. Then
/// TraceQA's own three packages from the release ref, without dependencies. They are two steps
/// because uv does not accept a git URL in hash-checking mode (it has no hash to check).
fn release_steps(python: &Path, requirements: &Path, git_ref: &str) -> Vec<(&'static str, Vec<String>)> {
    let python = python.display().to_string();
    let mut dependencies = words(&[
        "pip",
        "install",
        "--python",
        &python,
        "--require-hashes",
        "--no-deps",
        "-r",
    ]);
    dependencies.push(requirements.display().to_string());

    let mut drivers = words(&["pip", "install", "--python", &python, "--no-deps"]);
    // Installing again brings the drivers up to date, even when their version has not changed.
    for (name, _) in PACKAGES {
        drivers.extend(words(&["--reinstall-package", name]));
    }
    drivers.extend(package_specs(None, git_ref));
    vec![
        (
            "Installing the drivers' dependencies, checked against hashes",
            dependencies,
        ),
        ("Installing the drivers", drivers),
    ]
}

fn words(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_string()).collect()
}

/// Runs one step, showing it first. Its output goes straight to the user. In a dry run it is
/// only shown, on standard output.
fn step(dry_run: bool, what: &str, program: &Path, args: &[String]) -> Result<(), String> {
    if dry_run {
        println!("\n{what}");
        println!("  > {} {}", program.display(), args.join(" "));
        return Ok(());
    }
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

pub fn run(args: &InstallArgs) -> ExitCode {
    match try_install(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("\ntraceqa install: {why}");
            ExitCode::FAILURE
        }
    }
}

fn try_install(args: &InstallArgs) -> Result<(), String> {
    if args.axe {
        return axe::install(args.dry_run);
    }
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
        let complete = PACKAGES
            .iter()
            .all(|(_, folder)| from.join(folder).join("pyproject.toml").is_file());
        if !complete {
            return Err(format!("{} is not a TraceQA checkout", from.display()));
        }
    }
    let dry_run = args.dry_run;
    let home = home::dir().ok_or_else(|| format!("no home folder found. Set {}.", home::HOME_ENV))?;
    // A dry run only prints, so it does not need uv to be there.
    let uv = match find_uv() {
        Some(uv) => uv,
        None if dry_run => PathBuf::from("uv"),
        None => {
            return Err(format!(
                "uv was not found. It installs the drivers. Install it with:\n\n    {}\n\nthen run traceqa install again.",
                uv_instructions()
            ));
        }
    };
    let env = home::env_dir(&home);
    let python = home::bin_dir(&env).join(home::program("python"));
    let (git_ref, note) = resolve_ref(COMMIT, RELEASE_TAG, args.git_ref.as_deref());
    if dry_run {
        println!("Dry run: nothing is installed or written.");
    }
    eprintln!("Installing TraceQA's drivers into {}", env.display());
    if args.from.is_none() {
        if let Some(note) = note {
            eprintln!("{note}");
        }
    } else if args.git_ref.is_some() {
        eprintln!("--ref is ignored when installing from a checkout with --from.");
    }

    if !python.is_file() {
        let venv = words(&["venv", &env.display().to_string(), "--python", PYTHON]);
        step(dry_run, "Creating a Python environment", &uv, &venv)?;
    }

    let mut pip = words(&["pip", "install", "--python", &python.display().to_string()]);
    if let Some(root) = &args.from {
        // A checkout has a lock file: install the versions it was tested with, not the newest.
        let pins = home.join("constraints.txt").display().to_string();
        let mut export = words(&["export", "--project", &root.display().to_string()]);
        export.extend(words(&[
            "--frozen",
            "--no-hashes",
            "--no-emit-workspace",
            "--no-dev",
            "--quiet",
        ]));
        // The drivers name every package they need, the protocol package included.
        for (name, _) in &PACKAGES[1..] {
            export.extend(words(&["--package", name]));
        }
        export.extend(words(&["--output-file", &pins]));
        step(
            dry_run,
            "Reading the tested versions from the checkout's uv.lock",
            &uv,
            &export,
        )?;
        pip.extend(words(&["--constraint", &pins]));
        // Installing again brings the drivers up to date, even when their version has not changed.
        for (name, _) in PACKAGES {
            pip.extend(words(&["--reinstall-package", name]));
        }
        pip.extend(package_specs(args.from.as_deref(), &git_ref));
        step(dry_run, "Installing the drivers", &uv, &pip)?;
    } else {
        let requirements = home.join("driver-requirements.txt");
        if dry_run {
            println!("\nWriting the pinned dependencies to {}", requirements.display());
        } else {
            fs::create_dir_all(&home).map_err(|e| format!("could not create {}: {e}", home.display()))?;
            fs::write(&requirements, DRIVER_REQUIREMENTS)
                .map_err(|e| format!("could not write {}: {e}", requirements.display()))?;
        }
        for (what, args) in release_steps(&python, &requirements, &git_ref) {
            step(dry_run, what, &uv, &args)?;
        }
    }

    let downloads: Vec<String> = args.browsers.iter().filter(|b| *b != "safari").cloned().collect();
    if !downloads.is_empty() {
        let mut playwright = words(&["-m", "playwright", "install"]);
        if args.with_deps {
            playwright.push("--with-deps".into());
        }
        playwright.extend(downloads);
        step(dry_run, "Downloading browsers", &python, &playwright)?;
    }
    if args.browsers.iter().any(|b| b == "safari") {
        eprintln!("\nSafari needs no download. Switch on \"Allow remote automation\" in Safari's Developer settings.");
    }
    if dry_run {
        println!("\nDry run finished: nothing was installed.");
    } else {
        eprintln!("\nDone. Check the setup with: traceqa doctor");
    }
    Ok(())
}

/// What `doctor` says about the commit this binary was built from.
fn commit_text(commit: &str) -> &str {
    if is_full_commit(commit) {
        commit
    } else {
        "not a release build"
    }
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

pub fn doctor(explicit: &[String], verbose: bool) -> ExitCode {
    println!("TraceQA {}", traceqa_engine::VERSION);
    println!("Commit:  {}", commit_text(COMMIT));
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
        None => println!(
            "uv:      not found; traceqa install needs it (install it with: {}; see {UV_DOCS})",
            uv_install_command()
        ),
    }

    println!("\nDrivers:");
    let mut working = Vec::new();
    let mut missing = Vec::new();
    let mut other_problems = 0;
    let mut any_driver = false;
    for command in driver_commands(explicit) {
        println!("  {command}");
        println!("    {}", origin(&command));
        // The drivers' log lines would be mixed into the results, so they are shown only on request.
        let connected = ProcessTransport::spawn(&command)
            .map(|transport| if verbose { transport } else { transport.quiet() })
            .and_then(|transport| Driver::connect(Box::new(transport)));
        let mut driver = match connected {
            Ok(driver) => driver,
            Err(error) => {
                println!("    does not start: {}", error.message());
                continue;
            }
        };
        any_driver = true;
        println!(
            "    works: {} {}, opens {}",
            driver.info.driver_name,
            driver.info.driver_version,
            driver.info.browsers.join(", ")
        );
        // Opening a browser is the only sure way to know it is installed: a channel such as
        // msedge lives outside TraceQA's folder. Each launch takes a second or two, so every
        // browser gets its line as soon as it is known.
        let names: Vec<String> = driver.info.browsers.clone();
        for name in names.iter().filter(|name| can_probe(name)) {
            match probe_browser(&mut driver, name) {
                Ok(()) => {
                    println!("    {name}: ok");
                    working.push(name.clone());
                }
                Err(error) if *error.kind() == ErrorKind::Unsupported => {
                    println!("    {name}: not installed -> {}", error.message());
                    missing.push(name.clone());
                }
                Err(error) => {
                    println!("    {name}: does not open: {}", error.message());
                    other_problems += 1;
                }
            }
        }
    }

    if working.is_empty() {
        if !any_driver {
            println!("\nNo driver works. Run: traceqa install");
        } else {
            println!("\nNo browser opens.");
            print_missing(&missing);
            if other_problems > 0 {
                println!("See the lines above for the browsers that failed for another reason.");
            }
        }
        return ExitCode::FAILURE;
    }
    working.sort();
    working.dedup();
    println!("\nReady. Browsers that work: {}", working.join(", "));
    print_missing(&missing);
    ExitCode::SUCCESS
}

fn print_missing(missing: &[String]) {
    if !missing.is_empty() {
        println!(
            "Not installed: {}. Run: traceqa install {}",
            missing.join(", "),
            missing.join(" ")
        );
    }
}

/// Browsers that open by themselves. `cdp` and `webdriver` need an address, and Safari exists
/// only on macOS.
fn can_probe(browser: &str) -> bool {
    !matches!(browser, "cdp" | "webdriver") && (browser != "safari" || cfg!(target_os = "macos"))
}

/// Opens a headless window of `browser` and closes it again.
fn probe_browser(driver: &mut Driver, browser: &str) -> Result<(), DriverError> {
    let mut target = BrowserTarget::named(browser);
    target.headless = Some(true);
    let session = driver.open_session(&SessionOpenParams {
        target,
        viewport: Viewport {
            width: 1280,
            height: 800,
        },
        locale: None,
        storage_state: None,
    })?;
    driver.close_session(&session.session_id)
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
                .join("web-drivers/playwright-py")
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
        assert_eq!(RELEASE_TAG, format!("v{}", env!("CARGO_PKG_VERSION")));
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn package_specs_use_the_resolved_ref() {
        let (git_ref, _) = resolve_ref(SHA, "v0.1.0", None);
        for spec in package_specs(None, &git_ref) {
            assert!(spec.contains(&format!("@{SHA}#subdirectory=")), "{spec}");
        }
    }

    #[test]
    fn resolve_ref_prefers_the_embedded_commit() {
        let (git_ref, note) = resolve_ref(SHA, "v0.1.0", None);
        assert_eq!(git_ref, SHA);
        assert!(note.unwrap().contains("pinned"));
    }

    #[test]
    fn resolve_ref_uses_the_tag_with_a_warning_without_one() {
        let (git_ref, note) = resolve_ref("", "v0.1.0", None);
        assert_eq!(git_ref, "v0.1.0");
        let note = note.unwrap();
        assert!(note.contains("not built from a release commit") && note.contains("tags can be moved"));
        // Anything that is not a full commit id is no commit.
        assert_eq!(resolve_ref("abc123", "v0.1.0", None).0, "v0.1.0");
    }

    #[test]
    fn resolve_ref_warns_for_a_user_ref_that_is_not_a_sha() {
        for requested in ["master", "v0.2.0", "abc1234", &SHA[..39]] {
            let (git_ref, note) = resolve_ref(SHA, "v0.1.0", Some(requested));
            assert_eq!(
                git_ref, requested,
                "what the person asked for wins over the embedded commit"
            );
            let note = note.unwrap();
            assert!(note.contains("not pinned to a commit") && note.contains("Use a full commit SHA"));
        }
    }

    #[test]
    fn resolve_ref_accepts_a_40_hex_ref_silently() {
        let other = "ABCDEFabcdef0123456789abcdef0123456789ab";
        assert_eq!(resolve_ref("", "v0.1.0", Some(other)), (other.to_string(), None));
    }

    #[test]
    fn the_install_plan_installs_hashed_dependencies_before_the_drivers() {
        let steps = release_steps(
            Path::new("/h/env/bin/python"),
            Path::new("/h/driver-requirements.txt"),
            SHA,
        );
        assert_eq!(steps.len(), 2);
        let (_, dependencies) = &steps[0];
        assert!(dependencies.contains(&"--require-hashes".to_string()));
        assert!(dependencies.contains(&"--no-deps".to_string()));
        assert!(dependencies
            .windows(2)
            .any(|pair| pair[0] == "-r" && pair[1].ends_with("driver-requirements.txt")));
        assert!(
            !dependencies.iter().any(|arg| arg.contains("git+")),
            "git URLs cannot carry hashes"
        );

        let (_, drivers) = &steps[1];
        assert!(drivers.contains(&"--no-deps".to_string()));
        assert!(!drivers.contains(&"--require-hashes".to_string()));
        let specs: Vec<&String> = drivers.iter().filter(|arg| arg.contains("git+")).collect();
        assert_eq!(specs.len(), 3);
        assert!(specs.iter().all(|spec| spec.contains(&format!("@{SHA}#"))));
    }

    #[test]
    fn the_pinned_requirements_are_hashed_and_leave_out_our_own_packages() {
        assert!(DRIVER_REQUIREMENTS.contains("--hash=sha256:"));
        assert!(DRIVER_REQUIREMENTS.lines().any(|line| line.starts_with("playwright==")));
        assert!(!DRIVER_REQUIREMENTS.lines().any(|line| line.starts_with("traceqa")));
        // Every pinned package carries a hash, as --require-hashes demands.
        let pins = DRIVER_REQUIREMENTS.lines().filter(|line| line.contains("==")).count();
        let hashed = DRIVER_REQUIREMENTS
            .lines()
            .filter(|line| line.contains("--hash=sha256:"))
            .count();
        assert!(pins > 0 && hashed >= pins);
    }

    #[test]
    fn uv_is_never_installed_by_piping_a_download_into_a_shell() {
        let text = uv_instructions();
        assert!(
            !text.contains("curl") && !text.contains("| sh") && !text.contains("iex"),
            "{text}"
        );
        assert!(text.contains(UV_DOCS));
    }

    #[test]
    fn doctor_says_when_the_binary_is_not_a_release_build() {
        assert_eq!(commit_text(""), "not a release build");
        assert_eq!(commit_text(SHA), SHA);
    }

    #[test]
    fn an_unknown_browser_is_refused_before_anything_is_installed() {
        let args = InstallArgs {
            browsers: vec!["netscape".into()],
            from: None,
            git_ref: None,
            with_deps: false,
            dry_run: false,
            axe: false,
        };
        assert!(try_install(&args).unwrap_err().contains("unknown browser"));
    }
}
