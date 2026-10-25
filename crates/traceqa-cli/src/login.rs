//! `traceqa login`: sign in once by hand and save the browser's state for later runs.
//!
//! A browser window opens on the page. The person signs in there the way they always do,
//! with a password, a code or another service, and comes back to press Enter. The cookies
//! and stored values are then saved to a file that `traceqa extract --storage-state` starts
//! every browser with. TraceQA never sees the password.
//!
//! The file works like a password, so it is made with care. `--save` is checked before any
//! browser opens, so nobody signs in just to lose the result. The state is written to a
//! temporary file next to the destination that only its owner can open (mode 600 on Unix, an
//! access list with the owner alone on Windows), set before anything is written to it, and
//! then moved over the destination. The file is never readable by others, not even for a
//! moment, a file that already existed ends up owner-only as well, and a failure leaves no
//! temporary file behind.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use crate::interrupt;
use clap::Args;
use traceqa_engine::driver::DriverError;
use traceqa_engine::driver::Drivers;
use traceqa_engine::driver_commands;
use traceqa_protocol::sync::LockExt;
use traceqa_protocol::types::{BrowserTarget, PageGotoParams, SessionOpenParams, StorageState, Viewport, WaitUntil};

#[derive(Args)]
pub struct LoginArgs {
    /// The page to sign in on.
    url: String,
    /// Where to save the state, readable by you only. It holds session keys: keep it out of git.
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

/// What went wrong. A usage mistake exits with 2, as `traceqa extract` does, and a failure of
/// the browser, the driver or the disk with 1.
#[derive(Debug)]
enum LoginError {
    Usage(String),
    Failed(String),
    /// Ctrl-C, SIGTERM or SIGHUP arrived. Exits with 130.
    Interrupted,
}

impl From<String> for LoginError {
    fn from(message: String) -> Self {
        LoginError::Failed(message)
    }
}

pub fn login(args: &LoginArgs) -> ExitCode {
    match try_login(args) {
        Ok(message) => {
            eprintln!("{message}");
            ExitCode::SUCCESS
        }
        Err(LoginError::Usage(why)) => {
            eprintln!("traceqa login: {why}");
            ExitCode::from(2)
        }
        Err(LoginError::Interrupted) => interrupted(),
        Err(LoginError::Failed(why)) => {
            if interrupt_may_be_arriving() {
                return interrupted();
            }
            eprintln!("traceqa login: {why}");
            ExitCode::FAILURE
        }
    }
}

/// The end of a login that was stopped: nothing was saved, the temporary file is gone (the
/// `Pending` that held it was dropped, and this removes any that is left).
fn interrupted() -> ExitCode {
    remove_pending();
    interrupt::say("traceqa login: interrupted; nothing was saved");
    ExitCode::from(interrupt::INTERRUPTED)
}

/// How long closing the session may take after the first signal. The whole orderly stop is
/// bounded by [`STOP_BOUND`].
const CLOSE_BOUND: Duration = Duration::from_secs(5);

/// How long after the first signal the command may take to stop before it is ended anyway.
const STOP_BOUND: Duration = Duration::from_secs(10);

/// An error from the driver: an interrupt when one was asked for, otherwise a failure.
fn failed(error: &DriverError) -> LoginError {
    if interrupt::CANCEL.load(Ordering::SeqCst) {
        LoginError::Interrupted
    } else {
        LoginError::Failed(error.to_string())
    }
}

/// Waits for the person to press Enter, or for an interrupt. The read happens on a thread of its
/// own because a blocked read cannot be cancelled; the thread may stay blocked when the command ends.
fn wait_for_enter() -> Result<(), LoginError> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = sender.send(io::stdin().lock().read_line(&mut line).map(drop));
    });
    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(read) => return read.map_err(|e| LoginError::Failed(e.to_string())),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if interrupt::CANCEL.load(Ordering::SeqCst) {
                    return Err(LoginError::Interrupted);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn try_login(args: &LoginArgs) -> Result<String, LoginError> {
    check_address(&args.url)?;
    // Before a browser opens: a file that cannot be written is found now, not after signing in.
    let pending = Pending::prepare(&args.save)?;
    interrupt::install(interrupt::Hooks {
        first_message: "
traceqa login: interrupted; closing the browser. Press Ctrl-C again to quit at once.",
        waits_for_run_folder: false,
        exits_when_idle: false,
        on_exit: || {
            remove_pending();
            interrupt::say("traceqa login: interrupted; nothing was saved");
        },
        first_signal_bound: Some(STOP_BOUND),
    });

    let mut drivers = Drivers::new(driver_commands(&args.drivers));
    let driver = drivers.for_browser(&args.browser.browser).map_err(|e| e.to_string())?;
    let mut target = args.browser.clone();
    target.headless = Some(args.headless);
    let session = driver
        .until(&interrupt::CANCEL)
        .open_session(&SessionOpenParams {
            target,
            viewport: Viewport {
                width: 1280,
                height: 800,
            },
            locale: None,
            storage_state: None,
        })
        .map_err(|e| failed(&e))?;

    let outcome = (|| -> Result<StorageState, LoginError> {
        driver
            .until(&interrupt::CANCEL)
            .goto(&PageGotoParams {
                session_id: session.session_id.clone(),
                url: args.url.clone(),
                wait_until: Some(WaitUntil::Load),
                timeout_ms: None,
            })
            .map_err(|e| failed(&e))?;
        if !args.headless {
            eprintln!("A {} window has opened on {}.", args.browser.browser, args.url);
            eprintln!("Sign in there as you always do, then come back here and press Enter.");
            wait_for_enter()?;
        }
        driver.storage(&session.session_id).map_err(|e| failed(&e))
    })();
    // After an interrupt the driver may be busy with the call that was given up on: wait a short,
    // fixed time for it, then let `drivers` drop, which ends the process.
    let _ = match outcome {
        Err(LoginError::Interrupted) => driver.close_session_within(&session.session_id, CLOSE_BOUND),
        _ => driver.close_session(&session.session_id),
    };

    let state = outcome?;
    pending.commit(&state)?;
    Ok(describe(&state, &args.save))
}

/// The address must be http or https and must not carry a user name or password: the sign-in
/// happens in the browser, and an address ends up in shell history, process lists and logs.
fn check_address(url: &str) -> Result<(), LoginError> {
    // Browsers drop tabs and line breaks from an address before reading it.
    let cleaned: String = url.chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    if !(cleaned.starts_with("http://") || cleaned.starts_with("https://")) {
        return Err(LoginError::Usage("expected an http or https address".into()));
    }
    let after_scheme = cleaned.split_once("://").map_or("", |(_, rest)| rest);
    let authority = after_scheme.split(['/', '\\', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(LoginError::Usage(
            "the address has a user name or password in it. Do not put them in the address: sign in in the \
             browser window, or start from a state file with `traceqa extract --storage-state`"
                .into(),
        ));
    }
    Ok(())
}

fn describe(state: &StorageState, path: &Path) -> String {
    let values: usize = state.origins.iter().map(|origin| origin.local_storage.len()).sum();
    format!(
        "Saved {} cookies and {values} stored values to {}.\n\
         Saved with owner-only permissions.\n\
         It works like a password until the session expires: keep it out of git and share it only as a secret.\n\
         Use it with: traceqa extract URL --storage-state {}",
        state.cookies.len(),
        path.display(),
        path.display()
    )
}

/// A temporary file, already owner-only and empty, waiting to be filled and moved into place.
/// Dropped without `commit`, it removes itself.
struct Pending {
    temp: PathBuf,
    destination: PathBuf,
    file: Option<File>,
}

impl Pending {
    /// Checks that `destination` can be written and makes the temporary file next to it.
    fn prepare(destination: &Path) -> Result<Pending, LoginError> {
        let shown = destination.display();
        if destination.is_dir() {
            return Err(LoginError::Usage(format!(
                "--save {shown} is a directory: give a file name, such as {}",
                destination.join("state.json").display()
            )));
        }
        let Some(name) = destination.file_name() else {
            return Err(LoginError::Usage(format!("--save {shown} is not a file name")));
        };
        let folder = match destination.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        if !folder.is_dir() {
            return Err(LoginError::Usage(format!(
                "--save {shown}: the folder {} does not exist",
                folder.display()
            )));
        }
        let temp = folder.join(format!(".{}.tmp-{}", name.to_string_lossy(), std::process::id()));
        let file = create_private(&temp)?;
        PENDING.lock_ok().push(temp.clone());
        Ok(Pending {
            temp,
            destination: destination.to_path_buf(),
            file: Some(file),
        })
    }

    /// Writes the state and moves the file over the destination, replacing whatever was there:
    /// an older file with looser permissions, or a link, which is replaced rather than written through.
    fn commit(mut self, state: &StorageState) -> Result<(), String> {
        let text = serde_json::to_string_pretty(state).expect("spec types always serialise");
        let mut file = self.file.take().expect("a pending file is committed once");
        let shown = self.destination.display();
        // Held from the first write to the move, so an interrupt cannot delete the file in between.
        let mut pending = PENDING.lock_ok();
        file.write_all(text.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("could not write {shown}: {e}"))?;
        // Closed before the move: Windows will not move an open file in every case.
        drop(file);
        fs::rename(&self.temp, &self.destination).map_err(|e| format!("could not write {shown}: {e}"))?;
        // Nothing left to remove: Drop and the interrupt handler only clean up a file that was not moved.
        pending.retain(|path| *path != self.temp);
        self.temp = PathBuf::new();
        Ok(())
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.file.take();
        if !self.temp.as_os_str().is_empty() {
            let mut pending = PENDING.lock_ok();
            let _ = fs::remove_file(&self.temp);
            pending.retain(|path| *path != self.temp);
        }
    }
}

/// The temporary files that exist right now, so an interrupt can remove them: Ctrl-C ends the
/// process without running `Drop`, and the file is owner-only but still a stray secret holder.
/// More than one only in tests; a login has a single one.
static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Removes the temporary files that were not moved into place. Returns how many it removed.
fn remove_pending() -> usize {
    let mut pending = PENDING.lock_ok();
    let removed = pending.len();
    for path in pending.drain(..) {
        let _ = fs::remove_file(path);
    }
    removed
}

/// A browser window shares the console with this command, so Ctrl-C can end the driver a moment
/// before it reaches the handler. Gives the handler that moment before an error is reported.
fn interrupt_may_be_arriving() -> bool {
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if interrupt::CANCEL.load(Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    interrupt::CANCEL.load(Ordering::SeqCst)
}

/// Makes a new file that only its owner can open, before any secret is written to it.
fn create_private(path: &Path) -> Result<File, LoginError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|e| {
        LoginError::Usage(format!(
            "--save cannot be written: could not create {}: {e}",
            path.display()
        ))
    })?;
    #[cfg(windows)]
    if let Err(why) = restrict_to_owner(path) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(LoginError::Failed(why));
    }
    Ok(file)
}

/// Windows: removes the inherited access entries and leaves the owner alone (`*S-1-3-4` is the
/// well-known OWNER RIGHTS group). Done to the empty file, so there is no moment in which a
/// secret could be read by others. `icacls` is run by absolute path, not looked up on PATH.
#[cfg(windows)]
fn restrict_to_owner(path: &Path) -> Result<(), String> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let icacls = Path::new(&root).join("System32").join("icacls.exe");
    let output = std::process::Command::new(&icacls)
        .arg(path)
        .args(["/inheritance:r", "/grant:r", "*S-1-3-4:(F)"])
        .output()
        .map_err(|e| {
            format!(
                "could not make the state file owner-only: could not run {}: {e}",
                icacls.display()
            )
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "could not make the state file owner-only, so it was not saved: icacls said {}",
            String::from_utf8_lossy(&output.stdout).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// An empty folder of its own, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            let path = std::env::temp_dir().join(format!(
                "traceqa-login-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn state() -> StorageState {
        StorageState {
            cookies: vec![],
            origins: vec![],
        }
    }

    fn save(path: &Path) -> Result<(), String> {
        match Pending::prepare(path) {
            Ok(pending) => pending.commit(&state()),
            Err(LoginError::Usage(why) | LoginError::Failed(why)) => Err(why),
            Err(LoginError::Interrupted) => Err("interrupted".into()),
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn save_creates_the_file_with_mode_0600() {
        let dir = Scratch::new();
        save(&dir.path("state.json")).unwrap();
        assert_eq!(mode(&dir.path("state.json")), 0o600);
        assert_eq!(dir.names(), ["state.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn save_overwrites_an_existing_0644_file_and_ends_with_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Scratch::new();
        let path = dir.path("state.json");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        save(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert!(fs::read_to_string(&path).unwrap().contains("cookies"));
    }

    #[cfg(unix)]
    #[test]
    fn save_replaces_a_symlink_instead_of_writing_through_it() {
        let dir = Scratch::new();
        fs::write(dir.path("real"), "keep").unwrap();
        std::os::unix::fs::symlink(dir.path("real"), dir.path("link")).unwrap();
        save(&dir.path("link")).unwrap();
        assert_eq!(fs::read_to_string(dir.path("real")).unwrap(), "keep");
        assert!(!fs::symlink_metadata(dir.path("link")).unwrap().file_type().is_symlink());
        assert_eq!(mode(&dir.path("link")), 0o600);
    }

    #[cfg(windows)]
    #[test]
    fn save_leaves_only_owner_rights_on_the_file() {
        let dir = Scratch::new();
        let path = dir.path("state.json");
        // An existing file with inherited entries, as most files in a user's folder have.
        fs::write(&path, "old").unwrap();
        save(&path).unwrap();
        let root = std::env::var_os("SystemRoot").unwrap();
        let output = std::process::Command::new(Path::new(&root).join("System32").join("icacls.exe"))
            .arg(&path)
            .output()
            .unwrap();
        let listing = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(listing.contains("OWNER RIGHTS:(F)"), "{listing}");
        for other in ["Everyone", "BUILTIN\\Users", "Authenticated Users", "(I)"] {
            assert!(!listing.contains(other), "{other} still has access: {listing}");
        }
    }

    #[test]
    fn save_failure_leaves_no_temp_file() {
        let dir = Scratch::new();
        let path = dir.path("state.json");
        let pending = Pending::prepare(&path).unwrap();
        assert_eq!(dir.names().len(), 1, "the temporary file exists while waiting");
        // The destination became a directory while the person signed in: the move fails.
        fs::create_dir(&path).unwrap();
        assert!(pending.commit(&state()).is_err());
        assert_eq!(dir.names(), ["state.json"], "only the directory is left");
    }

    fn is_pending(path: &Path) -> bool {
        PENDING
            .lock_ok()
            .iter()
            .any(|pending| pending.starts_with(path.parent().unwrap()))
    }

    #[test]
    fn a_committed_file_is_no_longer_pending_so_the_handler_cannot_remove_it() {
        let dir = Scratch::new();
        let path = dir.path("state.json");
        let pending = Pending::prepare(&path).unwrap();
        assert!(is_pending(&path), "an interrupt now would remove the temporary file");
        pending.commit(&state()).unwrap();
        assert!(!is_pending(&path), "nothing left for the handler to remove");
        assert_eq!(dir.names(), ["state.json"]);
    }

    #[test]
    fn a_pending_file_that_is_dropped_removes_itself() {
        let dir = Scratch::new();
        drop(Pending::prepare(&dir.path("state.json")).unwrap());
        assert_eq!(dir.names(), Vec::<String>::new());
    }

    #[test]
    fn a_directory_as_save_is_a_usage_error() {
        let dir = Scratch::new();
        assert!(matches!(Pending::prepare(&dir.0), Err(LoginError::Usage(why)) if why.contains("directory")));
    }

    #[test]
    fn a_missing_folder_is_a_usage_error() {
        let dir = Scratch::new();
        let result = Pending::prepare(&dir.path("missing").join("state.json"));
        assert!(matches!(result, Err(LoginError::Usage(why)) if why.contains("does not exist")));
    }

    fn usage(url: &str) -> Option<String> {
        match check_address(url) {
            Err(LoginError::Usage(why)) => Some(why),
            Err(LoginError::Failed(_) | LoginError::Interrupted) => panic!("an address problem is a usage error"),
            Ok(()) => None,
        }
    }

    #[test]
    fn only_http_and_https_addresses_are_accepted() {
        assert!(usage("ftp://example.com/").is_some_and(|why| why.contains("http or https")));
        assert!(usage("example.com").is_some());
        assert!(usage("https://example.com/").is_none());
        assert!(usage("http://localhost:8080/a").is_none());
    }

    #[test]
    fn a_user_name_or_password_in_the_address_is_a_usage_error() {
        for url in [
            "https://u:p@h/",
            "https://user@example.com",
            "http://a\tb:c@example.com/",
        ] {
            let why = usage(url).unwrap_or_else(|| panic!("{url} was accepted"));
            assert!(why.contains("--storage-state"), "{why}");
            assert!(
                !why.contains(":p@") && !why.contains("user@"),
                "the secret is echoed: {why}"
            );
        }
    }

    #[test]
    fn an_at_sign_after_the_host_is_not_user_information() {
        assert!(usage("https://example.com/path@x").is_none());
        assert!(usage("https://example.com?next=a@b").is_none());
        assert!(usage("https://example.com#a@b").is_none());
    }
}
