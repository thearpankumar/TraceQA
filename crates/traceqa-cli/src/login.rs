//! `traceqa login`: sign in once by hand and save the browser's state for later runs.
//!
//! A browser window opens on the page. The person signs in there the way they always do,
//! with a password, a code or another service, and comes back to press Enter. The cookies
//! and stored values are then saved to a file that `traceqa extract --storage-state` starts
//! every browser with. TraceQA never sees the password.

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use traceqa_engine::driver::Drivers;
use traceqa_engine::driver_commands;
use traceqa_protocol::types::{BrowserTarget, PageGotoParams, SessionOpenParams, StorageState, Viewport, WaitUntil};

#[derive(Args)]
pub struct LoginArgs {
    /// The page to sign in on.
    url: String,
    /// Where to save the state. It holds session keys: keep it out of git.
    #[arg(long, value_name = "FILE")]
    save: PathBuf,
    /// The browser to sign in with. Its state works in the other browsers too.
    #[arg(long, default_value = "chromium", value_parser = BrowserTarget::parse)]
    browser: BrowserTarget,
    /// Do not show the window, and save as soon as the page has loaded. For scripts and tests,
    /// where the page signs in by itself.
    #[arg(long)]
    headless: bool,
    /// Command that starts a browser driver. Repeat for several.
    #[arg(long = "driver", value_name = "COMMAND")]
    drivers: Vec<String>,
}

pub fn login(args: LoginArgs) -> ExitCode {
    match try_login(args) {
        Ok(message) => {
            eprintln!("{message}");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("traceqa login: {why}");
            ExitCode::FAILURE
        }
    }
}

fn try_login(args: LoginArgs) -> Result<String, String> {
    if !(args.url.starts_with("http://") || args.url.starts_with("https://")) {
        return Err(format!("expected an http or https address, got {:?}", args.url));
    }
    let mut drivers = Drivers::new(driver_commands(&args.drivers));
    let driver = drivers.for_browser(&args.browser.browser).map_err(|e| e.to_string())?;
    let mut target = args.browser.clone();
    target.headless = Some(args.headless);
    let session = driver
        .open_session(&SessionOpenParams {
            target,
            viewport: Viewport {
                width: 1280,
                height: 800,
            },
            locale: None,
            storage_state: None,
        })
        .map_err(|e| e.to_string())?;

    let outcome = (|| {
        driver
            .goto(&PageGotoParams {
                session_id: session.session_id.clone(),
                url: args.url.clone(),
                wait_until: Some(WaitUntil::Load),
                timeout_ms: None,
            })
            .map_err(|e| e.to_string())?;
        if !args.headless {
            eprintln!("A {} window has opened on {}.", args.browser.browser, args.url);
            eprintln!("Sign in there as you always do, then come back here and press Enter.");
            let mut line = String::new();
            io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
        }
        let state = driver.storage(&session.session_id).map_err(|e| e.to_string())?;
        save(&args.save, &state)?;
        Ok(describe(&state, &args.save))
    })();
    let _ = driver.close_session(&session.session_id);
    outcome
}

fn describe(state: &StorageState, path: &Path) -> String {
    let values: usize = state.origins.iter().map(|origin| origin.local_storage.len()).sum();
    format!(
        "Saved {} cookies and {values} stored values to {}.\n\
         It works like a password until the session expires: keep it out of git and share it only as a secret.\n\
         Use it with: traceqa extract URL --storage-state {}",
        state.cookies.len(),
        path.display(),
        path.display()
    )
}

/// Writes the state readable by its owner only, where the system has such a thing.
fn save(path: &Path, state: &StorageState) -> Result<(), String> {
    let text = serde_json::to_string_pretty(state).expect("spec types always serialise");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|e| format!("could not write {}: {e}", path.display()))
}
