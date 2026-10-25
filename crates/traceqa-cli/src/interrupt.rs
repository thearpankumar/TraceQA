//! Ctrl-C, SIGTERM and SIGHUP for the commands that run for a while (`extract`, `login`).
//!
//! The first signal sets [`CANCEL`]; the command then stops at its next step and closes what it
//! opened. A second signal, or a first one whose cleanup takes too long, ends the process at once
//! after killing the drivers it started. ctrlc runs one handler for all three signals and does not
//! say which arrived, so all of them end the command with 130.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use traceqa_engine::Abort;

/// Set by the first signal. The command then stops at the next step and closes its browsers.
pub static CANCEL: AtomicBool = AtomicBool::new(false);

/// Set by the second signal. The engine then writes the final run folder at once, without waiting
/// for the browsers, and the watchdog ends the process.
pub static ABORT: Abort = Abort::new();

/// Exit code of a command stopped by Ctrl-C, SIGTERM or SIGHUP, as shells report an interrupted command.
pub const INTERRUPTED: u8 = 130;

/// How long the process waits for the run folder to be closed after a second signal.
const ABORT_WAIT: Duration = Duration::from_secs(2);

/// What a command wants from the handler.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Printed on the first signal.
    pub first_message: &'static str,
    /// Whether the watchdog waits for the engine to close the run folder before exiting.
    pub waits_for_run_folder: bool,
    /// Run by the watchdog just before the process exits (removes a temporary file).
    pub on_exit: fn(),
    /// End the process this long after the first signal even without a second one.
    pub first_signal_bound: Option<Duration>,
}

/// Writes a line to standard error and ignores a failure. `eprintln!` panics when the write fails,
/// and after SIGHUP the terminal is gone.
pub fn say(text: &str) {
    let _ = writeln!(io::stderr(), "{text}");
}

/// Installs the handler and the watchdog thread. Call once per process (ctrlc allows one handler).
pub fn install(hooks: Hooks) {
    let first_message = hooks.first_message;
    // The handler only sets flags, so no file or process work happens inside it.
    let handler = move || {
        if CANCEL.swap(true, Ordering::SeqCst) {
            ABORT.request();
            return;
        }
        say(first_message);
    };
    if let Err(error) = ctrlc::set_handler(handler) {
        say(&format!(
            "traceqa: Ctrl-C will stop the command without closing it cleanly: {error}"
        ));
        return;
    }
    let watchdog = std::thread::Builder::new()
        .name("abort-watchdog".into())
        .spawn(move || {
            let mut first_seen: Option<Instant> = None;
            loop {
                if ABORT.is_requested() {
                    say("\nQuitting at once.");
                    if hooks.waits_for_run_folder {
                        ABORT.wait_closed(ABORT_WAIT);
                    }
                    break;
                }
                if CANCEL.load(Ordering::SeqCst) {
                    let since = *first_seen.get_or_insert_with(Instant::now);
                    if hooks.first_signal_bound.is_some_and(|bound| since.elapsed() >= bound) {
                        say("\nStopping took too long; quitting.");
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            traceqa_engine::driver::kill_all_drivers();
            (hooks.on_exit)();
            std::process::exit(i32::from(INTERRUPTED));
        });
    if let Err(error) = watchdog {
        say(&format!("traceqa: a second Ctrl-C will not quit at once: {error}"));
    }
}
